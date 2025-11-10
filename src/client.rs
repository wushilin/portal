use tracing::{debug, error, info};
use tokio_tree_context::Context;
use anyhow::Result;
use quinn::{RecvStream, SendStream};
use rand::Rng;
use std::{net::SocketAddr, time::Duration};
use tokio::net::{TcpListener, lookup_host};

use crate::util::{self, ConnectionId, read_length_prefixed};
use crate::messages;

async fn lookup_server_address(server: &str, port: u16) -> Vec<SocketAddr> {
    let server_addr = format!("{}:{}", server, port);
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

pub async fn run_client(mut context: Context, mut tcp_listener: TcpListener, target_address: String, endpoint: quinn::Endpoint, server: String, port: u16) {
    info!("Client started main loop");
    
    let mut loop_counter:usize = 0;
    loop {
        loop_counter += 1;
        if loop_counter > 1 {
            info!("Client sleeping for 5 seconds before retrying");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let addrs= lookup_server_address(&server, port).await;
        if addrs.is_empty() {
            error!("Client failed to lookup server address: {}:{}", server, port);
            continue;
        }
        let random_addr = addrs[rand::rng().random_range(0..addrs.len())];
        let connection = endpoint.connect(random_addr, &server);
        match connection {
            Ok(connection) => {
                let conection = connection.await;
                match conection {
                    Ok(connection) => {
                        info!("Client connected to server: {:?}", random_addr);
                        let connection_id:ConnectionId = Default::default();
                        let child_context = context.new_child_context();
                        let _ = handle_client_connection(child_context, connection_id,&mut tcp_listener, target_address.clone(), connection).await;
                        info!("handle_client_connection ended: connection closed");
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

async fn handle_client_connection(mut context: Context, connection_id: ConnectionId, tcp_listener: &mut TcpListener, target_address: String, mut connection: quinn::Connection) {
    let keep_alive_stream = my_open_bi(&mut connection).await;
    match keep_alive_stream {
        Ok((send_stream, recv_stream)) => {
            info!("{} Client starting keep alive stream...", connection_id);
            context.spawn(util::keep_alive(recv_stream, send_stream, None));
        }
        Err(e) => {
            error!("{} Client failed to open keep alive stream: {}", connection_id, e);
            return;
        }
    }

    let connection_id_clone = connection_id.clone();
    let result = handle_client_connection_loop(context, connection_id, tcp_listener, target_address, connection).await;
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

async fn handle_client_connection_loop(mut context: Context, connection_id: ConnectionId, tcp_listener: &mut TcpListener, target_address: String,  mut connection: quinn::Connection) -> Result<()> {
    loop {
        let (tcp_stream, _) = tcp_listener.accept().await?;
        info!("{} accepted tcp stream from {:?}", connection_id, tcp_stream.peer_addr()?);
        let (tcpr, tcpw) = tcp_stream.into_split();
        let (mut quic_send_stream, mut quic_recv_stream) = my_open_bi(&mut connection).await?;
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
                continue;
            }
        }
        info!("{} spawning pipe to forward data between TCP and QUIC streams", stream_id);
        context.spawn(
            async move {
                let result = util::run_pipe((tcpr, tcpw), (quic_recv_stream, quic_send_stream)).await;
                match result {
                    Ok((total_copied1, total_copied2)) => {
                        info!("{} connection closed. total copied bytes: TCP -> QUIC: {}, QUIC -> TCP: {}", stream_id, total_copied1, total_copied2);
                    }
                    Err(e) => {
                        error!("{} connection failed: {}", stream_id, e);
                    }
                }
            }
        );
    }
}