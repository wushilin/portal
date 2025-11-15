use anyhow::Result;
use lazy_static::lazy_static;
use short_uuid::{ShortUuid, short};
use tokio::time::MissedTickBehavior;
use tracing::{debug, info};
use std::collections::HashMap;
use std::fmt::{self, Display};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::any::{Any, TypeId};

lazy_static! {
    static ref NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(0);
}

pub async fn read_string<R>(stream: &mut R, max_length: Option<usize>) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; max_length.unwrap_or(1024)];
    let n = read_length_prefixed(stream, &mut buffer).await?;
    if n == 0 {
        return Err(anyhow::anyhow!("read_string failed to read string: n == 0"));
    }
    let result = String::from_utf8(buffer[..n].to_vec())?;
    return Ok(result);
}

pub async fn write_string<W>(stream: &mut W, string: &str) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_length_prefixed(stream, string.as_bytes()).await?;
    Ok(())
}

pub async fn read_length_prefixed<R>(stream: &mut R, buffer: &mut [u8]) -> Result<usize>
where
    R: AsyncRead + Unpin,
{
    if buffer.len() < 4 {
        return Err(anyhow::anyhow!("Buffer too small to read length prefix"));
    }
    stream.read_exact(&mut buffer[..4]).await?;
    let length = u32::from_be_bytes(buffer[..4].try_into()?) as usize;
    if buffer.len() < length {
        return Err(anyhow::anyhow!(
            "Buffer too small to read data. needed: {}, got: {}",
            length,
            buffer.len()
        ));
    }
    stream.read_exact(&mut buffer[..length]).await?;
    Ok(length)
}

pub async fn write_length_prefixed<W>(stream: &mut W, buffer: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    if buffer.len() > u32::MAX as usize {
        return Err(anyhow::anyhow!("Buffer too large to write length prefix"));
    }
    stream
        .write_all(&(buffer.len() as u32).to_be_bytes())
        .await?;
    stream.write_all(buffer).await?;
    Ok(())
}

pub async fn run_pipe_one_way<R, W>(mut reader: R, mut writer: W) -> Result<usize>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 4096];
    let mut total_copied = 0;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).await.is_err() {
                    break;
                } else {
                    // this copy ok
                    total_copied += n;
                }
            }
            Err(cause) => {
                return Err(anyhow::anyhow!("Error reading from reader: {}", cause));
            }
        }
    }
    Ok(total_copied)
}

pub async fn run_pipe<R1, W1, R2, W2>(stream1: (R1, W1), stream2: (R2, W2), r1_to_w2: Arc<AtomicUsize>, r2_to_w1: Arc<AtomicUsize>) -> (usize, usize)
where
    R1: AsyncRead + Unpin + Send + Sync,
    W1: AsyncWrite + Unpin + Send + Sync,
    R2: AsyncRead + Unpin + Send + Sync,
    W2: AsyncWrite + Unpin + Send + Sync,
{
    let (reader1, writer1) = stream1;
    let (reader2, writer2) = stream2;
    let mut async_copy1 = AsyncCopy::new(reader1, writer2);
    let mut async_copy2 = AsyncCopy::new(reader2, writer1);
    let local_copy1 = Arc::new(AtomicUsize::new(0));
    let local_copy2 = Arc::new(AtomicUsize::new(0));
    let r1_to_w2 = vec![r1_to_w2, local_copy1.clone()];
    let r2_to_w1 = vec![r2_to_w1, local_copy2.clone()];
    let _ = tokio::select!(
        result = async_copy1.copy(&r1_to_w2) => result,
        result = async_copy2.copy(&r2_to_w1) => result,
    );
    return (local_copy1.load(Ordering::Relaxed), local_copy2.load(Ordering::Relaxed));
}

#[derive(Debug)]
pub struct ConnectionId(u64,Arc<AtomicU64>);

impl Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn-{}", self.0.to_string())
    }
}

impl Clone for ConnectionId {
    fn clone(&self) -> Self {
        Self(self.0, self.1.clone())
    }
}

impl ConnectionId {
    pub(crate) fn new() -> Self {
        Self(NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed), Arc::new(AtomicU64::new(0)))
    }

    pub fn from_string(connection_id: String) -> Result<Self> {
        if connection_id.starts_with("conn-") {
            return Ok(Self(connection_id[5..].parse::<u64>()?, Arc::new(AtomicU64::new(0))));
        }
        return Err(anyhow::anyhow!("invalid connection id: {}", connection_id));
    }

    pub fn get(&self) -> String {
        self.0.to_string()
    }

    pub fn next_stream_id(&self) -> StreamId {
        StreamId::new(self.0, self.1.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct StreamId(u64,u64);

impl Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stream-{}-{}", self.0, self.1)
    }
}

impl StreamId {
    pub(crate) fn new(connection_id: u64, stream_id: u64) -> Self {
        Self(connection_id, stream_id)
    }

    pub fn from_string(stream_id: String) -> Result<Self> {
        let parts = stream_id.split("-").collect::<Vec<&str>>();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("invalid stream id: {}", stream_id));
        }
        Ok(Self(parts[1].parse::<u64>()?, parts[2].parse::<u64>()?))
    }

    pub fn connection_id(&self) -> u64 {
        self.0
    }

    pub fn stream_id(&self) -> u64 {
        self.1
    }
}

pub async fn keep_alive<R, W>(read: Extendable<R>, write: Extendable<W>, read_first: Option<bool>) 
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let connection_id = read.get::<ConnectionId>().unwrap().clone();
    info!("{} keep_alive started", connection_id);
    let _ = keep_alive_inner(read, write, read_first).await;
    info!("{} keep_alive ended", connection_id);
}
pub async fn keep_alive_inner<R, W>(mut read: Extendable<R>, mut write: Extendable<W>, read_first: Option<bool>) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let payload = [0u8; 1];
    let mut buf = vec![0u8; 1];
    let read_first = read_first.unwrap_or(false);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if read_first {
            read.read_exact(&mut buf).await?;
            debug!("keep_alive read dummy byte");
            write.write_all(&payload).await?;
            debug!("keep_alive wrote dummy byte");
        } else {
            write.write_all(&payload).await?;
            debug!("keep_alive wrote dummy byte");
            read.read_exact(&mut buf).await?;
            debug!("keep_alive read dummy byte");
        }
    }
}


pub struct AsyncCopy<R, W> where R: AsyncRead + Unpin, W: AsyncWrite + Unpin {
    read:  R,
    write: W,
}

impl<R, W> AsyncCopy<R, W> where R: AsyncRead + Unpin, W: AsyncWrite + Unpin {
    pub fn new(read: R, write: W) -> Self {
        Self { read, write }
    }

    pub async fn copy(&mut self, progresses:&Vec<Arc<AtomicUsize>>) -> Result<usize> {
        let mut buf = vec![0u8; 4096];
        let mut total_copied = 0;
        loop {
            let n = self.read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            if self.write.write_all(&buf[..n]).await.is_err() {
                break;
            }
            for progress in progresses {
                progress.fetch_add(n, Ordering::Relaxed);
            }
            total_copied += n;
        }
        Ok(total_copied)
    }
}

pub fn bytes_str(size:usize) -> String {
    humansize::format_size(size, humansize::BINARY)
}

pub fn uuid() -> ShortUuid {
    short!()
}

pub struct Extendable<T> {
    data: T,
    extensions: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    attributes: HashMap<String, String>,
}

impl<T> Extendable<T> {
    pub fn new(data: T) -> Self {
        Self { data, extensions: HashMap::new(), attributes: HashMap::new() }
    }

    pub fn attach_all(&mut self, extensions: HashMap<TypeId, Box<dyn Any + Send + Sync>>) 
    {
        for (key, value) in extensions {
            self.extensions.insert(key, value);
        }
    }

    pub fn attach<U: Any + Send + Sync>(&mut self, value: U) {
        self.extensions.insert(TypeId::of::<U>(), Box::new(value));
    }

    pub fn get_attributes(&self) -> &HashMap<String, String> {
        &self.attributes
    }

    pub fn count_attributes(&self) -> usize {
        self.attributes.len()
    }

    pub fn set_attribute(&mut self, key:String, value:String) {
        self.attributes.insert(key, value);
    }

    pub fn get_attribute(&self, key:&str) -> Option<&String> {
        self.attributes.get(key)
    }
    pub fn delete_attribute(&mut self, key:String) {
        self.attributes.remove(&key);
    }

    pub fn clear_attributes(&mut self) {
        self.attributes.clear();
    }

    pub fn get<U: Any + Send + Sync>(&self) -> Option<&U> {
        self.extensions.get(&TypeId::of::<U>()).and_then(|v| v.downcast_ref::<U>())
    }

    pub fn get_mut<U: Any + Send + Sync>(&mut self) -> Option<&mut U> {
        self.extensions
            .get_mut(&TypeId::of::<U>())
            .and_then(|v| v.downcast_mut::<U>())
    }

    pub fn take<U: Any + Send + Sync>(&mut self) -> Option<U> {
        self.extensions.remove(&TypeId::of::<U>()).and_then(|v| v.downcast::<U>().ok()).map(|v| *v)
    }

    pub fn remove<U: Any + Send + Sync>(&mut self) -> Option<U> {
        self.extensions.remove(&TypeId::of::<U>()).and_then(|v| v.downcast::<U>().ok()).map(|v| *v)
    }

    pub fn replace<U: Any + Send + Sync>(&mut self, value: U) -> Option<U> {
        let old =self.extensions.insert(TypeId::of::<U>(), Box::new(value));
        old.and_then(|v| v.downcast::<U>().ok()).map(|v| *v)
    }

    pub fn contains<U: Any + Send + Sync>(&self) -> bool {
        self.extensions.contains_key(&TypeId::of::<U>())
    }

    pub fn clear(&mut self) {
        self.extensions.clear();
    }

    pub fn unwrap(self) -> (T, HashMap<TypeId, Box<dyn Any + Send + Sync>>, HashMap<String, String>) {
        (self.data, self.extensions, self.attributes)
    }
}

impl<T> AsRef<T> for Extendable<T> {
    fn as_ref(&self) -> &T {
        &self.data
    }
}

impl<T> AsMut<T> for Extendable<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T> Deref for Extendable<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for Extendable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}


pub async fn encode_map_as_json(map: &HashMap<String, String>) -> Result<String> {
    let json = serde_json::to_string(map)?;
    Ok(json)
}

pub async fn decode_json_as_map(json: &str) -> Result<HashMap<String, String>> {
    let map: HashMap<String, String> = serde_json::from_str(json)?;
    Ok(map)
}