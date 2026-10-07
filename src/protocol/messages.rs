use serde::{Deserialize, Serialize};

pub const DEFAULT_CHUNK_SIZE: u32 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOffer {
    pub file_name: String,
    pub file_size: u64,
    pub chunk_size: u32,
    pub file_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferDecision {
    Accept { start_chunk_index: u64 },
    Reject { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkHeader {
    pub chunk_index: u64,
    pub payload_len: u32,
    pub chunk_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferComplete;
