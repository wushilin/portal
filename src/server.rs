use std::time::Duration;

use tracing::{debug, info};
use crate::server_stats::ServerStatsClone;
use crate::util::{ConnectionId, Extendable, StreamId, bytes_str};
use quinn::{RecvStream, SendStream};
use anyhow::Result;
use crate::{requests};
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
    let accept_result = tokio::time::timeout(Duration::from_secs(3), my_accept_bi(&mut connection, "ServerControl")).await?;
    if accept_result.is_err() {
        return Err(anyhow::anyhow!("{} accept_bi timeout on server control stream", connection_id));
    }
    let (control_send, control_recv) = accept_result?;

    debug!("{} spawning server loop for control stream", connection_id);
    tokio::spawn(requests::run_server_loop(control_recv, control_send, "ServerControlStream"));
    debug!("{} server loop for control stream spawned", connection_id);
    info!("{} starting data link service loop", connection_id);
    loop {
        let stream = my_accept_bi(&mut connection, "ServerDataStream").await?;
        let (send_stream_e, recv_stream_e) = stream;
        let stream_id = recv_stream_e.get::<StreamId>().unwrap().clone();
        tokio::spawn(async move {
            debug!("{} spawning server loop for stream {}", stream_id, stream_id);
            server_stats::increment_active_streams();
            server_stats::increment_active_upstream_connections();
            let result = requests::run_server_loop( recv_stream_e, send_stream_e, "ServerDataStream").await;
            server_stats::decrement_active_streams();
            server_stats::decrement_active_upstream_connections();
            match result {
                Ok(()) => {
                }
                Err(e) => {
                    info!("{} server loop error: {}", stream_id, e);
                }
            }
            debug!("{} server loop ended for stream {}", stream_id, stream_id);
        });
    }
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

