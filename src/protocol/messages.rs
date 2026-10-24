//! Wire messages of the RQS transfer protocol.
//!
//! Every message is a [`postcard`]-encoded frame written with
//! [`write_msg`](super::codec::write_msg). A transfer runs on one bidirectional
//! *control stream* opened by the dialing peer:
//!
//! ```text
//! dialer   ── Hello::Push | Hello::Pull { token } | Hello::Pair { name, addr } ──▶
//! sender   ── TransferOffer ──────────────────────────────────────────────────▶
//! receiver ◀─ TransferDecision::Accept { start_chunk_index } | Reject { reason } ─
//! sender   ── one unidirectional stream per chunk: ChunkHeader + payload ─────▶
//! receiver ◀─ TransferResult::Verified | Failed { reason } ───────────────────
//! ```
//!
//! [`Hello::Pair`] is answered with a single [`PairResponse`] instead.

use iroh::EndpointAddr;
use rust_i18n::t;
use serde::{Deserialize, Serialize};

/// Default chunk size used by senders (1 MiB).
pub const DEFAULT_CHUNK_SIZE: u32 = 1024 * 1024;
/// Largest chunk size accepted from a remote peer (16 MiB).
pub const MAX_CHUNK_SIZE: u32 = 16 * 1024 * 1024;
/// Largest accepted metadata frame (offer, chunk header, …) in bytes.
pub const MAX_FRAME_LEN: u32 = 64 * 1024;
/// Largest accepted file name in bytes (the limit of most file systems).
pub const MAX_FILE_NAME_LEN: usize = 255;
/// Largest device name in bytes, used for discovery and pairing.
pub const MAX_DEVICE_NAME_LEN: usize = 64;
/// Number of chunk streams in flight at the same time.
pub const CHUNK_WINDOW: usize = 8;

/// Random token identifying a file shared through a TICKET ("pull" mode).
pub type ShareToken = [u8; 16];

/// First frame on a control stream, sent by the peer that opened it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Hello {
    /// The dialing peer is about to send a file.
    Push,
    /// The dialing peer wants to download the file shared under `token`.
    Pull {
        /// Token taken from the share TICKET.
        token: ShareToken,
    },
    /// The dialing peer asks to become a mutually trusted device.
    Pair {
        /// Device name shown to the user who confirms the request.
        name: String,
        /// Current address of the requesting peer, stored by the other side.
        addr: EndpointAddr,
    },
}

/// Answer to [`Hello::Pair`], sent after the user made a decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    /// Whether the user accepted the pairing request.
    pub accepted: bool,
    /// Device name of the responding peer.
    pub name: String,
    /// Address of the responding peer; present only when accepted.
    pub addr: Option<EndpointAddr>,
}

/// Description of the file the sender is about to transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOffer {
    /// File name proposed by the sender (sanitized by the receiver).
    pub file_name: String,
    /// Total file size in bytes.
    pub file_size: u64,
    /// Size of every chunk except possibly the last one.
    pub chunk_size: u32,
    /// BLAKE3 hash of the whole file.
    pub file_hash: [u8; 32],
}

impl TransferOffer {
    /// Rejects offers that could cause a panic or an excessive allocation.
    ///
    /// # Errors
    ///
    /// Returns an error when the chunk size is zero or above [`MAX_CHUNK_SIZE`],
    /// or when the file name is empty or longer than [`MAX_FILE_NAME_LEN`].
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.chunk_size == 0 || self.chunk_size > MAX_CHUNK_SIZE {
            anyhow::bail!(t!("protocol.bad_chunk_size", size = self.chunk_size));
        }
        if self.file_name.is_empty() || self.file_name.len() > MAX_FILE_NAME_LEN {
            anyhow::bail!(t!("protocol.bad_file_name_len"));
        }
        Ok(())
    }

    /// Number of chunks needed to transfer the whole file.
    pub fn total_chunks(&self) -> u64 {
        self.file_size.div_ceil(self.chunk_size as u64)
    }

    /// Expected length of the chunk at `index`; the last chunk may be shorter.
    ///
    /// Returns `None` when `index` is past the end of the file.
    pub fn chunk_len(&self, index: u64) -> Option<u32> {
        let start = index.checked_mul(self.chunk_size as u64)?;
        if start >= self.file_size {
            return None;
        }
        Some((self.file_size - start).min(self.chunk_size as u64) as u32)
    }
}

/// Receiver's answer to a [`TransferOffer`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferDecision {
    /// Send chunks starting at `start_chunk_index` (non-zero when resuming).
    Accept {
        /// Index of the first chunk the receiver still needs.
        start_chunk_index: u64,
    },
    /// The offer was declined.
    Reject {
        /// Human readable reason shown to the sender.
        reason: String,
    },
}

/// Header preceding the payload on every unidirectional chunk stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkHeader {
    /// Zero-based chunk index.
    pub chunk_index: u64,
    /// Payload length in bytes.
    pub payload_len: u32,
    /// BLAKE3 hash of the payload.
    pub chunk_hash: [u8; 32],
}

/// Receiver's final verdict after verifying the hash of the whole file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferResult {
    /// The file was saved and its hash matches the offer.
    Verified,
    /// Verification failed; the partial file was discarded.
    Failed {
        /// Human readable failure reason.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(size: u64, chunk: u32) -> TransferOffer {
        TransferOffer {
            file_name: "a.bin".into(),
            file_size: size,
            chunk_size: chunk,
            file_hash: [0; 32],
        }
    }

    #[test]
    fn chunk_math() {
        let o = offer(2 * 1024 * 1024 + 5, DEFAULT_CHUNK_SIZE);
        assert_eq!(o.total_chunks(), 3);
        assert_eq!(o.chunk_len(0), Some(DEFAULT_CHUNK_SIZE));
        assert_eq!(o.chunk_len(2), Some(5));
        assert_eq!(o.chunk_len(3), None);
        assert_eq!(offer(0, DEFAULT_CHUNK_SIZE).total_chunks(), 0);
    }

    #[test]
    fn rejects_malicious_offers() {
        assert!(offer(10, 0).validate().is_err());
        assert!(offer(10, MAX_CHUNK_SIZE + 1).validate().is_err());
        let mut o = offer(10, DEFAULT_CHUNK_SIZE);
        o.file_name = "x".repeat(300);
        assert!(o.validate().is_err());
        assert!(offer(10, DEFAULT_CHUNK_SIZE).validate().is_ok());
    }
}
