use tracing::{debug, error, info};
use anyhow::Result;
use quinn::{RecvStream, SendStream};
use rand::Rng;
use std::collections::HashMap;
use std::{net::SocketAddr, time::Duration};
use tokio::net::{TcpListener, TcpStream, lookup_host};

use crate::util::{self, ConnectionId, Extendable, StreamId, bytes_str, read_length_prefixed};
use crate::messages;
use crate::client_stats::{self, ClientStatsClone};

async fn lookup_server_address(server_addr: &str) -> Vec<SocketAddr> {
    let addrs_result = lookup_host(&server_addr).await;
    let mut addrs = Vec::new();
    match addrs_result {
        Ok(addrs_iter) => {
            for addr in addrs_iter {
                addrs.push(addr);
            }
        }
        Err(e) => {
            error!("lookup_server_address failed: failed to lookup server address {}: {}", server_addr,e);
        }
    }
    return addrs;
}

fn get_server_name(server_address: &str) -> String {
    let tokens = server_address.rfind(':');
    if tokens.is_none() {
        return server_address.to_string();
    }
    return server_address[..tokens.unwrap()].to_string();
}

pub async fn connect_quic_server(endpoint: quinn::Endpoint, server_address: &str) -> Result<quinn::Connection> {
    let addrs= lookup_server_address(server_address).await;
    if addrs.is_empty() {
        return Err(anyhow::anyhow!("Client failed to lookup server address: {}", server_address));
    }
    let random_addr = addrs[rand::rng().random_range(0..addrs.len())];
    let server_name = get_server_name(&server_address);
    let connection = endpoint.connect(random_addr, &server_name)?.await?;
    return Ok(connection);
}

pub async fn run_client(mut tcp_listener: TcpListener, target_address: String, endpoint: quinn::Endpoint, server_address: String) -> Result<()> {
    info!("client started main loop for target address: {}. Listening on: {}", target_address, tcp_listener.local_addr().unwrap());
    let mut loop_counter:usize = 0;
    loop {
        loop_counter += 1;
        if loop_counter > 1 {
            info!("client sleeping for 5 seconds before retrying");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let connection = connect_quic_server(endpoint.clone(), &server_address).await;
        if connection.is_err() {
            error!("client failed to connect to QUIC server due to: {}", connection.err().unwrap());
            continue;
        }

        let connection = connection.unwrap();
        
        let mut connection_e = Extendable::new(connection);
        let metadata = receive_connection_metadata(&mut connection_e).await;
        if metadata.is_err() {
            error!("client failed to receive connection metadata due to: {}", metadata.err().unwrap());
            continue;
        }
        let metadata = metadata.unwrap();
        let connection_id = ConnectionId::from_string(metadata.get("connection_id").unwrap().clone())?;
        connection_e.attach(connection_id.clone());
        info!("{} client connection id from server received: {}", connection_id, metadata.get("connection_id").unwrap());
        client_stats::increment_active_connections();
        let handle_result = handle_server_connection( &mut tcp_listener, target_address.clone(), connection_e).await;
        if handle_result.is_err() {
            let err = handle_result.err().unwrap();
            error!("{} client failed to handle server connection due to: {}", connection_id, err);
        }
        client_stats::decrement_active_connections();
    }
}

async fn handle_server_connection(tcp_listener: &mut TcpListener, target_address: String, mut connection: Extendable<quinn::Connection>) -> Result<()> {
    let (keep_alive_send_stream, keep_alive_recv_stream) = my_open_bi(&mut connection, "KeepAlive").await?;
    tokio::spawn(util::keep_alive(keep_alive_recv_stream, keep_alive_send_stream, None));
    let _ = handle_client_connection_loop(tcp_listener, target_address, connection).await?;
    Ok(())
}

async fn my_open_bi(connection: &mut Extendable<quinn::Connection>, purpose: &str) -> Result<(Extendable<SendStream>, Extendable<RecvStream>)> {
    let connection_id = connection.get::<ConnectionId>().unwrap().clone();
    info!("{} open_bi started for {}", connection_id, purpose);
    let (mut send_stream, mut recv_stream) = connection.open_bi().await?;
    debug!("{} open_bi opened quic bi stream", connection_id);
    debug!("{} writing dummy byte", connection_id);
    send_stream.write_all(&[0u8]).await?;
    debug!("{} dummy byte written", connection_id);
    let metadata = receive_stream_metadata(&mut recv_stream).await?;
    let stream_id = StreamId::from_string(metadata.get("stream_id").unwrap().clone())?;
    debug!("{} open_bi stream id received {}", connection_id, stream_id);

    let mut send_stream_e = Extendable::new(send_stream);
    let mut recv_stream_e = Extendable::new(recv_stream);

    send_stream_e.attach(connection_id.clone());
    recv_stream_e.attach(connection_id.clone());
    send_stream_e.attach(stream_id.clone());
    recv_stream_e.attach(stream_id.clone());
    info!("{} open_bi stream id {} opened for purpose {}", connection_id, stream_id, purpose);
    Ok((send_stream_e, recv_stream_e))
}

async fn handle_client_connection(tcp_stream: TcpStream, target_address: String, bi_stream: (Extendable<SendStream>, Extendable<RecvStream>)) -> Result<()> {
    let source_ip = tcp_stream.peer_addr().unwrap().ip().to_string();
    let (tcpr, tcpw) = tcp_stream.into_split();
    let (mut quic_send_stream_e, mut quic_recv_stream_e) = bi_stream;
    let stream_id = quic_recv_stream_e.get::<StreamId>().unwrap().clone();
    let request = messages::build_connect_request(&target_address, &source_ip);
    info!("{} sending connection request to server, target address: {:?} source ip: {}", stream_id, target_address, source_ip);
    //quic_send_stream_e.as_mut().write_all(&request).await?;
    util::send_map(quic_send_stream_e.as_mut(), &request).await?;
    let mut buffer = vec![0u8; 10];
    let response = read_length_prefixed(quic_recv_stream_e.as_mut(), &mut buffer).await?;
    info!("{} server connect response received", stream_id);
    if response < 1 {
        return Err(anyhow::anyhow!("{} server connect response too short: response < 1", stream_id));
    } else {
        let response_bytes = buffer[0];
        if response_bytes == 0x00 {
            info!("{} server connect successful.", stream_id);
        } else {
            error!("{} server connect failed. Check server logs for details.", stream_id);
        }
    }
    let (quic_send_stream, _, _) = quic_send_stream_e.unwrap();
    let (quic_recv_stream, _, _) = quic_recv_stream_e.unwrap();
    tokio::spawn(
        async move {
            client_stats::increment_active_client_connections();
            client_stats::increment_active_streams();
            info!("{} spawning pipe for data forwarding", stream_id);
            let (total_copied1, total_copied2) = util::run_pipe(
                (tcpr, tcpw), 
            (quic_recv_stream, quic_send_stream),
            client_stats::get_total_received_bytes_counter(),
            client_stats::get_total_sent_bytes_counter(),
            ).await;
            info!("{} connection closed. total copied bytes: TCP -> QUIC: {}, QUIC -> TCP: {}", stream_id, total_copied1, total_copied2);
            client_stats::decrement_active_client_connections();
            client_stats::decrement_active_streams();
        }
    );
    Ok(())
}

async fn handle_client_connection_loop(tcp_listener: &mut TcpListener, target_address: String,  mut connection: Extendable<quinn::Connection>) -> Result<()> {
    let connection_id = connection.get::<ConnectionId>().unwrap().clone();
    loop {
        let (tcp_stream, socket_addr) = tcp_listener.accept().await?;
        info!("{} accepted tcp connection from {:?}", connection_id, socket_addr);
        let target_address_clone = target_address.clone();
        let bi_stream = my_open_bi(&mut connection, "ClientDataStream").await?;
        let stream_id = bi_stream.1.get::<StreamId>().unwrap().clone();
        tokio::spawn(async move {
            let result = handle_client_connection(tcp_stream, target_address_clone, bi_stream).await;
            match result {
                Ok(()) => {
                }
                Err(e) => {
                    error!("{} stream pipe failed to start: {}", stream_id, e);
                }
            }
        });
    }
}

pub async fn print_client_stats(stats_interval: usize) {
    if stats_interval == 0 {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(stats_interval as u64)).await;
        let stats = ClientStatsClone::get();
        info!("Stats: TC={},AC={},TS={},AS={},TCC={},ACC={},SENT={},RECV={}", 
            stats.total_connections, 
            stats.active_connections, 
            stats.total_streams, 
            stats.active_streams, 
            stats.total_client_connections, 
            stats.active_client_connections, 
            bytes_str(stats.total_sent_bytes), 
            bytes_str(stats.total_received_bytes));
    }
}

async fn receive_connection_metadata(connection: &mut Extendable<quinn::Connection>) -> Result<HashMap<String, String>> {
    let mut uni_stream = connection.accept_uni().await?;
    let json = util::read_string(&mut uni_stream, Some(4096)).await?;
    let metadata = util::decode_json_as_map(&json).await?;
    Ok(metadata)
}

async fn receive_stream_metadata(stream: &mut RecvStream) -> Result<HashMap<String, String>> {
    let json = util::read_string(stream, None).await?;
    let metadata = util::decode_json_as_map(&json).await?;
    Ok(metadata)
}