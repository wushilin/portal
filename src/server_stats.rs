use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use lazy_static::lazy_static;
use tracing::{debug};

lazy_static! {
    static ref SERVER_STATS: Arc<ServerStats> = Arc::new(ServerStats::new());
}

struct ServerStats {
    total_connections: AtomicUsize,
    total_streams: AtomicUsize,
    active_connections: AtomicUsize,
    active_streams: AtomicUsize,
    total_upstream_connections: AtomicUsize,
    active_upstream_connections: AtomicUsize,
    sent_bytes: Arc<AtomicUsize>,
    received_bytes: Arc<AtomicUsize>,
}

pub struct ServerStatsClone {
    pub total_connections: usize,
    pub total_streams: usize,
    pub active_connections: usize,
    pub active_streams: usize,
    pub total_upstream_connections: usize,
    pub active_upstream_connections: usize,
    pub sent_bytes: usize,
    pub received_bytes: usize,
}

impl ServerStats {
    pub(crate) fn new() -> Self {
        Self { 
            total_connections: AtomicUsize::new(0),
            total_streams: AtomicUsize::new(0),
            active_connections: AtomicUsize::new(0),
            active_streams: AtomicUsize::new(0),
            total_upstream_connections: AtomicUsize::new(0),
            active_upstream_connections: AtomicUsize::new(0),
            sent_bytes: Arc::new(AtomicUsize::new(0)),
            received_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

pub fn get_sent_bytes_counter() -> Arc<AtomicUsize> {
    Arc::clone(&SERVER_STATS.sent_bytes)
}

pub fn get_received_bytes_counter() -> Arc<AtomicUsize> {
    Arc::clone(&SERVER_STATS.received_bytes)
}

fn increment_total_connections() {
    debug!("Increasing total connections");
    SERVER_STATS.total_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn increment_active_connections() {
    debug!("Increasing active connections");
    increment_total_connections();
    SERVER_STATS.active_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active_connections() {
    debug!("Decreasing active connections");
    SERVER_STATS.active_connections.fetch_sub(1, Ordering::Relaxed);
}

fn increment_total_streams() {
    debug!("Increasing total streams");
    SERVER_STATS.total_streams.fetch_add(1, Ordering::Relaxed);
}

pub fn increment_active_streams() {
    debug!("Increasing active streams");
    increment_total_streams();
    SERVER_STATS.active_streams.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active_streams() {
    debug!("Decreasing active streams");
    SERVER_STATS.active_streams.fetch_sub(1, Ordering::Relaxed);
}

fn increment_total_upstream_connections() {
    debug!("Increasing total upstream connections");
    SERVER_STATS.total_upstream_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn increment_active_upstream_connections() {
    debug!("Increasing active upstream connections");
    increment_total_upstream_connections();
    SERVER_STATS.active_upstream_connections.fetch_add(1, Ordering::Relaxed);
}

pub fn decrement_active_upstream_connections() {
    debug!("Decreasing active upstream connections");
    SERVER_STATS.active_upstream_connections.fetch_sub(1, Ordering::Relaxed);
}

impl ServerStatsClone {
    pub fn get() -> Self {
        Self {
            total_connections: SERVER_STATS.total_connections.load(Ordering::Relaxed),
            total_streams: SERVER_STATS.total_streams.load(Ordering::Relaxed),
            active_connections: SERVER_STATS.active_connections.load(Ordering::Relaxed),
            active_streams: SERVER_STATS.active_streams.load(Ordering::Relaxed),
            total_upstream_connections: SERVER_STATS.total_upstream_connections.load(Ordering::Relaxed),
            active_upstream_connections: SERVER_STATS.active_upstream_connections.load(Ordering::Relaxed),
            sent_bytes: SERVER_STATS.sent_bytes.load(Ordering::Relaxed),
            received_bytes: SERVER_STATS.received_bytes.load(Ordering::Relaxed),
        }
    }
}