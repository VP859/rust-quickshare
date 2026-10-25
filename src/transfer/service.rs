//! Node service shared by every user interface (daemon, TUI, GUI).
//!
//! [`serve`] accepts connections and dispatches incoming control streams:
//! file offers, downloads of files shared by TICKET, pairing requests and
//! texts for the clipboard. [`send_files`], [`pull_shared`] and [`send_text`]
//! are the outgoing counterparts.
//!
//! An interface only implements [`TransferHandler`] (decisions, shared files,
//! pairing confirmation, received texts) and renders the [`TransferEvent`]s it
//! receives.

use anyhow::{Context, Result};
use iroh::EndpointId;
use iroh::endpoint::Connection;
use rust_i18n::t;
use serde::Serialize;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::Channel;
use super::receiver::{read_offer, receive_on_channel, reject};
use super::sender::{PreparedFile, SendOutcome, prepare_file, push_file, send_on_channel};
use crate::node::{NodeEngine, NodeTicket};
use crate::pairing::{PAIR_DECISION_TIMEOUT, PairRequest, clean_device_name, pairing_code};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    Hello, MAX_TEXT_LEN, PairResponse, ShareToken, TextDecision, TransferOffer,
};

/// Application close code sent when a download asks for an unknown share token.
const CLOSE_UNKNOWN_SHARE: u32 = 2;
/// Upper bound for a handler to decide about a received text.
const TEXT_DECISION_TIMEOUT: Duration = Duration::from_secs(150);

/// Direction of a transfer as seen from this device.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
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
}

/// Snapshot of one transfer, serialized as-is to the GUI frontend.
#[derive(Debug, Clone, Serialize)]
pub struct TransferEvent {
    /// Unique transfer ID (UUID v4).
    pub id: String,
    /// Node ID of the other device.
    pub peer: String,
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

/// Interface-specific behaviour plugged into [`serve`].
pub trait TransferHandler: Send + Sync + 'static {
    /// Decides about an offer from `peer`: Auto-Accept, ask the user or reject.
    fn decide(
        &self,
        peer: EndpointId,
        transfer_id: &str,
        offer: &TransferOffer,
    ) -> impl Future<Output = OfferDecision> + Send;

    /// File shared under `token` for download by TICKET, if any.
    fn shared_file(&self, _token: &ShareToken) -> Option<PreparedFile> {
        None
    }

    /// Receives every state change of every transfer handled by [`serve`].
    fn on_event(&self, event: TransferEvent);

    /// Name of this device sent in pairing responses.
    fn device_name(&self) -> String {
        crate::config::Config::load()
            .map(|c| c.device_name())
            .unwrap_or_else(|_| crate::config::paths::default_device_name())
    }

    /// Handles a pairing request.
    ///
    /// Implementations ask the user (showing [`PairRequest::code`]) and on
    /// acceptance store the device, e.g. with
    /// [`Config::add_paired`](crate::config::Config::add_paired). The default
    /// rejects every request (e.g. a daemon without a terminal).
    fn confirm_pair(&self, _request: PairRequest) -> impl Future<Output = bool> + Send {
        async { false }
    }

    /// Handles a text sent by `peer` (at most [`MAX_TEXT_LEN`] bytes).
    ///
    /// Implementations accept it automatically for Auto-Accept devices or ask
    /// the user, and on acceptance copy it to the clipboard. The default
    /// declines every text.
    fn receive_text(
        &self,
        _peer: EndpointId,
        _text: String,
    ) -> impl Future<Output = TextDecision> + Send {
        async {
            TextDecision::Rejected {
                reason: t!("service.text_unsupported").into_owned(),
            }
        }
    }
}

/// Emits the successive states of a single transfer.
pub struct Tracker<'a> {
    base: TransferEvent,
    sink: EventSink<'a>,
}

impl<'a> Tracker<'a> {
    /// Starts tracking a transfer with a fresh ID.
    pub fn new(
        peer: EndpointId,
        file_name: &str,
        total: u64,
        direction: Direction,
        sink: EventSink<'a>,
    ) -> Self {
        Self {
            base: TransferEvent {
                id: uuid::Uuid::new_v4().to_string(),
                peer: peer.to_string(),
                file_name: file_name.to_string(),
                direction,
                total,
                done: 0,
                status: TransferStatus::Preparing,
            },
            sink,
        }
    }

    /// ID of the tracked transfer.
    pub fn id(&self) -> &str {
        &self.base.id
    }

    /// Emits an event with the given progress and status.
    pub fn emit(&self, done: u64, status: TransferStatus) {
        let mut ev = self.base.clone();
        ev.done = done;
        ev.status = status;
        (self.sink)(ev);
    }

    /// Emits a [`TransferStatus::Running`] event.
    pub fn progress(&self, done: u64) {
        self.emit(done, TransferStatus::Running);
    }

    fn finish<T>(&self, result: &Result<T>, path: impl FnOnce(&T) -> Option<String>) {
        match result {
            Ok(v) => self.emit(self.base.total, TransferStatus::Completed { path: path(v) }),
            Err(e) => self.emit(
                0,
                TransferStatus::Failed {
                    error: format!("{e:#}"),
                },
            ),
        }
    }
}

/// Main node loop: accepts connections until the endpoint is closed.
///
/// Every connection is handled in its own task; it may carry several
/// consecutive transfers and a failure of one does not affect the others.
pub async fn serve<H: TransferHandler>(engine: NodeEngine, handler: Arc<H>) {
    loop {
        let conn = match engine.accept().await {
            Ok(Some(conn)) => conn,
            Ok(None) => {
                info!("Endpoint closed, stopping the accept loop");
                return;
            }
            Err(e) => {
                warn!("Incoming connection failed: {e:#}");
                continue;
            }
        };
        let handler = handler.clone();
        let engine = engine.clone();
        tokio::spawn(async move {
            let peer = conn.remote_id();
            if let Err(e) = handle_connection(&engine, conn, handler).await {
                warn!("Error while handling the connection from {peer}: {e:#}");
            }
        });
    }
}

async fn handle_connection<H: TransferHandler>(
    engine: &NodeEngine,
    conn: Connection,
    handler: Arc<H>,
) -> Result<()> {
    while let Some((channel, hello)) = Channel::accept(&conn).await? {
        let result = match hello {
            Hello::Push => handle_push(&conn, channel, handler.as_ref()).await,
            Hello::Pull { token } => handle_pull(&conn, channel, &token, handler.as_ref()).await,
            Hello::Pair { name, addr } => {
                handle_pair(engine, &conn, channel, &name, addr, handler.as_ref()).await
            }
            Hello::Text { text } => handle_text(&conn, channel, text, handler.as_ref()).await,
        };
        if let Err(e) = result {
            warn!("Transfer from {} failed: {e:#}", conn.remote_id());
        }
    }
    debug!("Connection with {} finished", conn.remote_id());
    Ok(())
}

async fn handle_push<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    handler: &H,
) -> Result<()> {
    let peer = conn.remote_id();
    let offer = read_offer(&mut channel).await?;
    let sink = |e| handler.on_event(e);
    let tracker = Tracker::new(
        peer,
        &offer.file_name,
        offer.file_size,
        Direction::Download,
        &sink,
    );
    tracker.emit(0, TransferStatus::AwaitingDecision);

    match handler.decide(peer, tracker.id(), &offer).await {
        OfferDecision::Reject { reason } => {
            reject(channel, &reason).await?;
            tracker.emit(0, TransferStatus::Rejected { reason });
        }
        OfferDecision::Accept { dest_dir } => {
            let result =
                receive_on_channel(conn, channel, &offer, &dest_dir, &|d| tracker.progress(d))
                    .await;
            tracker.finish(&result, |p| Some(p.display().to_string()));
            result?;
        }
    }
    Ok(())
}

async fn handle_pull<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    token: &ShareToken,
    handler: &H,
) -> Result<()> {
    let Some(file) = handler.shared_file(token) else {
        warn!("{} requested an unknown share", conn.remote_id());
        conn.close(CLOSE_UNKNOWN_SHARE.into(), b"unknown share");
        return Ok(());
    };
    let sink = |e| handler.on_event(e);
    let tracker = Tracker::new(
        conn.remote_id(),
        &file.name,
        file.size,
        Direction::Upload,
        &sink,
    );
    let result = send_on_channel(conn, &mut channel, &file, &|d| tracker.progress(d)).await;
    report_send(&tracker, result)?;
    channel.finish().await;
    Ok(())
}

async fn handle_pair<H: TransferHandler>(
    engine: &NodeEngine,
    conn: &Connection,
    mut channel: Channel,
    name: &str,
    addr: iroh::EndpointAddr,
    handler: &H,
) -> Result<()> {
    let peer = conn.remote_id();
    let request = PairRequest {
        peer,
        name: clean_device_name(name),
        code: pairing_code(&engine.node_id(), &peer),
        addr: (addr.id == peer).then_some(addr),
    };
    info!("Pairing request from '{}' ({peer})", request.name);
    let accepted = tokio::time::timeout(PAIR_DECISION_TIMEOUT, handler.confirm_pair(request))
        .await
        .unwrap_or(false);
    let response = PairResponse {
        accepted,
        name: handler.device_name(),
        addr: accepted.then(|| engine.endpoint().addr()),
    };
    write_msg(&mut channel.send, &response).await?;
    channel.finish().await;
    Ok(())
}

async fn handle_text<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    text: String,
    handler: &H,
) -> Result<()> {
    let peer = conn.remote_id();
    let decision = if text.is_empty() || text.len() > MAX_TEXT_LEN {
        TextDecision::Rejected {
            reason: t!("service.text_too_long", max = MAX_TEXT_LEN).into_owned(),
        }
    } else {
        info!("Text ({} bytes) from {peer}", text.len());
        tokio::time::timeout(TEXT_DECISION_TIMEOUT, handler.receive_text(peer, text))
            .await
            .unwrap_or_else(|_| TextDecision::Rejected {
                reason: t!("transfer.user_rejected").into_owned(),
            })
    };
    write_msg(&mut channel.send, &decision).await?;
    channel.finish().await;
    Ok(())
}

fn report_send(tracker: &Tracker<'_>, result: Result<SendOutcome>) -> Result<()> {
    match result {
        Ok(SendOutcome::Rejected(reason)) => {
            tracker.emit(0, TransferStatus::Rejected { reason });
            Ok(())
        }
        other => {
            tracker.finish(&other, |_| None);
            other.map(|_| ())
        }
    }
}

/// Sends files to a device over one connection, one control stream per file.
///
/// A failure of one file does not stop the remaining ones.
///
/// # Errors
///
/// Fails when the device cannot be reached or when at least one file failed.
pub async fn send_files(
    engine: &NodeEngine,
    target: iroh::EndpointAddr,
    paths: &[PathBuf],
    sink: EventSink<'_>,
) -> Result<()> {
    let peer = target.id;
    let conn = engine.connect(target).await?;
    let mut failures = 0usize;
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = tokio::fs::metadata(path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let tracker = Tracker::new(peer, &name, size, Direction::Upload, sink);
        tracker.emit(0, TransferStatus::Preparing);
        let result = async {
            let file = prepare_file(path).await?;
            push_file(&conn, &file, &|d| tracker.progress(d)).await
        }
        .await;
        if report_send(&tracker, result).is_err() {
            failures += 1;
        }
    }
    conn.close(0u32.into(), b"done");
    if failures > 0 {
        anyhow::bail!(t!(
            "service.send_failed",
            failed = failures,
            total = paths.len()
        ));
    }
    Ok(())
}

/// Downloads a file shared by TICKET; this device dials and receives.
///
/// # Errors
///
/// Fails when the ticket carries no share token (a pairing ticket), the sender
/// no longer shares the file, or the transfer fails.
pub async fn pull_shared(
    engine: &NodeEngine,
    ticket: &NodeTicket,
    dest_dir: &Path,
    sink: EventSink<'_>,
) -> Result<PathBuf> {
    let token = ticket.share.context(t!("service.pairing_ticket"))?;
    let peer = ticket.addr.id;
    let conn = engine.connect(ticket.addr.clone()).await?;
    let mut channel = Channel::open(&conn, &Hello::Pull { token }).await?;
    let offer = read_offer(&mut channel)
        .await
        .context(t!("service.share_gone"))?;
    let tracker = Tracker::new(
        peer,
        &offer.file_name,
        offer.file_size,
        Direction::Download,
        sink,
    );
    let result =
        receive_on_channel(&conn, channel, &offer, dest_dir, &|d| tracker.progress(d)).await;
    tracker.finish(&result, |p| Some(p.display().to_string()));
    conn.close(0u32.into(), b"done");
    result
}

/// Sends a text (e.g. a link) that the other device copies to its clipboard
/// once accepted; waits for the decision.
///
/// # Errors
///
/// Fails when the text is empty or longer than [`MAX_TEXT_LEN`], the device
/// cannot be reached or does not answer (e.g. an older version without text
/// support).
pub async fn send_text(
    engine: &NodeEngine,
    target: iroh::EndpointAddr,
    text: &str,
) -> Result<TextDecision> {
    if text.trim().is_empty() {
        anyhow::bail!(t!("service.text_empty"));
    }
    if text.len() > MAX_TEXT_LEN {
        anyhow::bail!(t!("service.text_too_long", max = MAX_TEXT_LEN));
    }
    let conn = engine.connect(target).await?;
    let mut channel = Channel::open(
        &conn,
        &Hello::Text {
            text: text.to_string(),
        },
    )
    .await?;
    let decision: Result<TextDecision> = read_msg(&mut channel.recv)
        .await
        .context(t!("service.text_no_answer"));
    conn.close(0u32.into(), b"done");
    decision
}

/// One-line preview of a text: line breaks and control characters become
/// spaces, repeated whitespace is collapsed and the result is cut to
/// `max_chars` characters with an ellipsis.
pub fn text_preview(text: &str, max_chars: usize) -> String {
    let flat = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let mut cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Generates a random share token.
pub fn new_share_token() -> ShareToken {
    *uuid::Uuid::new_v4().as_bytes()
}
