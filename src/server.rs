use std::collections::HashMap;
use std::time::Duration;

use tracing::{debug, error, info};
use crate::server_stats::ServerStatsClone;
use crate::util::{self, ConnectionId, Extendable, StreamId, bytes_str, write_string};
use quinn::{RecvStream, SendStream};
use tokio::net::TcpStream;
use anyhow::Result;
use crate::messages;
use crate::server_stats;

pub async fn run_server(endpoint: Extendable<quinn::Endpoint>) {
    info!("{} starting server main loop", endpoint.get_attribute("name").unwrap());
    loop {
        let incoming = endpoint.as_ref().accept().await;
        match incoming {
            Some(incoming) => {
                tokio::spawn(async move {
                    server_stats::increment_active_connections();
                    let mut incoming_e = Extendable::new(incoming);
                    let connection_id:ConnectionId = Default::default();
                    incoming_e.attach(connection_id.clone());
                    let handle_result = handle_incoming_quic_connection(incoming_e).await;
                    if handle_result.is_err() {
                        let err = handle_result.err().unwrap();
                        info!("{} ended with error: {}", connection_id, err);
                    }
                    server_stats::decrement_active_connections();
                });
            }
            None => {
                info!("{} endpoint closed", endpoint.get_attribute("name").unwrap());
                break;
            }
        }
    }
    info!("{} endpoint closed", endpoint.get_attribute("name").unwrap());
}

async fn handle_incoming_quic_connection(incoming: Extendable<quinn::Incoming>) -> Result<()> {
    let connection_id = incoming.get::<ConnectionId>().unwrap().clone();
    let (incoming, _, _) = incoming.unwrap();
    let connection = incoming.accept()?.await?;
    let mut connection_e = Extendable::new(connection);
    connection_e.attach(connection_id.clone());
    info!("{} accepted from {}", connection_id, connection_e.as_ref().remote_address());
    let handle_result = handle_incoming_quic_connection_inner(connection_e).await?;
    Ok(handle_result)
}

async fn handle_incoming_quic_connection_inner(
    mut connection: Extendable<quinn::Connection>) -> Result<()> {
    let connection_id = connection.get::<ConnectionId>().unwrap().clone();
    debug!("{} sending connection metadata", connection_id);
    send_connection_metadata(&mut connection).await?;
    debug!("{} connection metadata sent", connection_id);
    info!("{} connection handle loop started", connection_id);
    let (keep_alive_send_stream, keep_alive_recv_stream) = my_accept_bi(&mut connection, "KeepAlive").await?;
    tokio::spawn(util::keep_alive(keep_alive_recv_stream, keep_alive_send_stream, None));
    loop {
        let stream = my_accept_bi(&mut connection, "ServerDataStream").await?;
        let (send_stream_e, recv_stream_e) = stream;
        let stream_id = recv_stream_e.get::<StreamId>().unwrap().clone();
        info!("{} accepted stream {}", connection_id, stream_id);
        tokio::spawn(async move {
            info!("{} stream handleing started", stream_id);
            server_stats::increment_active_streams();
            server_stats::increment_active_upstream_connections();
            let result = handle_server_stream_inner( send_stream_e, recv_stream_e).await;
            server_stats::decrement_active_streams();
            server_stats::decrement_active_upstream_connections();
            match result {
                Ok(()) => {
                }
                Err(e) => {
                    error!("{} stream handle error: {}", stream_id, e);
                }
            }
        });
    }
}


async fn handle_server_stream_inner(send_stream_e: Extendable<SendStream>, recv_stream_e: Extendable<RecvStream>) ->Result<()> {
    let stream_id = recv_stream_e.get::<StreamId>().unwrap().clone();

    let (mut write, _, _) = send_stream_e.unwrap();
    let (mut read, _, _) = recv_stream_e.unwrap();
    // first read length prefixed data for target address
    debug!("{} reading target address from client", stream_id);
    let target_address = util::read_string(&mut read, None).await?;
    info!("{} target address: {}. connecting...", stream_id, target_address);

    let tcp_stream = TcpStream::connect(&target_address).await;
    if tcp_stream.is_err() {
        let err = tcp_stream.err().unwrap();
        error!("{} failed to connect to target address: {}", stream_id, err);
        let response = messages::build_connect_response(false);
        write.write_all(&response).await?;
        return Err(anyhow::anyhow!("{} failed to connect to target address due to {}", stream_id, err));
    }
    let tcp_stream = tcp_stream.unwrap();
    let response = messages::build_connect_response(true);
    write.write_all(&response).await?;
    info!("{} connected to target address: {}", stream_id, target_address);

    let (read_tcp, write_tcp) = tcp_stream.into_split();
    info!("{} started piping data", stream_id);
    let (total_copied1, total_copied2) = util::run_pipe((read, write), (read_tcp, write_tcp), 
    server_stats::get_received_bytes_counter(), 
    server_stats::get_sent_bytes_counter()).await;
    info!("{} copied bytes: client -> upstream: {}, upstream -> client: {}", 
                stream_id,
                total_copied1, total_copied2);
    info!("{} stream closed", stream_id);
    Ok(())
}


// my_accept_bi read 1 byte dummy data
// then write the stream id to the client
async fn my_accept_bi(connection: &mut Extendable<quinn::Connection>, purpose: &str) -> Result<(Extendable<SendStream>, Extendable<RecvStream>)> {
    let connection_id = connection.get::<ConnectionId>().unwrap().clone();
    info!("{} accept_bi started for {}", connection_id, purpose);
    loop {
        let stream = connection.accept_bi().await?;
        debug!("{} accept_bi accepted bi stream", connection_id);
        let (send_stream, recv_stream) = stream;
        let stream_id = connection_id.next_stream_id();
        let mut send_stream_e = Extendable::new(send_stream);
        let mut recv_stream_e = Extendable::new(recv_stream);
        send_stream_e.attach(connection_id.clone());
        send_stream_e.attach(stream_id.clone());
        recv_stream_e.attach(connection_id.clone());
        recv_stream_e.attach(stream_id.clone());
        let mut buffer = vec![0u8; 1];
        let read_result = tokio::time::timeout(Duration::from_secs(1), recv_stream_e.read_exact(&mut buffer)).await;
        if read_result.is_err() {
            error!("{} accept_bi timeout on dummy byte read", connection_id);
            continue;
        }
        let read_result = read_result.unwrap();
        if read_result.is_err() {
            error!("{} accept_bi read dummy byte failed", connection_id);
            continue;
        }
        let send_meta_result = send_stream_metadata(&mut send_stream_e).await;
        if send_meta_result.is_err() {
            let err = send_meta_result.err().unwrap();
            error!("{} accept_bi failed to send stream metadata: {}", connection_id, err);
            continue;
        } else {
            debug!("{} metadata sent to client {}", connection_id, stream_id);
        }
        info!("{} accepted bi stream {} for {}", connection_id, stream_id, purpose);
        return Ok((send_stream_e, recv_stream_e));
    } 
}


pub async fn print_server_stats(stats_interval: usize) {
    if stats_interval == 0 {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(stats_interval as u64)).await;
        let stats = ServerStatsClone::get();
        info!("Stats: TC={},AC={},TS={},AS={},TUC={},AUC={},SENT={},RECV={}", 
            stats.total_connections, 
            stats.active_connections, 
            stats.total_streams, 
            stats.active_streams, 
            stats.total_upstream_connections, 
            stats.active_upstream_connections, 
            bytes_str(stats.sent_bytes), 
            bytes_str(stats.received_bytes));
    }
}

async fn send_connection_metadata(connection: &mut Extendable<quinn::Connection>) -> Result<()> {
    let mut metadata = HashMap::<String, String>::new();
    let connection_id = connection.get::<ConnectionId>().unwrap().clone();
    metadata.insert("connection_id".into(), connection_id.to_string());
    let json = util::encode_map_as_json(&metadata).await?;
    let mut uni_stream = connection.open_uni().await?;
    write_string(&mut uni_stream, &json).await?;
    uni_stream.finish()?;
    Ok(())
}

async fn send_stream_metadata(stream: &mut Extendable<SendStream>) -> Result<()> {
    let mut metadata = HashMap::<String, String>::new();
    let stream_id = stream.get::<StreamId>().unwrap().clone();
    metadata.insert("stream_id".into(), stream_id.to_string());
    let json = util::encode_map_as_json(&metadata).await?;
    write_string(stream.as_mut(), &json).await?;
    Ok(())
}


