use tracing::{debug, error, info};
use tokio_tree_context::Context;
use crate::util::{self, ConnectionId, StreamId};
use quinn::{RecvStream, SendStream};
use tokio::net::TcpStream;
use anyhow::Result;
use crate::messages;

pub async fn run_server(mut context: Context, endpoint: quinn::Endpoint) {
    info!("run_server started main loop");
    loop {
        let incoming = endpoint.accept().await;
        match incoming {
            Some(incoming) => {
                let child_context = context.new_child_context();
                context.spawn(handle_server_connection(child_context, incoming));
            }
            None => {
                break;
            }
        }
    }
    info!("run_server ended main loop. endpoint closed");
}

async fn handle_server_connection(context: Context, incoming: quinn::Incoming) {
    let connection = incoming.await;
    match connection {
        Ok(connection) => {
            let connection_id: ConnectionId = Default::default();
            let connection_id_clone = connection_id.clone();
            info!("{} accepted from {}", connection_id, connection.remote_address());
            handle_server_connection_inner(context, connection_id,connection).await;
            info!("{} closed", connection_id_clone);
        }
        Err(e) => {
            info!("Failed to accept connection due to ConnectionError: {}", e);
            return;
        }
    }
}

async fn handle_server_connection_inner(mut context: Context, 
    connection_id: ConnectionId, 
    mut connection: quinn::Connection) {
    let keep_alive_stream = my_accept_bi(&mut connection).await;
    match keep_alive_stream {
        Ok((send_stream, recv_stream)) => {
            context.spawn(util::keep_alive(recv_stream, send_stream, None));
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
                let child_context = context.new_child_context();
                context.spawn(handle_server_stream_inner(child_context, stream_id, stream));
            }
            Err(e) => {
                error!("{} failed to accept stream: {}", connection_id, e);
                break;
            }
        }
    }
}


async fn handle_server_stream_inner(mut context: Context, stream_id: StreamId, stream: (SendStream, RecvStream)) ->Result<()> {
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

    let tcp_stream = TcpStream::connect(target_address).await;
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

    let (read_tcp, write_tcp) = tcp_stream.into_split();
    let join_handle = context.spawn(async move {
        info!("{} spawning pipe to copy data between client and upstream", stream_id);
        let result = util::run_pipe((read, write), (read_tcp, write_tcp)).await;
        match result {
            Ok((total_copied1, total_copied2)) => {
                info!("{} copied bytes: client -> upstream: {}, upstream -> client: {}", 
                stream_id,
                total_copied1, total_copied2);
            }
            Err(e) => {
                error!("{} failed to copy data: {}", stream_id, e);
            }
        }
        info!("{} connection closed", stream_id);
    });
    let _ = join_handle.await;
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
                match e {
                    quinn::ConnectionError::TimedOut => {
                        info!("accept_bi timed out");
                        return Err(anyhow::anyhow!("accept_bi timed out"));
                    }
                    _ => {
                        error!("accept_bi ended: failed to accept quic bi stream: {}", e);
                        return Err(anyhow::anyhow!("accept_bi ended: failed to accept quic bi stream: {}", e));
                    }
                }
            }
        }
    } 
}