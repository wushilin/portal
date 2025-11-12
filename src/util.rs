use anyhow::Result;
use lazy_static::lazy_static;
use tokio::time::MissedTickBehavior;
use std::fmt::{self, Display};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
lazy_static! {
    static ref NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(0);
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

pub async fn run_pipe<R1, W1, R2, W2>(stream1: (R1, W1), stream2: (R2, W2), r1_to_w2: Arc<AtomicUsize>, r2_to_w1: Arc<AtomicUsize>) -> Result<(usize, usize)>
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
    let join_result = tokio::try_join!(
        async_copy1.copy(r1_to_w2),
        async_copy2.copy(r2_to_w1),
    );
    if join_result.is_err() {
        return Err(join_result.unwrap_err());
    }
    return Ok(join_result.unwrap());
}

fn next_connection_id() -> u64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
pub struct ConnectionId(u64, AtomicU64);

impl Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Connection-{}", self.0)
    }
}

impl Clone for ConnectionId {
    fn clone(&self) -> Self {
        Self(self.0, AtomicU64::new(self.1.load(Ordering::Relaxed)))
    }
}

impl ConnectionId {
    pub fn new() -> Self {
        Self(next_connection_id(), AtomicU64::new(0))
    }

    pub fn get(&self) -> u64 {
        self.0
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
pub struct StreamId(u64, u64);

impl Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Stream-{}:{}", self.0, self.1)
    }
}

impl StreamId {
    pub fn new(connection_id: u64, stream_id: u64) -> Self {
        Self(connection_id, stream_id)
    }

    pub fn connection_id(&self) -> u64 {
        self.0
    }

    pub fn stream_id(&self) -> u64 {
        self.1
    }
}

pub async fn keep_alive<R, W>(mut read: R, mut write: W, read_first: Option<bool>) -> Result<()>
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
            write.write_all(&payload).await?;
        } else {
            write.write_all(&payload).await?;
            read.read_exact(&mut buf).await?;
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

    pub async fn copy(&mut self, progress:Arc<AtomicUsize>) -> Result<usize> {
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
            progress.fetch_add(n, Ordering::Relaxed);
            total_copied += n;
        }
        Ok(total_copied)
    }
}