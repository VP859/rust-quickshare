//! Wire messages of the Ferry transfer protocol.
//!
//! Every message is a [`postcard`]-encoded frame written with
//! [`write_msg`](super::codec::write_msg). A transfer runs on one bidirectional
//! *control stream* opened by the dialing peer:
//!
//! ```text
//! dialer   ── Hello::Push | Hello::Pull { token } | Hello::Pair { … } | Hello::Text { … }
//!             | Hello::Batch(BatchOffer) ─▶
//! sender   ── TransferOffer ──────────────────────────────────────────────────▶
//! receiver ◀─ TransferDecision::Accept { start_chunk_index } | Reject { reason } ─
//! sender   ── one unidirectional stream per chunk: ChunkHeader + payload ─────▶
//! receiver ◀─ TransferResult::Progress { received }* (while data arrives) ─────
//! receiver ◀─ TransferResult::Verified | Failed { reason } ───────────────────
//! ```
//!
//! Pairing exchanges four messages on the control stream:
//!
//! ```text
//! requester ── Hello::Pair { name, addr, commitment } ─▶
//! responder ◀─ PairChallenge { nonce } ─────────────────
//! requester ── PairReveal { nonce } ───────────────────▶
//! responder ◀─ PairResponse { accepted, name, addr } ───  (after the user decided)
//! ```
//!
//! [`Hello::Text`] is answered with a single [`TextDecision`] and
//! [`Hello::Batch`] with a single
//! [`BatchDecision`] instead. A batch announces several files that follow as
//! [`Hello::Push`] streams on the same connection, so the receiver decides
//! once for all of them.

use iroh::EndpointAddr;
use rust_i18n::t;

use crate::error::ProtocolViolation;
use serde::{Deserialize, Serialize};

/// Chunk size of files up to 1 GiB (1 MiB); see [`chunk_size_for`].
pub const DEFAULT_CHUNK_SIZE: u32 = 1024 * 1024;
/// Smallest chunk size a sender picks (1 MiB). Smaller chunks cost more CPU
/// per byte (a stream, a header and a hash each) – about 30% less throughput
/// with 256 KiB – and progress no longer depends on the chunk size.
pub const MIN_SEND_CHUNK_SIZE: u32 = DEFAULT_CHUNK_SIZE;
/// Largest chunk size a sender picks (8 MiB).
pub const MAX_SEND_CHUNK_SIZE: u32 = 8 * 1024 * 1024;
/// Smallest chunk size accepted from a remote peer (16 KiB): tiny chunks would
/// turn a large file into millions of streams.
pub const MIN_CHUNK_SIZE: u32 = 16 * 1024;
/// Largest chunk size accepted from a remote peer (16 MiB).
pub const MAX_CHUNK_SIZE: u32 = 16 * 1024 * 1024;
/// Chunk data kept in flight per transfer (8 MiB): enough for about
/// 650 Mbit/s at 100 ms round trip, and the bound of the receiver's buffers.
/// More only adds work on fast local links, where the CPU is the limit.
pub const IN_FLIGHT_BYTES: u64 = 8 * 1024 * 1024;
/// Largest accepted metadata frame (offer, chunk header, …) in bytes.
pub const MAX_FRAME_LEN: u32 = 64 * 1024;
/// Largest accepted file name in bytes (the limit of most file systems).
pub const MAX_FILE_NAME_LEN: usize = 255;
/// Largest device name in bytes, used for discovery and pairing.
pub const MAX_DEVICE_NAME_LEN: usize = 64;
/// Largest text sent with [`Hello::Text`], in bytes; it fits in one frame.
pub const MAX_TEXT_LEN: usize = 32 * 1024;
/// Largest number of file names carried by a [`BatchOffer`] for display.
pub const MAX_BATCH_NAMES: usize = 20;
/// Deepest folder structure accepted in [`TransferOffer::folder`].
pub const MAX_FOLDER_DEPTH: usize = 16;
/// Chunk size a sender uses for a file of `file_size` bytes.
///
/// Aims at about 1024 chunks per file, rounded up to a power of two between
/// [`MIN_SEND_CHUNK_SIZE`] and [`MAX_SEND_CHUNK_SIZE`]: 1 MiB up to 1 GiB, then
/// growing to 8 MiB for files above 4 GiB. Large files thus need fewer streams,
/// headers and hashes, and the number of chunks a receiver handles stays
/// bounded; a resume repeats at most one chunk. The size depends only on the
/// file size, so an interrupted transfer resumes with the same chunks.
///
/// # Examples
///
/// ```
/// use ferry::protocol::messages::chunk_size_for;
///
/// assert_eq!(chunk_size_for(5 * 1024 * 1024), 1024 * 1024);
/// assert_eq!(chunk_size_for(3 * 1024 * 1024 * 1024), 4 * 1024 * 1024);
/// assert_eq!(chunk_size_for(100 * 1024 * 1024 * 1024), 8 * 1024 * 1024);
/// ```
pub fn chunk_size_for(file_size: u64) -> u32 {
    /// Number of chunks the size aims at.
    const TARGET_CHUNKS: u64 = 1024;
    let wanted = file_size
        .div_ceil(TARGET_CHUNKS)
        .clamp(MIN_SEND_CHUNK_SIZE.into(), MAX_SEND_CHUNK_SIZE.into());
    // `wanted` ≤ MAX_SEND_CHUNK_SIZE, a power of two, so neither step overflows.
    u32::try_from(wanted.next_power_of_two()).unwrap_or(MAX_SEND_CHUNK_SIZE)
}

/// Number of chunk streams in flight for chunks of `chunk_size` bytes, so that
/// about [`IN_FLIGHT_BYTES`] are on the way: 8 chunks of 1 MiB, 4 of 2 MiB or
/// larger (never fewer, so the link does not idle between chunks), up to 64
/// of a peer's small chunks. Sender and receiver compute the same value.
pub fn chunk_window(chunk_size: u32) -> usize {
    let window = IN_FLIGHT_BYTES / u64::from(chunk_size.max(1));
    usize::try_from(window.clamp(4, 64)).unwrap_or(4)
}

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
        /// Commitment to the requester's secret nonce, revealed later in
        /// [`PairReveal`] (see [`crate::pairing`]).
        commitment: [u8; 32],
    },
    /// The dialing peer sends a short text (e.g. a link) for the clipboard.
    ///
    /// Added after the first release as the last variant, so the encoding of
    /// the other messages is unchanged; an older peer fails to decode it and
    /// closes the stream.
    Text {
        /// The text, at most [`MAX_TEXT_LEN`] bytes.
        text: String,
    },
    /// The dialing peer is about to push several files over this connection
    /// and asks for one decision for all of them.
    ///
    /// Added after [`Hello::Text`] for the same compatibility reasons: an older
    /// peer closes the connection, and the sender then reconnects and offers
    /// every file separately.
    Batch(BatchOffer),
}

/// Announcement of several files sent together ([`Hello::Batch`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BatchOffer {
    /// Number of files that follow.
    pub count: u32,
    /// Sum of their sizes in bytes.
    pub total_size: u64,
    /// Names of the first files, at most [`MAX_BATCH_NAMES`], for display only.
    pub names: Vec<String>,
}

impl BatchOffer {
    /// Rejects announcements that are empty or carry too many or too long names.
    ///
    /// # Errors
    ///
    /// Returns an error when `count` is zero, or when `names` has more than
    /// [`MAX_BATCH_NAMES`] entries or an entry longer than [`MAX_FILE_NAME_LEN`].
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.count == 0
            || self.names.len() > MAX_BATCH_NAMES
            || self.names.iter().any(|n| n.len() > MAX_FILE_NAME_LEN)
        {
            return Err(ProtocolViolation(t!("protocol.bad_batch").into_owned()).into());
        }
        Ok(())
    }
}

/// Answer to [`Hello::Batch`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BatchDecision {
    /// All announced files are accepted; they will not be asked about again.
    Accepted,
    /// The receiver will decide about every file separately.
    AskEach,
    /// The whole batch was declined; the sender should not push the files.
    Rejected {
        /// Human readable reason shown to the sender.
        reason: String,
    },
}

/// Answer to [`Hello::Text`], sent after the user made a decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextDecision {
    /// The text was accepted and copied to the clipboard.
    Accepted,
    /// The text was declined.
    Rejected {
        /// Human readable reason shown to the sender.
        reason: String,
    },
}

/// Random nonce of the responding peer, sent right after [`Hello::Pair`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairChallenge {
    /// 32 random bytes, chosen after the requester committed to its own nonce.
    pub nonce: [u8; 32],
}

/// The requester's nonce, sent after [`PairChallenge`]; it must match the
/// commitment from [`Hello::Pair`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairReveal {
    /// The nonce the requester committed to.
    pub nonce: [u8; 32],
}

/// Final answer to [`Hello::Pair`], sent after the user made a decision.
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
    /// Folders the file belongs in, relative to the download folder (e.g.
    /// `["Photos", "2024"]` when a folder is sent); empty for a single file.
    /// Every component is sanitized by the receiver like a file name.
    pub folder: Vec<String>,
}

impl TransferOffer {
    /// File name with its folders (`Photos/2024/a.jpg`), for display only.
    pub fn display_name(&self) -> String {
        self.folder
            .iter()
            .chain(std::iter::once(&self.file_name))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Rejects offers that could cause a panic or an excessive allocation.
    ///
    /// # Errors
    ///
    /// Returns an error when the chunk size is zero or above [`MAX_CHUNK_SIZE`],
    /// when the file name or a folder name is empty or longer than
    /// [`MAX_FILE_NAME_LEN`], or when there are more than [`MAX_FOLDER_DEPTH`]
    /// folders.
    pub fn validate(&self) -> anyhow::Result<()> {
        if !(MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE).contains(&self.chunk_size) {
            return Err(ProtocolViolation(
                t!("protocol.bad_chunk_size", size = self.chunk_size).into_owned(),
            )
            .into());
        }
        let bad_name = |name: &String| name.is_empty() || name.len() > MAX_FILE_NAME_LEN;
        if bad_name(&self.file_name) || self.folder.iter().any(bad_name) {
            return Err(ProtocolViolation(t!("protocol.bad_file_name_len").into_owned()).into());
        }
        if self.folder.len() > MAX_FOLDER_DEPTH {
            return Err(ProtocolViolation(
                t!("protocol.too_deep", max = MAX_FOLDER_DEPTH).into_owned(),
            )
            .into());
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

/// Messages from the receiver on the control stream after it accepted a file:
/// any number of [`TransferResult::Progress`] updates, then the final verdict
/// ([`TransferResult::Verified`] or [`TransferResult::Failed`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferResult {
    /// The file was saved and its hash matches the offer.
    Verified,
    /// Verification failed; the partial file was discarded.
    Failed {
        /// Human readable failure reason.
        reason: String,
    },
    /// Bytes of the file the receiver has so far (including a resumed
    /// prefix), sent a few times per second while data arrives. The sender
    /// shows it as its progress: its own count only says what left its
    /// buffers, which on a slow link is far ahead of what arrived.
    Progress {
        /// Bytes received so far.
        received: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_sizes_follow_the_file_size() {
        assert_eq!(chunk_size_for(0), MIN_SEND_CHUNK_SIZE);
        assert_eq!(chunk_size_for(10), MIN_SEND_CHUNK_SIZE);
        assert_eq!(chunk_size_for(1024 * 1024 * 1024), MIN_SEND_CHUNK_SIZE);
        assert_eq!(chunk_size_for(1024 * 1024 * 1024 + 1), 2 * 1024 * 1024);
        assert_eq!(chunk_size_for(4 * 1024 * 1024 * 1024), 4 * 1024 * 1024);
        assert_eq!(chunk_size_for(5 * 1024 * 1024 * 1024), MAX_SEND_CHUNK_SIZE);
        assert_eq!(chunk_size_for(u64::MAX), MAX_SEND_CHUNK_SIZE);
        let mut previous = 0;
        for shift in 0..48 {
            let size = chunk_size_for(1u64 << shift);
            assert!(size.is_power_of_two() && size >= previous);
            assert!((MIN_SEND_CHUNK_SIZE..=MAX_SEND_CHUNK_SIZE).contains(&size));
            previous = size;
        }
    }

    #[test]
    fn window_bounds_memory() {
        assert_eq!(chunk_window(MIN_CHUNK_SIZE), 64);
        assert_eq!(chunk_window(1024 * 1024), 8);
        assert_eq!(chunk_window(MAX_SEND_CHUNK_SIZE), 4);
        // A peer's largest or smallest chunks keep the receiver within 64 MiB.
        assert!(chunk_window(MAX_CHUNK_SIZE) as u64 * u64::from(MAX_CHUNK_SIZE) <= 64 << 20);
        assert!(chunk_window(MIN_CHUNK_SIZE) as u64 * u64::from(MIN_CHUNK_SIZE) <= 64 << 20);
        assert_eq!(chunk_window(0), 64);
    }

    fn offer(size: u64, chunk: u32) -> TransferOffer {
        TransferOffer {
            file_name: "a.bin".into(),
            file_size: size,
            chunk_size: chunk,
            file_hash: [0; 32],
            folder: Vec::new(),
        }
    }

    #[test]
    fn chunk_math() {
        let o = offer(2 * 1024 * 1024 + 5, DEFAULT_CHUNK_SIZE);
        assert!(
            offer(10, MIN_CHUNK_SIZE - 1).validate().is_err(),
            "tiny chunks"
        );
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
