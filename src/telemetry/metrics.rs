use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Default)]
pub struct TransferMetrics {
    pub bytes_sent: Arc<AtomicU64>,
    pub bytes_received: Arc<AtomicU64>,
    pub packets_lost: Arc<AtomicU64>,
    pub current_rtt_ms: Arc<AtomicU64>,
    pub is_direct_connection: Arc<AtomicU64>, // 1 = Direct UDP Hole Punching, 0 = Relay/DERP
}

impl TransferMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn set_rtt(&self, rtt_ms: u64) {
        self.current_rtt_ms.store(rtt_ms, Ordering::Relaxed);
    }

    pub fn set_connection_type(&self, direct: bool) {
        self.is_direct_connection
            .store(if direct { 1 } else { 0 }, Ordering::Relaxed);
    }
}
