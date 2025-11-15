use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use lazy_static::lazy_static;
use tracing::{debug};

lazy_static! {
    static ref CLIENT_STATS: Arc<ClientStats> = Arc::new(ClientStats::new());
}

struct ClientStats {
    total_client_connections: AtomicUsize,
    active_client_connections: AtomicUsize,
    total_streams: AtomicUsize,
    active_streams: AtomicUsize,
    total_connections: AtomicUsize,
    active_connections: AtomicUsize,
    total_sent_bytes: Arc<AtomicUsize>,
    total_received_bytes: Arc<AtomicUsize>,
}

pub struct ClientStatsClone {
    pub total_connections: usize,
    pub total_streams: usize,
    pub active_connections: usize,
    pub active_streams: usize,
    pub total_sent_bytes: usize,
    pub total_received_bytes: usize,
    pub total_client_connections: usize,
    pub active_client_connections: usize,
}



impl ClientStats {
    pub(crate) fn new() -> Self {
        Self { 
            total_connections: AtomicUsize::new(0),
            total_streams: AtomicUsize::new(0),
            active_connections: AtomicUsize::new(0),
            active_streams: AtomicUsize::new(0),
            total_sent_bytes: Arc::new(AtomicUsize::new(0)),
            total_received_bytes: Arc::new(AtomicUsize::new(0)),
            active_client_connections: AtomicUsize::new(0),
            total_client_connections: AtomicUsize::new(0),
        }
    }
}

impl ClientStatsClone {
    pub fn get() -> Self {
        Self {
            total_connections: CLIENT_STATS.total_connections.load(Ordering::Relaxed),
            total_streams: CLIENT_STATS.total_streams.load(Ordering::Relaxed),
            active_connections: CLIENT_STATS.active_connections.load(Ordering::Relaxed),
            active_streams: CLIENT_STATS.active_streams.load(Ordering::Relaxed),
            total_sent_bytes: CLIENT_STATS.total_sent_bytes.load(Ordering::Relaxed),
            total_received_bytes: CLIENT_STATS.total_received_bytes.load(Ordering::Relaxed),
            total_client_connections: CLIENT_STATS.total_client_connections.load(Ordering::Relaxed),
            active_client_connections: CLIENT_STATS.active_client_connections.load(Ordering::Relaxed),
        }
    }
}

pub fn get_total_sent_bytes_counter() -> Arc<AtomicUsize> {
    Arc::clone(&CLIENT_STATS.total_sent_bytes)
}

pub fn get_total_received_bytes_counter() -> Arc<AtomicUsize> {
    Arc::clone(&CLIENT_STATS.total_received_bytes)
}

fn increment_total_connections() {
    debug!("Increasing total connections");
    CLIENT_STATS.total_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn increment_active_connections() {
    debug!("Increasing active connections");
    increment_total_connections();
    CLIENT_STATS.active_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active_connections() {
    debug!("Decreasing active connections");
    CLIENT_STATS.active_connections.fetch_sub(1, Ordering::Relaxed);
}

fn increment_total_streams() {
    debug!("Increasing total streams");
    CLIENT_STATS.total_streams.fetch_add(1, Ordering::Relaxed);
}

pub fn increment_active_streams() {
    increment_total_streams();
    debug!("Increasing active streams");
    CLIENT_STATS.active_streams.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active_streams() {
    debug!("Decreasing active streams");
    CLIENT_STATS.active_streams.fetch_sub(1, Ordering::Relaxed);
}

fn increment_total_client_connections() {
    debug!("Increasing total client connections");
    CLIENT_STATS.total_client_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn increment_active_client_connections() {
    debug!("Increasing active client connections");
    increment_total_client_connections();
    CLIENT_STATS.active_client_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active_client_connections() {
    debug!("Decreasing active client connections");
    CLIENT_STATS.active_client_connections.fetch_sub(1, Ordering::Relaxed);
}