use std::time::Duration;

use tracing::{debug, error, info};
use crate::server_stats::ServerStatsClone;
use crate::util::{self, ConnectionId, StreamId, bytes_str};
use quinn::{RecvStream, SendStream};
use tokio::net::TcpStream;
use anyhow::Result;
use crate::messages;
use crate::server_stats;

pub async fn run_server(endpoint: quinn::Endpoint) {
    info!("starting server main loop");
    tokio::spawn(print_server_stats());
    loop {
        let incoming = endpoint.accept().await;
        match incoming {
            Some(incoming) => {
                tokio::spawn(handle_server_connection(incoming));
            }
            None => {
                break;
            }
        }
    }
    info!("endpoint closed");
}

async fn handle_server_connection(incoming: quinn::Incoming) {
    let connection = incoming.await;
    match connection {
        Ok(connection) => {
            let connection_id: ConnectionId = Default::default();
            let connection_id_clone = connection_id.clone();
            info!("{} accepted from {}", connection_id, connection.remote_address());
            server_stats::increment_active_connections();
            handle_server_connection_inner(connection_id,connection).await;
            server_stats::decrement_active_connections();
            info!("{} connection closed", connection_id_clone);
        }
        Err(e) => {
            info!("failed to accept connection due to ConnectionError: {}", e);
            return;
        }
    }
}

async fn handle_server_connection_inner(
    connection_id: ConnectionId, 
    mut connection: quinn::Connection) {
    info!("{} connection handle loop started", connection_id);
    let keep_alive_stream = my_accept_bi(&mut connection).await;
    match keep_alive_stream {
        Ok((send_stream, recv_stream)) => {
            tokio::spawn(util::keep_alive(recv_stream, send_stream, None));
        }
        Err(e) => {
            error!("{} failed to open keep alive stream: {}", connection_id, e);
            return;
        }
    }
    loop {
        let stream = my_accept_bi(&mut connection).await;
        match stream {
            Ok(stream) => {
                let stream_id = connection_id.next_stream_id();
                info!("{} accepted stream {}", connection_id, stream_id);
                tokio::spawn(async move {
                    info!("{} spawning stream handle loop", stream_id);
                    server_stats::increment_active_streams();
                    server_stats::increment_active_upstream_connections();
                    let result = handle_server_stream_inner( stream_id.clone(), stream).await;
                    server_stats::decrement_active_streams();
                    server_stats::decrement_active_upstream_connections();
                    match result {
                        Ok(()) => {
                        }
                        Err(e) => {
                            error!("{} stream handle error: {}", stream_id, e);
                        }
                    }
                    info!("{} stream handle loop ended", stream_id);
                });
            }
            Err(e) => {
                error!("{} failed to accept stream: {}", connection_id, e);
                break;
            }
        }
    }
    info!("{} connection handle loop ended", connection_id);
}


async fn handle_server_stream_inner(stream_id: StreamId, stream: (SendStream, RecvStream)) ->Result<()> {
    info!("{} handling started", stream_id);

    let mut read = stream.1;
    let mut write = stream.0;
    // first read length prefixed data for target address
    let mut buffer = vec![0u8; 1024];
    info!("{} reading target address data", stream_id);
    let n = util::read_length_prefixed(&mut read, &mut buffer).await?;
    debug!("{} read target address data: n: {:?}", stream_id, n);
    if n == 0 {
        return Err(anyhow::anyhow!("{} failed to read target address: total bytes == 0", stream_id));
    }
    let target_address = String::from_utf8(buffer[..n].to_vec()).unwrap();
    info!("{} read target address: {:?}", stream_id, target_address);

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
    info!("{} starting pipe to copy data between client and upstream", stream_id);
    let (total_copied1, total_copied2) = util::run_pipe((read, write), (read_tcp, write_tcp), 
    server_stats::get_received_bytes_counter(), 
    server_stats::get_sent_bytes_counter()).await;
    info!("{} copied bytes: client -> upstream: {}, upstream -> client: {}", 
                stream_id,
                total_copied1, total_copied2);
    info!("{} stream closed", stream_id);
    Ok(())
}


async fn my_accept_bi(connection: &mut quinn::Connection) -> Result<(SendStream, RecvStream)> {
    loop {
        let stream = connection.accept_bi().await;
        match stream {
            Ok(mut stream) => {
                let mut buffer = vec![0u8; 1];
                let read_result = stream.1.read_exact(&mut buffer).await;
                match read_result {
                    Ok(()) => {
                        debug!("accept_bi read dummy byte: {:?}", buffer[0]);
                        return Ok(stream);
                    }
                    Err(e) => {
                        error!("accept_bi ended: failed to read dummy byte: {}", e);
                        continue;
                    }
                }
            }
            Err(e) => {
                return Err(anyhow::anyhow!("accept_bi timed out: {}", e));
            }
        }
    } 
}


pub async fn print_server_stats() {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
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
