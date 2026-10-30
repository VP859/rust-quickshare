//! Node service shared by every user interface (daemon, TUI, GUI).
//!
//! [`serve`] accepts connections and dispatches incoming control streams:
//! file offers (one by one or announced together as a batch), downloads of
//! files shared by TICKET, pairing requests and texts for the clipboard.
//! [`send_files`], [`pull_shared`] and [`send_text`] are the outgoing
//! counterparts.
//!
//! An interface only implements [`TransferHandler`] (decisions about files and
//! batches, shared files, pairing confirmation, received texts) and renders the [`TransferEvent`]s it
//! receives.

use anyhow::{Context, Result};
use iroh::EndpointId;
use iroh::endpoint::Connection;
use rust_i18n::t;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::Channel;
use super::receiver::{read_offer, receive_on_channel, reject};
use super::sender::{
    PreparedFile, SendItem, SendOutcome, expand_paths, prepare_file, push_file, send_on_channel,
};
use crate::node::{NodeEngine, NodeTicket};
use crate::pairing::{PAIR_DECISION_TIMEOUT, PairRequest, answer_pairing, clean_device_name};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    BatchDecision, BatchOffer, Hello, MAX_BATCH_NAMES, MAX_FILE_NAME_LEN, MAX_TEXT_LEN,
    PairResponse, ShareToken, TextDecision, TransferOffer,
};

/// Application close code sent when a download asks for an unknown share token.
const CLOSE_UNKNOWN_SHARE: u32 = 2;
/// Upper bound for a handler to answer a question (a received text or a batch
/// of files); no answer counts as a rejection.
const QUESTION_TIMEOUT: Duration = Duration::from_secs(150);
/// Questions one device may have waiting for the user at the same time.
const MAX_OPEN_QUESTIONS_PER_PEER: usize = 2;
/// Questions all devices together may have waiting for the user. Node IDs are
/// free to generate, so this is the bound that keeps a flood of requests from
/// burying the user in dialogs.
const MAX_OPEN_QUESTIONS: usize = 8;
/// Pause after a declined or broken pairing request before the same device
/// may ask again.
const PAIR_RETRY_DELAY: Duration = Duration::from_secs(30);

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
    /// The transfer was cancelled on this device ([`cancel_transfer`]). A
    /// partly received file is kept and resumed by the next transfer of the
    /// same file.
    Cancelled,
}

impl TransferStatus {
    /// Whether the transfer has ended (completed, declined, failed or cancelled).
    pub fn is_final(&self) -> bool {
        !matches!(
            self,
            Self::Preparing | Self::AwaitingDecision | Self::Running
        )
    }
}

/// Error returned by a transfer stopped with [`cancel_transfer`].
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!("transfer.cancelled"))
    }
}

impl std::error::Error for Cancelled {}

/// Cancellation tokens of the transfers in progress, keyed by transfer ID.
static ACTIVE_TRANSFERS: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(Mutex::default);

/// Stops a running transfer of this process; returns whether it was found.
///
/// The transfer ends with [`TransferStatus::Cancelled`]. The other device sees
/// its streams end and reports the transfer as failed.
pub fn cancel_transfer(id: &str) -> bool {
    let token = ACTIVE_TRANSFERS
        .lock()
        .ok()
        .and_then(|active| active.get(id).cloned());
    token.inspect(CancellationToken::cancel).is_some()
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

    /// Decides about several files announced together ([`Hello::Batch`]).
    ///
    /// `Some(OfferDecision::Accept { .. })` accepts all announced files at once:
    /// the following offers on the same connection are received without calling
    /// [`TransferHandler::decide`], up to the announced number of files and
    /// total size. `Some(OfferDecision::Reject { .. })` declines all of them.
    /// `None` (the default) keeps asking about every file separately.
    fn decide_batch(
        &self,
        _peer: EndpointId,
        _batch: &BatchOffer,
    ) -> impl Future<Output = Option<OfferDecision>> + Send {
        async { None }
    }
}

/// Files of an accepted batch that may still arrive on a connection without
/// asking again. The sender cannot exceed what the user agreed to: neither
/// more files nor more bytes than announced.
#[derive(Debug)]
struct BatchApproval {
    /// Files that may still be accepted automatically.
    remaining_files: u32,
    /// Bytes that may still be accepted automatically.
    remaining_bytes: u64,
    /// Folder chosen when the batch was accepted.
    dest_dir: PathBuf,
}

impl BatchApproval {
    /// Uses the approval for `offer`; returns the folder to save it to, or
    /// `None` when the offer exceeds what is left of the batch.
    fn take(&mut self, offer: &TransferOffer) -> Option<PathBuf> {
        if self.remaining_files == 0 || offer.file_size > self.remaining_bytes {
            return None;
        }
        self.remaining_files -= 1;
        self.remaining_bytes -= offer.file_size;
        Some(self.dest_dir.clone())
    }
}

/// Rate limits for questions that need the user's attention (file offers,
/// batches, texts, pairing requests), shared by all connections of [`serve`].
///
/// Requests over the limit are declined without asking, so another device
/// cannot flood the screen with dialogs or queue unlimited work.
#[derive(Debug, Default)]
struct QuestionGate {
    /// Mutable state; held only for short, non-async sections.
    state: Mutex<GateState>,
}

/// State of [`QuestionGate`].
#[derive(Debug, Default)]
struct GateState {
    /// Open questions per device.
    open: HashMap<EndpointId, usize>,
    /// Open questions of all devices.
    total: usize,
    /// Devices that may not send a pairing request before the given instant.
    pair_blocked_until: HashMap<EndpointId, Instant>,
}

impl QuestionGate {
    /// Locks the state; a poisoned lock only means another task panicked while
    /// updating counters, which stay usable.
    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reserves a slot for a question from `peer`; `None` when it already has
    /// too many open questions or all devices together do. The slot is released
    /// when the permit is dropped.
    fn enter(self: &Arc<Self>, peer: EndpointId) -> Option<QuestionPermit> {
        let mut state = self.lock();
        let open = state.open.get(&peer).copied().unwrap_or(0);
        if open >= MAX_OPEN_QUESTIONS_PER_PEER || state.total >= MAX_OPEN_QUESTIONS {
            return None;
        }
        state.open.insert(peer, open + 1);
        state.total += 1;
        Some(QuestionPermit {
            gate: self.clone(),
            peer,
        })
    }

    /// Whether `peer` may send a pairing request now.
    fn pairing_allowed(&self, peer: &EndpointId) -> bool {
        let mut state = self.lock();
        let now = Instant::now();
        state.pair_blocked_until.retain(|_, until| *until > now);
        !state.pair_blocked_until.contains_key(peer)
    }

    /// Blocks pairing requests from `peer` for [`PAIR_RETRY_DELAY`].
    fn block_pairing(&self, peer: EndpointId) {
        self.lock()
            .pair_blocked_until
            .insert(peer, Instant::now() + PAIR_RETRY_DELAY);
    }
}

/// Slot of one open question in a [`QuestionGate`], released on drop.
#[derive(Debug)]
struct QuestionPermit {
    /// Gate the slot belongs to.
    gate: Arc<QuestionGate>,
    /// Device that asked the question.
    peer: EndpointId,
}

impl Drop for QuestionPermit {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        state.total = state.total.saturating_sub(1);
        if let Some(open) = state.open.get_mut(&self.peer) {
            *open = open.saturating_sub(1);
            if *open == 0 {
                state.open.remove(&self.peer);
            }
        }
    }
}

/// Reason sent back when a request is declined by the [`QuestionGate`].
fn busy_reason() -> String {
    t!("service.busy").into_owned()
}

/// Emits the successive states of a single transfer and lets it be cancelled
/// by its ID while the tracker exists.
pub struct Tracker<'a> {
    /// Event template with the fixed fields of this transfer.
    base: TransferEvent,
    /// Receiver of the events.
    sink: EventSink<'a>,
    /// Triggered by [`cancel_transfer`].
    cancel: CancellationToken,
}

impl Drop for Tracker<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_TRANSFERS.lock() {
            active.remove(&self.base.id);
        }
    }
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
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        if let Ok(mut active) = ACTIVE_TRANSFERS.lock() {
            active.insert(id.clone(), cancel.clone());
        }
        Self {
            base: TransferEvent {
                id,
                peer: peer.to_string(),
                file_name: file_name.to_string(),
                direction,
                total,
                done: 0,
                status: TransferStatus::Preparing,
            },
            sink,
            cancel,
        }
    }

    /// Runs `transfer` until it finishes or the transfer is cancelled, in
    /// which case the future is dropped (closing its streams) and the result
    /// is a [`Cancelled`] error.
    ///
    /// # Errors
    ///
    /// Returns the error of `transfer`, or [`Cancelled`].
    pub async fn cancellable<T>(&self, transfer: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(Cancelled.into()),
            result = transfer => result,
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

    /// Emits [`TransferStatus::Completed`], [`TransferStatus::Cancelled`] or
    /// [`TransferStatus::Failed`] for `result`; `path` extracts the saved file
    /// path from a success.
    fn finish<T>(&self, result: &Result<T>, path: impl FnOnce(&T) -> Option<String>) {
        match result {
            Ok(v) => self.emit(self.base.total, TransferStatus::Completed { path: path(v) }),
            Err(e) if e.is::<Cancelled>() => self.emit(0, TransferStatus::Cancelled),
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
    let gate = Arc::new(QuestionGate::default());
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
        let gate = gate.clone();
        tokio::spawn(async move {
            let peer = conn.remote_id();
            if let Err(e) = handle_connection(&engine, conn, handler, &gate).await {
                warn!("Error while handling the connection from {peer}: {e:#}");
            }
        });
    }
}

/// Serves every control stream of one incoming connection in turn.
async fn handle_connection<H: TransferHandler>(
    engine: &NodeEngine,
    conn: Connection,
    handler: Arc<H>,
    gate: &Arc<QuestionGate>,
) -> Result<()> {
    let mut approval: Option<BatchApproval> = None;
    while let Some((channel, hello)) = Channel::accept(&conn).await? {
        let result = match hello {
            Hello::Push => handle_push(&conn, channel, handler.as_ref(), gate, &mut approval).await,
            Hello::Batch(batch) => handle_batch(&conn, channel, &batch, handler.as_ref(), gate)
                .await
                .map(|granted| approval = granted),
            Hello::Pull { token } => handle_pull(&conn, channel, &token, handler.as_ref()).await,
            Hello::Pair {
                name,
                addr,
                commitment,
            } => {
                handle_pair(
                    engine,
                    &conn,
                    channel,
                    &name,
                    addr,
                    &commitment,
                    handler.as_ref(),
                    gate,
                )
                .await
            }
            Hello::Text { text } => handle_text(&conn, channel, text, handler.as_ref(), gate).await,
        };
        if let Err(e) = result {
            warn!("Transfer from {} failed: {e:#}", conn.remote_id());
        }
    }
    debug!("Connection with {} finished", conn.remote_id());
    Ok(())
}

/// Incoming file offer ([`Hello::Push`]): receives the file when it belongs to
/// an accepted batch, otherwise asks the handler first.
async fn handle_push<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    handler: &H,
    gate: &Arc<QuestionGate>,
    approval: &mut Option<BatchApproval>,
) -> Result<()> {
    let peer = conn.remote_id();
    let offer = read_offer(&mut channel).await?;
    let sink = |e| handler.on_event(e);
    let tracker = Tracker::new(
        peer,
        &offer.display_name(),
        offer.file_size,
        Direction::Download,
        &sink,
    );
    let decision = match approval.as_mut().and_then(|a| a.take(&offer)) {
        Some(dest_dir) => OfferDecision::Accept { dest_dir },
        None => match gate.enter(peer) {
            Some(_permit) => {
                tracker.emit(0, TransferStatus::AwaitingDecision);
                handler.decide(peer, tracker.id(), &offer).await
            }
            None => OfferDecision::Reject {
                reason: busy_reason(),
            },
        },
    };
    match decision {
        OfferDecision::Reject { reason } => {
            reject(channel, &reason).await?;
            tracker.emit(0, TransferStatus::Rejected { reason });
        }
        OfferDecision::Accept { dest_dir } => {
            let result = tracker
                .cancellable(receive_on_channel(conn, channel, &offer, &dest_dir, &|d| {
                    tracker.progress(d)
                }))
                .await;
            tracker.finish(&result, |p| Some(p.display().to_string()));
            ignore_cancelled(result)?;
        }
    }
    Ok(())
}

/// Download of a file shared by TICKET ([`Hello::Pull`]); unknown tokens close the connection.
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
    let result = tracker
        .cancellable(send_on_channel(conn, &mut channel, &file, &|d| {
            tracker.progress(d)
        }))
        .await;
    if result.as_ref().is_err_and(|e| e.is::<Cancelled>()) {
        // The download ends for the other side as well.
        conn.close(0u32.into(), b"cancelled");
    }
    report_send(&tracker, result)?;
    channel.finish().await;
    Ok(())
}

/// Pairing request ([`Hello::Pair`]): exchanges the nonces, asks the handler
/// and answers with [`PairResponse`].
// The three fields of `Hello::Pair` are passed separately, like the other handlers.
#[allow(clippy::too_many_arguments)]
async fn handle_pair<H: TransferHandler>(
    engine: &NodeEngine,
    conn: &Connection,
    mut channel: Channel,
    name: &str,
    addr: iroh::EndpointAddr,
    commitment: &[u8; 32],
    handler: &H,
    gate: &Arc<QuestionGate>,
) -> Result<()> {
    let peer = conn.remote_id();
    // Dropping the stream without an answer ends the request on the other side
    // with "interrupted"; nothing is shown to the user here.
    if !gate.pairing_allowed(&peer) {
        anyhow::bail!("pairing request from {peer} ignored: retry delay after a declined request");
    }
    let Some(_permit) = gate.enter(peer) else {
        anyhow::bail!("pairing request from {peer} ignored: too many open requests");
    };
    let code = match answer_pairing(&mut channel, &engine.node_id(), &peer, commitment).await {
        Ok(code) => code,
        Err(e) => {
            gate.block_pairing(peer);
            return Err(e);
        }
    };
    let request = PairRequest {
        peer,
        name: clean_device_name(name),
        code,
        addr: (addr.id == peer).then_some(addr),
    };
    info!("Pairing request from '{}' ({peer})", request.name);
    let accepted = tokio::time::timeout(PAIR_DECISION_TIMEOUT, handler.confirm_pair(request))
        .await
        .unwrap_or(false);
    if !accepted {
        gate.block_pairing(peer);
    }
    let response = PairResponse {
        accepted,
        name: handler.device_name(),
        addr: accepted.then(|| engine.endpoint().addr()),
    };
    write_msg(&mut channel.send, &response).await?;
    channel.finish().await;
    Ok(())
}

/// Incoming text ([`Hello::Text`]): validates its size, asks the handler and
/// answers with [`TextDecision`].
async fn handle_text<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    text: String,
    handler: &H,
    gate: &Arc<QuestionGate>,
) -> Result<()> {
    let peer = conn.remote_id();
    let permit = gate.enter(peer);
    let decision = if text.is_empty() || text.len() > MAX_TEXT_LEN {
        TextDecision::Rejected {
            reason: t!("service.text_too_long", max = MAX_TEXT_LEN).into_owned(),
        }
    } else if permit.is_none() {
        TextDecision::Rejected {
            reason: busy_reason(),
        }
    } else {
        info!("Text ({} bytes) from {peer}", text.len());
        tokio::time::timeout(QUESTION_TIMEOUT, handler.receive_text(peer, text))
            .await
            .unwrap_or_else(|_| TextDecision::Rejected {
                reason: t!("transfer.user_rejected").into_owned(),
            })
    };
    drop(permit);
    write_msg(&mut channel.send, &decision).await?;
    channel.finish().await;
    Ok(())
}

/// Announcement of several files ([`Hello::Batch`]): asks the handler once and
/// returns the approval for the files that follow, if they were accepted.
async fn handle_batch<H: TransferHandler>(
    conn: &Connection,
    mut channel: Channel,
    batch: &BatchOffer,
    handler: &H,
    gate: &Arc<QuestionGate>,
) -> Result<Option<BatchApproval>> {
    batch.validate()?;
    let peer = conn.remote_id();
    let Some(_permit) = gate.enter(peer) else {
        let answer = BatchDecision::Rejected {
            reason: busy_reason(),
        };
        write_msg(&mut channel.send, &answer).await?;
        channel.finish().await;
        return Ok(None);
    };
    info!(
        "{} files ({} B) announced by {peer}",
        batch.count, batch.total_size
    );
    let decision = tokio::time::timeout(QUESTION_TIMEOUT, handler.decide_batch(peer, batch))
        .await
        .unwrap_or_else(|_| {
            Some(OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            })
        });
    let (answer, approval) = match decision {
        None => (BatchDecision::AskEach, None),
        Some(OfferDecision::Accept { dest_dir }) => (
            BatchDecision::Accepted,
            Some(BatchApproval {
                remaining_files: batch.count,
                remaining_bytes: batch.total_size,
                dest_dir,
            }),
        ),
        Some(OfferDecision::Reject { reason }) => (BatchDecision::Rejected { reason }, None),
    };
    write_msg(&mut channel.send, &answer).await?;
    channel.finish().await;
    Ok(approval)
}

/// Turns a [`Cancelled`] error into success: cancelling is the user's choice,
/// not a failure.
fn ignore_cancelled<T>(result: Result<T>) -> Result<()> {
    match result {
        Err(e) if !e.is::<Cancelled>() => Err(e),
        _ => Ok(()),
    }
}

/// Emits the final event of an upload; a rejection or cancellation is not an error.
fn report_send(tracker: &Tracker<'_>, result: Result<SendOutcome>) -> Result<()> {
    match result {
        Ok(SendOutcome::Rejected(reason)) => {
            tracker.emit(0, TransferStatus::Rejected { reason });
            Ok(())
        }
        other => {
            tracker.finish(&other, |_| None);
            ignore_cancelled(other)
        }
    }
}

/// Name and size of a file about to be sent, read without hashing it.
async fn file_summary(item: &SendItem) -> (String, u64) {
    let name = item
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let display = item
        .folder
        .iter()
        .chain(std::iter::once(&name))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("/");
    let size = tokio::fs::metadata(&item.path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    (display, size)
}

/// A name for display in a [`BatchOffer`]: without control characters and
/// cut to [`MAX_FILE_NAME_LEN`] bytes.
fn batch_name(name: &str) -> String {
    let mut clean: String = name.chars().filter(|c| !c.is_control()).collect();
    while clean.len() > MAX_FILE_NAME_LEN {
        clean.pop();
    }
    if clean.is_empty() {
        "file".to_string()
    } else {
        clean
    }
}

/// Announces several files with [`Hello::Batch`] and waits for the decision.
///
/// # Errors
///
/// Fails when the stream cannot be opened or the peer does not answer, e.g.
/// an older version that does not know batches and closes the connection.
async fn announce_batch(conn: &Connection, files: &[(String, u64)]) -> Result<BatchDecision> {
    let batch = BatchOffer {
        count: u32::try_from(files.len()).unwrap_or(u32::MAX),
        total_size: files.iter().map(|(_, size)| size).sum(),
        names: files
            .iter()
            .take(MAX_BATCH_NAMES)
            .map(|(name, _)| batch_name(name))
            .collect(),
    };
    let mut channel = Channel::open(conn, &Hello::Batch(batch)).await?;
    read_msg(&mut channel.recv).await
}

/// Sends files and folders to a device over one connection, one control
/// stream per file.
///
/// Folders are sent with their structure ([`expand_paths`]). Several files
/// are first announced together, so the receiver confirms them once. When the receiver declines the batch nothing is sent and every file
/// is reported as rejected. A receiver that does not support batches gets a
/// new connection and decides about every file. A failure of one file does
/// not stop the remaining ones.
///
/// # Errors
///
/// Fails when a folder cannot be read or exceeds the limits, the device
/// cannot be reached, or at least one file failed.
pub async fn send_files(
    engine: &NodeEngine,
    target: iroh::EndpointAddr,
    paths: &[PathBuf],
    sink: EventSink<'_>,
) -> Result<()> {
    let peer = target.id;
    let owned = paths.to_vec();
    let items = tokio::task::spawn_blocking(move || expand_paths(&owned))
        .await
        .context("folder scan interrupted")??;
    let mut conn = engine.connect(target.clone()).await?;
    let mut files = Vec::with_capacity(items.len());
    for item in &items {
        files.push(file_summary(item).await);
    }
    if files.len() > 1 {
        match announce_batch(&conn, &files).await {
            Ok(BatchDecision::Rejected { reason }) => {
                for (name, size) in &files {
                    Tracker::new(peer, name, *size, Direction::Upload, sink).emit(
                        0,
                        TransferStatus::Rejected {
                            reason: reason.clone(),
                        },
                    );
                }
                conn.close(0u32.into(), b"rejected");
                return Ok(());
            }
            Ok(BatchDecision::Accepted | BatchDecision::AskEach) => {}
            Err(e) => {
                debug!("Batch not supported by {peer} ({e:#}), offering files one by one");
                conn = engine.connect(target).await?;
            }
        }
    }
    let mut failures = 0usize;
    for (item, (name, size)) in items.iter().zip(&files) {
        let tracker = Tracker::new(peer, name, *size, Direction::Upload, sink);
        tracker.emit(0, TransferStatus::Preparing);
        let result = tracker
            .cancellable(async {
                let mut file = prepare_file(&item.path).await?;
                file.folder.clone_from(&item.folder);
                push_file(&conn, &file, &|d| tracker.progress(d)).await
            })
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
            total = items.len()
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
    let token = ticket.share.with_context(|| t!("service.pairing_ticket"))?;
    let peer = ticket.addr.id;
    let conn = engine.connect(ticket.addr.clone()).await?;
    let mut channel = Channel::open(&conn, &Hello::Pull { token }).await?;
    let offer = read_offer(&mut channel)
        .await
        .with_context(|| t!("service.share_gone"))?;
    let tracker = Tracker::new(
        peer,
        &offer.file_name,
        offer.file_size,
        Direction::Download,
        sink,
    );
    let result = tracker
        .cancellable(receive_on_channel(&conn, channel, &offer, dest_dir, &|d| {
            tracker.progress(d)
        }))
        .await;
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
        .with_context(|| t!("service.text_no_answer"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(size: u64) -> TransferOffer {
        TransferOffer {
            file_name: "a.jpg".into(),
            file_size: size,
            chunk_size: 1024,
            file_hash: [0; 32],
            folder: Vec::new(),
        }
    }

    #[test]
    fn question_gate_limits_open_questions() {
        let gate = Arc::new(QuestionGate::default());
        let a = iroh::SecretKey::generate().public();
        let first = gate.enter(a).unwrap();
        let _second = gate.enter(a).unwrap();
        assert!(gate.enter(a).is_none(), "per-device limit");
        drop(first);
        assert!(gate.enter(a).is_some(), "slot released on drop");

        let others: Vec<_> = (0..MAX_OPEN_QUESTIONS)
            .filter_map(|_| gate.enter(iroh::SecretKey::generate().public()))
            .collect();
        assert_eq!(others.len(), MAX_OPEN_QUESTIONS - 1, "global limit");
        drop(others);
        assert_eq!(gate.lock().total, 1);
    }

    #[test]
    fn question_gate_delays_repeated_pairing() {
        let gate = QuestionGate::default();
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        assert!(gate.pairing_allowed(&a));
        gate.block_pairing(a);
        assert!(!gate.pairing_allowed(&a));
        assert!(gate.pairing_allowed(&b));
    }

    #[test]
    fn batch_approval_never_exceeds_the_announcement() {
        let mut approval = BatchApproval {
            remaining_files: 2,
            remaining_bytes: 100,
            dest_dir: PathBuf::from("/tmp"),
        };
        assert!(
            approval.take(&offer(101)).is_none(),
            "more bytes than announced"
        );
        assert!(approval.take(&offer(60)).is_some());
        assert!(
            approval.take(&offer(41)).is_none(),
            "over the remaining bytes"
        );
        assert!(approval.take(&offer(40)).is_some());
        assert!(
            approval.take(&offer(0)).is_none(),
            "more files than announced"
        );
    }
}
