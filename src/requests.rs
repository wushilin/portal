use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Result;
use lazy_static::lazy_static;
use quinn::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
};
use tracing::{debug, error, info, warn};

use crate::server_stats;
use crate::{
    aclutil,
    util::{self, read_string, write_string, ConnectionId, Extendable, StreamId},
};

lazy_static! {
    static ref NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
}

pub fn get_next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Request {
    pub request_type: RequestType,
    pub request_id: u64,
    pub request_data: Option<HashMap<String, String>>,
}

impl Request {
    pub fn new(request_type: RequestType) -> Self {
        Self {
            request_type,
            request_id: get_next_request_id(),
            request_data: None,
        }
    }

    pub fn new_with_data(request_type: RequestType, request_data: HashMap<String, String>) -> Self {
        Self {
            request_type,
            request_id: get_next_request_id(),
            request_data: Some(request_data),
        }
    }

    pub fn add_data(&mut self, key: String, value: String) {
        if self.request_data.is_none() {
            self.request_data = Some(HashMap::new());
        }
        self.request_data.as_mut().unwrap().insert(key, value);
    }

    pub fn get_data(&self, key: &str) -> Option<&String> {
        if self.request_data.is_none() {
            return None;
        }
        self.request_data.as_ref().unwrap().get(key)
    }

    pub fn get_data_all(&self) -> Option<&HashMap<String, String>> {
        if self.request_data.is_none() {
            return None;
        }
        Some(self.request_data.as_ref().unwrap())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let request = serde_json::from_slice(bytes)?;
        Ok(request)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let request = serde_json::from_str(json)?;
        Ok(request)
    }

    pub fn build_success_response(&self) -> Response {
        Response::new(self.request_id, true, None, None)
    }

    pub fn build_error_response(&self, error_message: &str) -> Response {
        Response::new(
            self.request_id,
            false,
            Some(error_message.to_string()),
            None,
        )
    }
}

#[derive(Debug, PartialEq, EnumString, Serialize, Deserialize, Clone, Display)]
pub enum RequestType {
    GetStreamMetadata,
    Connect,
    Ping,
}

pub enum ServerNextStep {
    ContinueLoop,
    Disconnect,
    StartDataPiping(Extendable<OwnedReadHalf>, Extendable<OwnedWriteHalf>),
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Response {
    pub request_id: u64,
    pub success: bool,
    pub error_message: Option<String>,
    pub response_data: Option<HashMap<String, String>>,
}

impl Response {
    pub fn new(
        request_id: u64,
        success: bool,
        error_message: Option<String>,
        response_data: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            request_id,
            success,
            error_message,
            response_data,
        }
    }

    pub fn add_data(&mut self, key: String, value: String) {
        if self.response_data.is_none() {
            self.response_data = Some(HashMap::new());
        }
        self.response_data.as_mut().unwrap().insert(key, value);
    }

    pub fn get_data(&self, key: &str) -> Option<&String> {
        if self.response_data.is_none() {
            return None;
        }
        self.response_data.as_ref().unwrap().get(key)
    }

    pub fn get_data_all(&self) -> Option<&HashMap<String, String>> {
        if self.response_data.is_none() {
            return None;
        }
        Some(self.response_data.as_ref().unwrap())
    }

    pub fn success(request_id: u64) -> Self {
        Self::new(request_id, true, None, None)
    }

    pub fn is_ok(&self) -> bool {
        self.success
    }

    pub fn is_error(&self) -> bool {
        !self.success
    }

    pub fn get_error_message(&self) -> Option<&String> {
        self.error_message.as_ref()
    }

    pub fn get_response_data(&self) -> Option<&HashMap<String, String>> {
        self.response_data.as_ref()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let response = serde_json::from_slice(bytes)?;
        Ok(response)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let response = serde_json::from_str(json)?;
        Ok(response)
    }
}

pub fn create_ping_request() -> Request {
    Request::new(RequestType::Ping)
}

pub fn create_stream_meta_request() -> Request {
    Request::new(RequestType::GetStreamMetadata)
}

pub fn create_connect_request(target: &str, source_ip: &str) -> Request {
    let mut request = Request::new(RequestType::Connect);
    request.add_data("target".to_string(), target.to_string());
    request.add_data("source_ip".to_string(), source_ip.to_string());
    request
}

pub async fn client_keep_alive_inner<R, W>(
    mut read: util::Extendable<R>,
    mut write: util::Extendable<W>,
    _read_first: Option<bool>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    use tokio::time::{interval, Duration, MissedTickBehavior};

    let stream_id = read.get::<util::StreamId>().unwrap().clone();
    let ping_request = Request::new(RequestType::Ping);
    let mut interval = interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let response = make_request(read.as_mut(), write.as_mut(), &ping_request).await?;
        if response.is_error() {
            warn!(
                "{} keep_alive failed: {:?}",
                stream_id, response.error_message
            );
            break;
        }
    }
    Ok(())
}

pub async fn make_request<R, W>(
    reader: &mut R,
    writer: &mut W,
    request: &Request,
) -> Result<Response>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let request_bytes = serde_json::to_string(request)?;
    write_string(writer, &request_bytes).await?;
    let response_string = read_string(reader, Some(8096)).await?;
    let response = Response::from_json(&response_string)?;
    Ok(response)
}

pub async fn read_request<R>(reader: &mut R) -> Result<Request>
where
    R: AsyncRead + Unpin,
{
    let request_string = read_string(reader, Some(8096)).await?;
    let request = Request::from_json(&request_string)?;
    Ok(request)
}

pub async fn write_response<W>(writer: &mut W, response: &Response) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let response_string = serde_json::to_string(response)?;
    write_string(writer, &response_string).await?;
    Ok(())
}

// return (response, loop_action)
// response is sent to client, and if continue_loop is true, the loop continues
// if the continue_loop is false, start data piping
pub async fn handle_server_request(
    connection_id: &ConnectionId,
    stream_id: &StreamId,
    request: &Request,
) -> Result<(Response, ServerNextStep)> {
    let mut response = Response::success(request.request_id);
    let mut loop_action = ServerNextStep::ContinueLoop;
    debug!("{} handle server request started", stream_id);
    debug!("{} request {:?}", stream_id, request);
    match request.request_type {
        RequestType::GetStreamMetadata => {
            response.add_data("build_branch".to_string(), util::get_build_branch());
            response.add_data("build_time".to_string(), util::get_build_time());
            response.add_data("build_host".to_string(), util::get_build_host());
            response.add_data("connection_id".to_string(), connection_id.to_string());
            response.add_data("stream_id".to_string(), stream_id.to_string());
        }
        RequestType::Connect => {
            let target = request.get_data("target").unwrap();
            let source_ip = request.get_data("source_ip").unwrap();
            let acl_result = aclutil::is_acl_allowed(source_ip, target).await;
            if acl_result.is_err() {
                let err = acl_result.err().unwrap();
                error!("{} handle server request failed: {}", stream_id, err);
                response = request.build_error_response("ACL check failed");
                loop_action = ServerNextStep::Disconnect;
            } else {
                let acl_result = acl_result.unwrap();
                if !acl_result {
                    response = request.build_error_response("ACL denied");
                    loop_action = ServerNextStep::Disconnect;
                } else {
                    let tcp_stream = TcpStream::connect(target).await;
                    if tcp_stream.is_err() {
                        let err = tcp_stream.err().unwrap();
                        error!("{} handle server request failed: {}", stream_id, err);
                        response = request.build_error_response("TCP connection failed");
                        loop_action = ServerNextStep::Disconnect;
                    } else {
                        let tcp_stream = tcp_stream.unwrap();
                        let (read_tcp, write_tcp) = tcp_stream.into_split();
                        response = request.build_success_response();
                        let mut read_tcp_e = Extendable::new(read_tcp);
                        let mut write_tcp_e = Extendable::new(write_tcp);
                        read_tcp_e.attach(connection_id.clone());
                        read_tcp_e.attach(stream_id.clone());
                        write_tcp_e.attach(connection_id.clone());
                        write_tcp_e.attach(stream_id.clone());
                        loop_action = ServerNextStep::StartDataPiping(read_tcp_e, write_tcp_e);
                    }
                }
            }
        }
        RequestType::Ping => {
            response = Response::success(request.request_id);
            loop_action = ServerNextStep::ContinueLoop;
        }
    };

    debug!("{} handle server request ended", stream_id);
    debug!("{} response {:?}", stream_id, response);
    return Ok((response, loop_action));
}

pub async fn run_server_loop(
    mut recv_stream_e: Extendable<RecvStream>,
    mut send_stream_e: Extendable<SendStream>,
    purpose: &str,
) -> Result<()> {
    let connection_id = recv_stream_e.get::<ConnectionId>().unwrap().clone();
    let stream_id = recv_stream_e.get::<StreamId>().unwrap().clone();
    info!("{} server loop started for {}", stream_id, purpose);
    loop {
        debug!("{} reading request", stream_id);
        let request = read_request(recv_stream_e.as_mut()).await;
        if request.is_err() {
            let err = request.err().unwrap();
            info!("{} read request failed: {}", stream_id, err);
            break;
        }
        let request = request.unwrap();
        debug!("{} request read type {}", stream_id, request.request_type);
        let handle_result = handle_server_request(&connection_id, &stream_id, &request).await;
        if handle_result.is_err() {
            let err = handle_result.err().unwrap();
            error!("{} handle server request failed: {}", stream_id, err);
            break;
        }
        let (response, loop_action) = handle_result.unwrap();
        let write_result = write_response(send_stream_e.as_mut(), &response).await;
        if write_result.is_err() {
            let err = write_result.err().unwrap();
            error!("{} write response failed: {}", stream_id, err);
            break;
        }
        match loop_action {
            ServerNextStep::ContinueLoop => {
                debug!("{} continuing loop", stream_id);
                continue;
            }
            ServerNextStep::Disconnect => {
                info!("{} disconnecting", stream_id);
                break;
            }
            ServerNextStep::StartDataPiping(mut read_tcp_e, mut write_tcp_e) => {
                info!("{} starting data piping", stream_id);
                let (total_copied1, total_copied2) = util::run_pipe(
                    (read_tcp_e.as_mut(), write_tcp_e.as_mut()),
                    (recv_stream_e.as_mut(), send_stream_e.as_mut()),
                    server_stats::get_received_bytes_counter(),
                    server_stats::get_sent_bytes_counter(),
                )
                .await;
                info!(
                    "{} copied bytes: client -> upstream: {}, upstream -> client: {}",
                    stream_id, total_copied1, total_copied2
                );
                info!("{} data piping ended", stream_id);
                break;
            }
        }
    }
    info!("{} server loop ended for {}", stream_id, purpose);
    Ok(())
}

pub async fn client_keep_alive<R, W>(
    read: Extendable<R>,
    write: Extendable<W>,
    read_first: Option<bool>,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let connection_id = read.get::<ConnectionId>().unwrap().clone();
    info!("{} keep_alive started", connection_id);
    // Delegate to requests module to avoid circular dependency
    // Note: requests module is declared before util in main.rs to allow this
    let _ = client_keep_alive_inner(read, write, read_first).await;
    info!("{} keep_alive ended", connection_id);
}
