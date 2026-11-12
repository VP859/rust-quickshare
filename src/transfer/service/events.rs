//! What the interfaces see of transfers: their direction, status, events
//! and route, and the decision about an offer.

use iroh::endpoint::Connection;
use rust_i18n::t;
use serde::Serialize;
use std::path::PathBuf;

/// Direction of a transfer as seen from this device.
#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// This device sends the file.
    Upload,
    /// This device receives the file.
    Download,
}

/// Current state of a transfer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransferStatus {
    /// Computing the BLAKE3 hash before sending.
    Preparing,
    /// Waiting for the user to accept or reject the offer.
    AwaitingDecision,
    /// Data is being transferred; see [`TransferEvent::done`].
    Running,
    /// Paused on this device ([`pause_transfer`](super::pause_transfer)); resumes where it stopped.
    Paused,
    /// Finished and verified.
    Completed {
        /// Path of the received file (downloads only).
        path: Option<String>,
    },
    /// The offer was declined.
    Rejected {
        /// Reason given by the receiver.
        reason: String,
    },
    /// The transfer failed.
    Failed {
        /// Error description.
        error: String,
    },
    /// The transfer was cancelled on this device ([`cancel_transfer`](super::cancel_transfer)). A
    /// partly received file is kept and resumed by the next transfer of the
    /// same file.
    Cancelled,
    /// The connection broke; the sender reconnects and resumes (attempt
    /// `attempt` of `of`).
    Retrying {
        /// Number of this attempt, starting at 1.
        attempt: u32,
        /// Number of attempts in total.
        of: u32,
    },
}

impl TransferStatus {
    /// Whether the transfer has ended (completed, declined, failed or cancelled).
    pub fn is_final(&self) -> bool {
        !matches!(
            self,
            Self::Preparing
                | Self::AwaitingDecision
                | Self::Running
                | Self::Paused
                | Self::Retrying { .. }
        )
    }
}

/// Error returned by a transfer stopped with [`cancel_transfer`](super::cancel_transfer).
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!("transfer.cancelled"))
    }
}

impl std::error::Error for Cancelled {}

/// Snapshot of one transfer, serialized as-is to the GUI frontend.
#[derive(Debug, Clone, Serialize)]
pub struct TransferEvent {
    /// Unique transfer ID (UUID v4).
    pub id: String,
    /// Node ID of the other device (`localsend:<IP address>` for a LocalSend app).
    pub peer: String,
    /// Name of the other device when it is not a Ferry device with a Node ID
    /// (a LocalSend app); `None` for Ferry devices, whose names the
    /// interfaces look up by `peer`.
    pub peer_name: Option<String>,
    /// Name of the transferred file.
    pub file_name: String,
    /// Upload or download.
    pub direction: Direction,
    /// Total size in bytes (0 while unknown).
    pub total: u64,
    /// Bytes transferred so far.
    pub done: u64,
    /// Current state.
    pub status: TransferStatus,
    /// How the data currently travels; `None` before the connection exists.
    pub route: Option<Route>,
}

/// Network path a transfer currently uses.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Straight between the two devices (local network or holepunched).
    Direct,
    /// Through a public relay server (internet mode only); still end-to-end
    /// encrypted, but usually slower.
    Relay,
}

impl Route {
    /// Short localized label (`direct`, `via relay`).
    pub fn label(self) -> String {
        match self {
            Self::Direct => t!("transfer.route_direct"),
            Self::Relay => t!("transfer.route_relay"),
        }
        .into_owned()
    }

    /// Path the connection currently sends on; `None` when no path is open.
    pub fn of(conn: &Connection) -> Option<Self> {
        let paths = conn.paths();
        let selected = paths.iter().find(|p| p.is_selected())?;
        Some(if selected.is_relay() {
            Self::Relay
        } else {
            Self::Direct
        })
    }
}

/// Callback receiving [`TransferEvent`]s.
pub type EventSink<'a> = &'a (dyn Fn(TransferEvent) + Send + Sync);

/// Decision about an incoming file offer.
pub enum OfferDecision {
    /// Receive the file into `dest_dir`.
    Accept {
        /// Folder where the file will be saved.
        dest_dir: PathBuf,
    },
    /// Decline the offer.
    Reject {
        /// Reason shown to the sender.
        reason: String,
    },
}
