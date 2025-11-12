use tracing::{debug, error, info};
use anyhow::Result;
use quinn::{RecvStream, SendStream};
use rand::Rng;
use std::{net::SocketAddr, time::Duration};
use tokio::net::{TcpListener, TcpStream, lookup_host};

use crate::util::{self, ConnectionId, read_length_prefixed};
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

pub async fn run_client(mut tcp_listener: TcpListener, target_address: String, endpoint: quinn::Endpoint, server_address: String) {
    info!("Client started main loop for target address: {}. Listening on: {}", target_address, tcp_listener.local_addr().unwrap());
    let mut loop_counter:usize = 0;
    tokio::spawn(print_client_stats());
    loop {
        loop_counter += 1;
        if loop_counter > 1 {
            info!("Client sleeping for 5 seconds before retrying");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let addrs= lookup_server_address(&server_address).await;
        if addrs.is_empty() {
            error!("Client failed to lookup server address: {}", server_address);
            continue;
        }
        let random_addr = addrs[rand::rng().random_range(0..addrs.len())];
        let server_name = get_server_name(&server_address);
        let connection = endpoint.connect(random_addr, &server_name);
        match connection {
            Ok(connection) => {
                let conection = connection.await;
                match conection {
                    Ok(connection) => {
                        info!("Client connected to server: {:?}", random_addr);
                        let connection_id:ConnectionId = Default::default();
                        client_stats::increment_active_connections();
                        let _ = handle_server_connection( connection_id,&mut tcp_listener, target_address.clone(), connection).await;
                        client_stats::decrement_active_connections();
                        info!("ended: connection closed");
                    }
                    Err(e) => {
                        error!("Client failed to connect to QUIC server(1): {:?} due to {}", random_addr, e);
                    }
                }
            }
            Err(e) => {
                error!("Client failed to connect to QUIC server(2): {:?} due to {}", random_addr, e);
            }
        }
    }
}

async fn handle_server_connection(connection_id: ConnectionId, tcp_listener: &mut TcpListener, target_address: String, mut connection: quinn::Connection) {
    let keep_alive_stream = my_open_bi(&mut connection).await;
    match keep_alive_stream {
        Ok((send_stream, recv_stream)) => {
            info!("{} Client starting keep alive stream...", connection_id);
            tokio::spawn(util::keep_alive(recv_stream, send_stream, None));
        }
        Err(e) => {
            error!("{} Client failed to open keep alive stream: {}", connection_id, e);
            return;
        }
    }

    let connection_id_clone = connection_id.clone();
    let result = handle_client_connection_loop(connection_id, tcp_listener, target_address, connection).await;
    match result {
        Ok(()) => {
            info!("{} connection closed", connection_id_clone);
        }
        Err(e) => {
            error!("{} failed to handle connection: {}", connection_id_clone, e);
        }
    }
}

async fn my_open_bi(connection: &mut quinn::Connection) -> Result<(SendStream, RecvStream)> {
    debug!("open_bi opening quic bi stream");
    let (mut send_stream, recv_stream) = connection.open_bi().await?;
    debug!("open_bi opened quic bi stream");
    debug!("writing dummy byte");
    send_stream.write_all(&[0u8]).await?;
    debug!("dummy byte written");
    Ok((send_stream, recv_stream))
}

async fn handle_client_connection(connection_id: ConnectionId, tcp_stream: TcpStream, target_address: String,  bi_stream: (SendStream, RecvStream)) -> Result<()> {
    info!("{} accepted tcp stream from {:?}", connection_id, tcp_stream.peer_addr()?);
    let (tcpr, tcpw) = tcp_stream.into_split();
    let (mut quic_send_stream, mut quic_recv_stream) = bi_stream;
    let stream_id = connection_id.next_stream_id();
    info!("{} opened QUIC bi stream with server: {}", connection_id, stream_id);
    let request = messages::build_connect_request(&target_address);
    info!("{} sending connection request to server, target address: {:?}", stream_id, target_address);
    quic_send_stream.write_all(&request).await?;
    let mut buffer = vec![0u8; 10];
    let response = read_length_prefixed(&mut quic_recv_stream, &mut buffer).await?;
    info!("{} server connect response received", stream_id);
    if response < 1 {
        error!("{} server connect response too short: response < 1", stream_id);
        return Err(anyhow::anyhow!("{} server connect response too short: response < 1", stream_id));
    } else {
        let response_bytes = buffer[0];
        if response_bytes == 0x00 {
            info!("{} server connect successful.", stream_id);
        } else {
            error!("{} server connect failed. Check server logs for details.", stream_id);
        }
    }
    tokio::spawn(
        async move {
            client_stats::increment_active_client_connections();
            client_stats::increment_active_streams();
            info!("{} spawning pipe to forward data between TCP and QUIC streams", stream_id);
            let result = util::run_pipe(
                (tcpr, tcpw), 
            (quic_recv_stream, quic_send_stream),
            client_stats::get_total_received_bytes_counter(),
            client_stats::get_total_sent_bytes_counter(),
            ).await;
            match result {
                Ok((total_copied1, total_copied2)) => {
                    info!("{} connection closed. total copied bytes: TCP -> QUIC: {}, QUIC -> TCP: {}", stream_id, total_copied1, total_copied2);
                }
                Err(e) => {
                    error!("{} connection failed: {}", stream_id, e);
                }
            }
            client_stats::decrement_active_client_connections();
            client_stats::decrement_active_streams();
        }
    );
    Ok(())
}

async fn handle_client_connection_loop(connection_id: ConnectionId, tcp_listener: &mut TcpListener, target_address: String,  connection: quinn::Connection) -> Result<()> {
    loop {
        let (tcp_stream, socket_addr) = tcp_listener.accept().await?;
        info!("{} accepted tcp connection from {:?}", connection_id, socket_addr);
        let connection_id_clone = connection_id.clone();
        let connection_id_clone2 = connection_id_clone.clone();
        let target_address_clone = target_address.clone();
        let mut connection_clone = connection.clone();
        let bi_stream = my_open_bi(&mut connection_clone).await?;
        tokio::spawn(async move {
            let result = handle_client_connection(connection_id_clone, tcp_stream, target_address_clone, bi_stream).await;
            match result {
                Ok(()) => {
                    info!("{} connection closed", connection_id_clone2);
                }
                Err(e) => {
                    error!("{} failed to handle connection: {}", connection_id_clone2, e);
                }
            }
        });
    }
}

async fn print_client_stats() {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let stats = ClientStatsClone::get();
        info!("client stats: total connections: {}, active connections: {}, total streams: {}, active streams: {}, total client connections: {}, active client connections: {}, sent bytes: {}, received bytes: {}", stats.total_connections, stats.active_connections, stats.total_streams, stats.active_streams, stats.total_client_connections, stats.active_client_connections, stats.total_sent_bytes, stats.total_received_bytes);
    }
}