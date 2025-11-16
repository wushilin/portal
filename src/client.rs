use tracing::{error, info};
use anyhow::Result;
use quinn::{RecvStream, SendStream};
use rand::Rng;
use std::{net::SocketAddr, time::Duration};
use tokio::net::{TcpListener, TcpStream, lookup_host};

use crate::requests::{self, Request, RequestType};
use crate::util::{self, ConnectionId, Extendable, StreamId, bytes_str};
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
        
        let connection_e = Extendable::new(connection);
        client_stats::increment_active_connections();
        let handle_result = handle_server_connection( &mut tcp_listener, target_address.clone(), connection_e).await;
        if handle_result.is_err() {
            let err = handle_result.err().unwrap();
            error!("client failed to handle server connection due to: {}", err);
        }
        client_stats::decrement_active_connections();
    }
}

async fn handle_server_connection(tcp_listener: &mut TcpListener, target_address: String, mut connection: Extendable<quinn::Connection>) -> Result<()> {
    let (mut control_send, mut control_recv) = my_open_bi(&mut connection, "ClientControlStream").await?;
    let (connection_id, _) = client_hand_shake( &mut control_send, &mut control_recv).await?;
    connection.attach(connection_id.clone());
    tokio::spawn(client_control_loop(control_send, control_recv));
    let _ = handle_client_connection_loop(tcp_listener, target_address, connection).await?;
    Ok(())
}

async fn client_hand_shake(send_stream: &mut Extendable<SendStream>, recv_stream: &mut Extendable<RecvStream>) -> Result<(ConnectionId, StreamId)> {
    //let connection_id = send_stream.get::<ConnectionId>().unwrap().clone();
    let meta_request = Request::new(RequestType::GetStreamMetadata);
    let response = requests::make_request(recv_stream.as_mut(), send_stream.as_mut(), &meta_request).await?;
    if response.is_error() {
        return Err(anyhow::anyhow!("client hand shake failed: {:?}",response.error_message));
    }
    let data = response.get_response_data().unwrap();
    let connection_id_string = data.get("connection_id").unwrap().clone();
    let stream_id_string = data.get("stream_id").unwrap().clone();
    let build_branch = data.get("build_branch").unwrap().clone();
    let build_time = data.get("build_time").unwrap().clone();
    let build_host = data.get("build_host").unwrap().clone();

    let local_build_branch = util::get_build_branch();
    let local_build_time = util::get_build_time();
    let local_build_host = util::get_build_host();
    if build_branch != local_build_branch {
        return Err(anyhow::anyhow!("client hand shake failed: build branch mismatch: remote: {}, local: {}", build_branch, local_build_branch));
    }
    if build_time != local_build_time {
        return Err(anyhow::anyhow!("client hand shake failed: build time mismatch: remote: {}, local: {}", build_time, local_build_time));
    }
    if build_host != local_build_host {
        return Err(anyhow::anyhow!("client hand shake failed: build host mismatch: remote: {}, local: {}", build_host, local_build_host));
    }
    

    info!("client hand shake successful");
    info!(" > connection id: {}", connection_id_string);
    info!(" > stream id: {}", stream_id_string);
    info!(" > build branch: {}", build_branch);
    info!(" > build_time: {}", build_time);
    info!(" > build_host: {}", build_host);
    let connection_id = ConnectionId::from_string(connection_id_string)?;
    let stream_id = StreamId::from_string(stream_id_string)?;
    send_stream.attach(connection_id.clone());
    send_stream.attach(stream_id.clone());
    recv_stream.attach(connection_id.clone());
    recv_stream.attach(stream_id.clone());
    Ok((connection_id, stream_id))
}

async fn client_control_loop(send_stream: Extendable<SendStream>, recv_stream: Extendable<RecvStream>) -> Result<()> {
    requests::client_keep_alive(recv_stream, send_stream, None).await;
    Ok(())
}

async fn my_open_bi(connection: &mut Extendable<quinn::Connection>, purpose: &str) -> Result<(Extendable<SendStream>, Extendable<RecvStream>)> {
    let connection_id_str = {
        let connection_id = connection.get::<ConnectionId>();
        if connection_id.is_some() {
            connection_id.unwrap().to_string()
        } else {
            "unknown".to_string()
        }
    };

    info!("{} open_bi started for {}", connection_id_str, purpose);
    let (send_stream, recv_stream) = connection.open_bi().await?;
    let send_stream_e = Extendable::new(send_stream);
    let recv_stream_e = Extendable::new(recv_stream);
    Ok((send_stream_e, recv_stream_e))
}

async fn handle_client_connection(tcp_stream: TcpStream, target_address: String, bi_stream: (Extendable<SendStream>, Extendable<RecvStream>)) -> Result<()> {
    let source_ip = tcp_stream.peer_addr().unwrap().ip().to_string();
    let (tcpr, tcpw) = tcp_stream.into_split();
    let (mut quic_send_stream_e, mut quic_recv_stream_e) = bi_stream;
    let (_, stream_id) = client_hand_shake( &mut quic_send_stream_e, &mut quic_recv_stream_e).await?;
    quic_send_stream_e.attach(stream_id.clone());
    quic_recv_stream_e.attach(stream_id.clone());
    let meta_request = Request::new(RequestType::GetStreamMetadata);
    let response = requests::make_request(quic_recv_stream_e.as_mut(), quic_send_stream_e.as_mut(), &meta_request).await?;
    if response.is_error() {
        return Err(anyhow::anyhow!("client failed to get stream metadata due to: {:?}", response.error_message));
    }
    let data = response.get_response_data().unwrap();
    let connection_id_string = data.get("connection_id").unwrap().clone();
    let stream_id_string = data.get("stream_id").unwrap().clone();
    let connection_id = ConnectionId::from_string(connection_id_string)?;
    let stream_id = StreamId::from_string(stream_id_string)?;
    quic_send_stream_e.attach(connection_id.clone());
    quic_send_stream_e.attach(stream_id.clone());
    quic_recv_stream_e.attach(connection_id.clone());
    quic_recv_stream_e.attach(stream_id.clone());

    let request = requests::create_connect_request(&target_address, &source_ip);
    info!("{} sending connection request to server, target address: {:?} source ip: {}", stream_id, target_address, source_ip);
    //quic_send_stream_e.as_mut().write_all(&request).await?;
    let response = requests::make_request(quic_recv_stream_e.as_mut(), quic_send_stream_e.as_mut(), &request).await?;
    if response.is_error() {
        info!("{} server failed to connect to {} request due to: {:?}", stream_id, target_address, response.error_message.unwrap_or("[no error message]".into()));
        return Ok(());
    }
    info!("{} server connect to {} request successful", stream_id, target_address);
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
    // handshake for the control stream must be done now. 
    // client id is available but stream id is not available yet.
    let connection_id = connection.get::<ConnectionId>().unwrap().clone();
    loop {
        let connection_id_clone = connection_id.clone();
        let (tcp_stream, socket_addr) = tcp_listener.accept().await?;
        info!("{} accepted tcp connection from {:?}", connection_id_clone, socket_addr);
        let target_address_clone = target_address.clone();
        let bi_stream = my_open_bi(&mut connection, "ClientDataStream").await?;
        tokio::spawn(async move {
            let result = handle_client_connection(tcp_stream, target_address_clone, bi_stream).await;
            match result {
                Ok(()) => {
                }
                Err(e) => {
                    info!("{} stream pipe failed to start: {}", connection_id_clone, e);
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
