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
use super::receiver::{check_free_space, read_offer, receive_on_channel, reject};
use super::risk;
use super::sender::{
    PreparedFile, SendItem, SendOutcome, expand_paths, prepare_file, push_file, send_on_channel,
};
use crate::history::HistoryEntry;
use crate::localsend::LocalSendOffer;
use crate::node::{NodeEngine, NodeTicket};
use crate::pairing::{PAIR_DECISION_TIMEOUT, PairRequest, answer_pairing, clean_device_name};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    BatchDecision, BatchOffer, Hello, MAX_BATCH_NAMES, MAX_FILE_NAME_LEN, MAX_TEXT_LEN,
    PairResponse, ShareToken, TextDecision, TransferDecision, TransferOffer,
};
use crate::web::BrowserOffer;

/// Application close code sent when a download asks for an unknown share token.
const CLOSE_UNKNOWN_SHARE: u32 = 2;
/// Upper bound for a handler to answer a question (a received text or a batch
/// of files); no answer counts as a rejection.
pub(crate) const QUESTION_TIMEOUT: Duration = Duration::from_secs(150);
/// Questions one device may have waiting for the user at the same time.
const MAX_OPEN_QUESTIONS_PER_PEER: usize = 2;
/// Questions all devices together may have waiting for the user. Node IDs are
/// free to generate, so this is the bound that keeps a flood of requests from
/// burying the user in dialogs.
const MAX_OPEN_QUESTIONS: usize = 8;
/// Pause after a declined or broken pairing request before the same device
/// may ask again.
const PAIR_RETRY_DELAY: Duration = Duration::from_secs(30);
/// How long an accepted file or batch stays accepted after its connection
/// broke, so the sender can resume without the user being asked again.
const RESUME_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Upper bound of remembered accepted files and batches.
const MAX_RESUMABLE: usize = 256;
/// Shortest interval between two progress events of one transfer. Data moves
/// in pieces of up to a few MiB, so a fast transfer would otherwise report
/// hundreds of times per second, and every event costs the interfaces work
/// (the GUI redraws the transfer).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Pauses before the sender tries again after the connection broke: enough for
/// a phone waking up or a network coming back, short enough to wait for.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

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
    /// Paused on this device ([`pause_transfer`]); resumes where it stopped.
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
    /// The transfer was cancelled on this device ([`cancel_transfer`]). A
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

/// Error returned by a transfer stopped with [`cancel_transfer`].
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&t!("transfer.cancelled"))
    }
}

impl std::error::Error for Cancelled {}

/// Controls of one transfer in progress.
struct ActiveTransfer {
    /// Ends the transfer ([`cancel_transfer`]).
    cancel: CancellationToken,
    /// `true` while it is paused ([`pause_transfer`]).
    pause: tokio::sync::watch::Sender<bool>,
}

/// The transfers in progress, keyed by transfer ID.
static ACTIVE_TRANSFERS: LazyLock<Mutex<HashMap<String, ActiveTransfer>>> =
    LazyLock::new(Mutex::default);

/// Pauses (`paused`) or resumes a running transfer of this process; returns
/// whether it was found.
///
/// A paused transfer reports [`TransferStatus::Paused`]; it stops reading or
/// writing data, so the other device waits too, and the connection stays
/// open. Pausing is local: the other device cannot resume it.
pub fn pause_transfer(id: &str, paused: bool) -> bool {
    ACTIVE_TRANSFERS
        .lock()
        .ok()
        .and_then(|active| active.get(id).map(|t| t.pause.send_replace(paused)))
        .is_some()
}

/// Stops a running transfer of this process; returns whether it was found.
///
/// The transfer ends with [`TransferStatus::Cancelled`]. The other device sees
/// its streams end and reports the transfer as failed.
pub fn cancel_transfer(id: &str) -> bool {
    let token = ACTIVE_TRANSFERS
        .lock()
        .ok()
        .and_then(|active| active.get(id).map(|t| t.cancel.clone()));
    token.inspect(CancellationToken::cancel).is_some()
}

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

    /// Whether a request from `peer` may reach the user at all: `pairing` is
    /// `true` for a pairing request. A device that is not admitted gets a
    /// refusal (nothing is asked or shown to the user).
    /// The default admits everyone; interfaces follow
    /// [`Config::paired_only`](crate::config::Config::paired_only).
    fn admits(&self, _peer: EndpointId, _pairing: bool) -> impl Future<Output = bool> + Send {
        async { true }
    }

    /// `peer` removed this device from its trusted devices and asked
    /// ([`Hello::Unpair`]) to be removed here too. Implementations remove it
    /// from the trusted devices and refresh their lists; the default does
    /// nothing.
    fn peer_unpaired(&self, _peer: EndpointId) -> impl Future<Output = ()> + Send {
        async {}
    }

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

    /// Decides about files offered by a LocalSend app ([`crate::localsend`]).
    ///
    /// The sender is not authenticated: its name is whatever it announces and
    /// anyone on the local network can send such an offer. Implementations
    /// must therefore always ask the user (never Auto-Accept), show that the
    /// offer comes through the LocalSend compatibility mode and warn about
    /// [`LocalSendOffer::risk`]. The default declines.
    fn decide_localsend(
        &self,
        _offer: &LocalSendOffer,
    ) -> impl Future<Output = OfferDecision> + Send {
        async {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    /// Decides about files a web browser wants to send ([`crate::web`]).
    ///
    /// Only the device that opened the session's link can offer them, but the
    /// link travels over plain HTTP, so implementations always ask the user
    /// (never Auto-Accept) and warn about [`BrowserOffer::risk`]. The default
    /// declines.
    fn decide_browser(&self, _offer: &BrowserOffer) -> impl Future<Output = OfferDecision> + Send {
        async {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
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
/// cannot flood the screen with dialogs or queue unlimited work. The gate also
/// remembers answers the user already gave for files and batches whose
/// connection broke, for [`RESUME_WINDOW`], so a resumed transfer does not ask
/// again.
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
    /// Accepted files whose transfer broke: device and file hash → folder.
    resumable_files: HashMap<(EndpointId, [u8; 32]), (PathBuf, Instant)>,
    /// What was left of an accepted batch when its connection ended.
    resumable_batches: HashMap<EndpointId, (BatchApproval, Instant)>,
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

    /// Remembers that the user accepted the file `hash` from `peer` into
    /// `dest_dir`; its transfer broke and may be resumed.
    fn keep_file(&self, peer: EndpointId, hash: [u8; 32], dest_dir: PathBuf) {
        let mut state = self.lock();
        state.forget_expired();
        if state.resumable_files.len() < MAX_RESUMABLE {
            let until = Instant::now() + RESUME_WINDOW;
            state
                .resumable_files
                .insert((peer, hash), (dest_dir, until));
        }
    }

    /// Folder of an accepted file being resumed, if the user accepted it.
    fn take_file(&self, peer: EndpointId, hash: &[u8; 32]) -> Option<PathBuf> {
        let mut state = self.lock();
        state.forget_expired();
        state
            .resumable_files
            .remove(&(peer, *hash))
            .map(|(dir, _)| dir)
    }

    /// Keeps the rest of an accepted batch when its connection ends.
    fn keep_batch(&self, peer: EndpointId, approval: BatchApproval) {
        let mut state = self.lock();
        state.forget_expired();
        if approval.remaining_files > 0 && state.resumable_batches.len() < MAX_RESUMABLE {
            let until = Instant::now() + RESUME_WINDOW;
            state.resumable_batches.insert(peer, (approval, until));
        }
    }

    /// The rest of a batch `peer` started on an earlier connection.
    fn take_batch(&self, peer: &EndpointId) -> Option<BatchApproval> {
        let mut state = self.lock();
        state.forget_expired();
        state
            .resumable_batches
            .remove(peer)
            .map(|(approval, _)| approval)
    }
}

impl GateState {
    /// Drops remembered answers older than [`RESUME_WINDOW`].
    fn forget_expired(&mut self) {
        let now = Instant::now();
        self.resumable_files.retain(|_, (_, until)| *until > now);
        self.resumable_batches.retain(|_, (_, until)| *until > now);
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
    /// Set by [`pause_transfer`].
    pause: tokio::sync::watch::Receiver<bool>,
    /// Connection whose path is reported in every event, once known;
    /// replaced when the sender reconnects.
    conn: Mutex<Option<Connection>>,
    /// Bytes reported by the last progress event, shown while reconnecting.
    last_done: std::sync::atomic::AtomicU64,
    /// Progress events held back by [`PROGRESS_INTERVAL`].
    shown: Mutex<ShownProgress>,
}

/// What a [`Tracker`] has shown of the progress.
#[derive(Default)]
struct ShownProgress {
    /// When the last [`TransferStatus::Running`] event was emitted.
    at: Option<Instant>,
    /// Whether progress was held back since the last event; it is emitted by
    /// [`Tracker::cancellable`] once the interval has passed.
    owed: bool,
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
        Self::for_device(peer.to_string(), None, file_name, total, direction, sink)
    }

    /// Starts tracking a transfer with a device identified by `peer` and
    /// shown as `peer_name` (see [`TransferEvent::peer_name`]).
    pub(crate) fn for_device(
        peer: String,
        peer_name: Option<String>,
        file_name: &str,
        total: u64,
        direction: Direction,
        sink: EventSink<'a>,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let (pause_tx, pause) = tokio::sync::watch::channel(false);
        if let Ok(mut active) = ACTIVE_TRANSFERS.lock() {
            active.insert(
                id.clone(),
                ActiveTransfer {
                    cancel: cancel.clone(),
                    pause: pause_tx,
                },
            );
        }
        Self {
            base: TransferEvent {
                id,
                peer,
                peer_name,
                file_name: file_name.to_string(),
                direction,
                total,
                done: 0,
                status: TransferStatus::Preparing,
                route: None,
            },
            sink,
            cancel,
            pause,
            conn: Mutex::new(None),
            last_done: std::sync::atomic::AtomicU64::new(0),
            shown: Mutex::default(),
        }
    }

    /// Reports the path of `conn` ([`Route`]) in the following events.
    pub fn with_connection(self, conn: &Connection) -> Self {
        self.reconnected(conn);
        self
    }

    /// Reports the path of a new connection after a reconnection.
    fn reconnected(&self, conn: &Connection) {
        if let Ok(mut current) = self.conn.lock() {
            *current = Some(conn.clone());
        }
    }

    /// Runs `transfer` until it finishes or the transfer is cancelled, in
    /// which case the future is dropped (closing its streams) and the result
    /// is a [`Cancelled`] error.
    ///
    /// # Errors
    ///
    /// Returns the error of `transfer`, or [`Cancelled`].
    ///
    /// The transfer can also be paused ([`pause_transfer`]): its data loops
    /// wait in `control::pace` ([`super::control`]), and every change is
    /// reported as [`TransferStatus::Paused`] or [`TransferStatus::Running`].
    pub async fn cancellable<T>(&self, transfer: impl Future<Output = Result<T>>) -> Result<T> {
        let transfer = super::control::with_pause(self.pause.clone(), transfer);
        let mut transfer = std::pin::pin!(transfer);
        let mut pause = self.pause.clone();
        let mut watching = true;
        let mut tick = tokio::time::interval(PROGRESS_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Cancelled.into()),
                result = &mut transfer => return result,
                changed = pause.changed(), if watching => {
                    if changed.is_err() {
                        watching = false;
                        continue;
                    }
                    let done = self.last_done.load(std::sync::atomic::Ordering::Relaxed);
                    let paused = *pause.borrow_and_update();
                    self.emit(done, if paused { TransferStatus::Paused } else { TransferStatus::Running });
                }
                _ = tick.tick() => self.emit_owed_progress(),
            }
        }
    }

    /// ID of the tracked transfer.
    pub fn id(&self) -> &str {
        &self.base.id
    }

    /// Size of the transfer in bytes (0 when unknown).
    pub fn total(&self) -> u64 {
        self.base.total
    }

    /// Emits an event with the given progress and status.
    pub fn emit(&self, done: u64, status: TransferStatus) {
        if let Ok(mut shown) = self.shown.lock() {
            shown.owed = false;
            if status == TransferStatus::Running {
                shown.at = Some(Instant::now());
            }
        }
        let mut ev = self.base.clone();
        ev.done = done;
        ev.status = status;
        ev.route = self
            .conn
            .lock()
            .ok()
            .and_then(|c| c.as_ref().and_then(Route::of));
        if ev.status.is_final() {
            crate::history::record(&ev);
        }
        (self.sink)(ev);
    }

    /// Emits a [`TransferStatus::Running`] event: the first and the last
    /// (`done` reached the size) at once, the others at most every 100 ms.
    /// Progress held back is emitted by [`Tracker::cancellable`] when no newer
    /// one comes in time.
    pub fn progress(&self, done: u64) {
        self.last_done
            .store(done, std::sync::atomic::Ordering::Relaxed);
        // A total of 0 means an unknown size, not the end.
        let last = self.base.total > 0 && done >= self.base.total;
        let due = last
            || self.shown.lock().map_or(true, |mut shown| {
                let due = shown.at.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL);
                shown.owed = !due;
                due
            });
        if due {
            self.emit(done, TransferStatus::Running);
        }
    }

    /// Emits the progress [`Tracker::progress`] held back, unless the
    /// transfer is paused (its pause was reported with the latest progress).
    fn emit_owed_progress(&self) {
        let owed = self
            .shown
            .lock()
            .is_ok_and(|mut shown| std::mem::take(&mut shown.owed));
        if owed && !*self.pause.borrow() {
            let done = self.last_done.load(std::sync::atomic::Ordering::Relaxed);
            self.emit(done, TransferStatus::Running);
        }
    }

    /// Emits [`TransferStatus::Completed`], [`TransferStatus::Cancelled`] or
    /// [`TransferStatus::Failed`] for `result`; `path` extracts the saved file
    /// path from a success.
    pub(crate) fn finish<T>(&self, result: &Result<T>, path: impl FnOnce(&T) -> Option<String>) {
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
    let peer = conn.remote_id();
    let mut approval = gate.take_batch(&peer);
    let result = serve_streams(engine, &conn, handler, gate, &mut approval).await;
    // A batch that was not finished stays accepted for a reconnection.
    if let Some(rest) = approval {
        gate.keep_batch(peer, rest);
    }
    result
}

/// Serves the control streams of `conn` until it closes.
async fn serve_streams<H: TransferHandler>(
    engine: &NodeEngine,
    conn: &Connection,
    handler: Arc<H>,
    gate: &Arc<QuestionGate>,
    approval: &mut Option<BatchApproval>,
) -> Result<()> {
    while let Some((channel, hello)) = Channel::accept(conn).await? {
        let pairing = matches!(hello, Hello::Pair { .. });
        // Downloads by TICKET are the holder's right: the token is the secret.
        if !matches!(hello, Hello::Pull { .. }) && !handler.admits(conn.remote_id(), pairing).await
        {
            info!(
                "Declined a request from the unknown device {}",
                conn.remote_id()
            );
            if let Err(e) = refuse(channel, &hello).await {
                debug!("Could not tell the device about the refusal: {e:#}");
            }
            continue;
        }
        let result = match hello {
            Hello::Push => handle_push(conn, channel, handler.as_ref(), gate, approval).await,
            Hello::Batch(batch) => handle_batch(conn, channel, &batch, handler.as_ref(), gate)
                .await
                .map(|granted| *approval = granted),
            Hello::Pull { token } => handle_pull(conn, channel, &token, handler.as_ref()).await,
            Hello::Pair {
                name,
                addr,
                commitment,
            } => {
                handle_pair(
                    engine,
                    conn,
                    channel,
                    &name,
                    addr,
                    &commitment,
                    handler.as_ref(),
                    gate,
                )
                .await
            }
            Hello::Text { text } => handle_text(conn, channel, text, handler.as_ref(), gate).await,
            Hello::Unpair => {
                handler.peer_unpaired(conn.remote_id()).await;
                channel.finish().await;
                Ok(())
            }
        };
        if let Err(e) = result {
            warn!("Transfer from {} failed: {e:#}", conn.remote_id());
        }
    }
    debug!("Connection with {} finished", conn.remote_id());
    Ok(())
}

/// Answers a request of a device that is not admitted ([`TransferHandler::admits`])
/// with a refusal that looks like the user's, and closes the stream. A pairing
/// request gets no answer at all, as when it arrives too often.
async fn refuse(mut channel: Channel, hello: &Hello) -> Result<()> {
    let reason = t!("service.paired_only").into_owned();
    match hello {
        Hello::Push => write_msg(&mut channel.send, &TransferDecision::Reject { reason }).await?,
        Hello::Batch(_) => {
            write_msg(&mut channel.send, &BatchDecision::Rejected { reason }).await?
        }
        Hello::Text { .. } => {
            write_msg(&mut channel.send, &TextDecision::Rejected { reason }).await?;
        }
        Hello::Pair { .. } | Hello::Pull { .. } | Hello::Unpair => return Ok(()),
    }
    channel.finish().await;
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
    )
    .with_connection(conn);
    // A batch approval does not cover files that can run code: the user saw at
    // most 20 names, and an Auto-Accept batch was never looked at. Those files
    // go through `decide` like single files, where the interface warns about them.
    let covered_by_batch = risk::file_risk(&offer.file_name).is_none();
    let accepted_before = gate.take_file(peer, &offer.file_hash).or_else(|| {
        covered_by_batch
            .then(|| approval.as_mut().and_then(|a| a.take(&offer)))
            .flatten()
    });
    let decision = match accepted_before {
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
            if let Err(e) = &result
                && is_retryable(e)
            {
                // The sender reconnects and resumes; the user already said yes.
                gate.keep_file(peer, offer.file_hash, dest_dir);
            }
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
    )
    .with_connection(conn);
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
    let (code, discovery_key) =
        match answer_pairing(&mut channel, &engine.node_id(), &peer, commitment).await {
            Ok(answer) => answer,
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
        discovery_key,
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
    let text_len = text.len();
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
    crate::history::record_text(HistoryEntry::text(
        Direction::Download,
        &peer.to_string(),
        text_len,
        match &decision {
            TextDecision::Accepted => Ok(()),
            TextDecision::Rejected { reason } => Err(("rejected", reason.clone())),
        },
    ));
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
    let decision = match decision {
        Some(OfferDecision::Accept { dest_dir }) => {
            match check_free_space(&dest_dir, batch.total_size) {
                Ok(()) => Some(OfferDecision::Accept { dest_dir }),
                Err(reason) => Some(OfferDecision::Reject { reason }),
            }
        }
        other => other,
    };
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

/// Whether a failed transfer is worth resuming on a new connection: the
/// connection broke or timed out, as opposed to a refusal, a corrupted file or
/// a full disk.
fn is_retryable(error: &anyhow::Error) -> bool {
    matches!(
        crate::error::kind(error),
        crate::error::ErrorKind::Connection | crate::error::ErrorKind::Timeout
    )
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
                conn = engine.connect(target.clone()).await?;
            }
        }
    }
    let mut failures = 0usize;
    for (item, (name, size)) in items.iter().zip(&files) {
        let tracker =
            Tracker::new(peer, name, *size, Direction::Upload, sink).with_connection(&conn);
        tracker.emit(0, TransferStatus::Preparing);
        let prepared = tracker
            .cancellable(async {
                let mut file = prepare_file(&item.path).await?;
                file.folder.clone_from(&item.folder);
                Ok(file)
            })
            .await;
        let result = match prepared {
            Ok(file) => push_with_retries(engine, &target, &mut conn, &file, &tracker).await,
            Err(e) => Err(e),
        };
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

/// Sends `paths` to every device of `targets` at the same time, as
/// [`send_files`] does for one. Every file is hashed once for all of them.
/// Returns one result per target, in the same order; a device that cannot be
/// reached does not affect the others.
pub async fn send_files_to_many(
    engine: &NodeEngine,
    targets: &[iroh::EndpointAddr],
    paths: &[PathBuf],
    sink: EventSink<'_>,
) -> Vec<Result<()>> {
    if targets.len() > 1 {
        // Hash before the sends start, so they find the hashes ready instead
        // of all reading the same files at once.
        let owned = paths.to_vec();
        if let Ok(Ok(items)) = tokio::task::spawn_blocking(move || expand_paths(&owned)).await {
            for item in items {
                let _ = prepare_file(&item.path).await;
            }
        }
    }
    futures_util::future::join_all(
        targets
            .iter()
            .map(|target| send_files(engine, target.clone(), paths, sink)),
    )
    .await
}

/// Pushes `file` on `conn`. When the connection breaks, waits a little
/// ([`RETRY_DELAYS`]), reconnects to `target` and pushes again: the receiver
/// resumes from its partial file and does not ask the user again
/// ([`QuestionGate::keep_file`]). `conn` is replaced by the new connection.
///
/// # Errors
///
/// The error of the last attempt, or [`Cancelled`].
async fn push_with_retries(
    engine: &NodeEngine,
    target: &iroh::EndpointAddr,
    conn: &mut Connection,
    file: &PreparedFile,
    tracker: &Tracker<'_>,
) -> Result<SendOutcome> {
    let attempts = u32::try_from(RETRY_DELAYS.len()).unwrap_or(u32::MAX);
    for (attempt, delay) in (1..).zip(RETRY_DELAYS) {
        let result = tracker
            .cancellable(push_file(conn, file, &|d| tracker.progress(d)))
            .await;
        match &result {
            Err(e) if is_retryable(e) => {
                warn!(
                    "Sending '{}' broke off ({e:#}); retry {attempt}/{attempts} in {delay:?}",
                    file.name
                );
            }
            _ => return result,
        }
        let done = tracker.last_done.load(std::sync::atomic::Ordering::Relaxed);
        tracker.emit(
            done,
            TransferStatus::Retrying {
                attempt,
                of: attempts,
            },
        );
        let reconnect = async {
            tokio::time::sleep(delay).await;
            engine.connect(target.clone()).await
        };
        match tracker.cancellable(reconnect).await {
            Ok(new) => {
                *conn = new;
                tracker.reconnected(conn);
            }
            Err(e) if e.is::<Cancelled>() => return Err(e),
            // The next push fails at once on the old connection and counts
            // as the next attempt.
            Err(e) => warn!("Reconnecting to {} failed: {e:#}", target.id),
        }
    }
    tracker
        .cancellable(push_file(conn, file, &|d| tracker.progress(d)))
        .await
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
    )
    .with_connection(&conn);
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
    let peer = target.id.to_string();
    let decision = async {
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
    .await;
    crate::history::record_text(HistoryEntry::text(
        Direction::Upload,
        &peer,
        text.len(),
        match &decision {
            Ok(TextDecision::Accepted) => Ok(()),
            Ok(TextDecision::Rejected { reason }) => Err(("rejected", reason.clone())),
            Err(e) => Err(("failed", format!("{e:#}"))),
        },
    ));
    decision
}

/// Time [`send_unpair`] waits for the other device.
const UNPAIR_TIMEOUT: Duration = Duration::from_secs(8);

/// Tells a device that this one removed it from its trusted devices, so that
/// the other device stops listing this one as paired. Best effort: a device
/// that is offline or older keeps the pairing until its user removes it.
///
/// # Errors
///
/// Fails when the device cannot be reached in time.
pub async fn send_unpair(engine: &NodeEngine, target: iroh::EndpointAddr) -> Result<()> {
    tokio::time::timeout(UNPAIR_TIMEOUT, async {
        let conn = engine.connect(target).await?;
        let Channel { mut send, mut recv } = Channel::open(&conn, &Hello::Unpair).await?;
        send.finish()?;
        // The other device ends its side once it has acted on the request.
        let answer = recv.read(&mut [0u8; 1]).await;
        conn.close(0u32.into(), b"done");
        match answer {
            Ok(None) => Ok(()),
            Ok(Some(_)) => anyhow::bail!(t!("pairing.interrupted")),
            Err(e) => Err(e).with_context(|| t!("pairing.interrupted")),
        }
    })
    .await
    .with_context(|| t!("pairing.no_answer"))?
}

/// Tells the removed devices among `nearby` ([`crate::config::Config::pending_unpair`])
/// that this device removed them, now that they can be reached. Returns the
/// Node IDs that were told; the caller forgets them. A device already being
/// told by another call is skipped.
pub async fn deliver_unpairs(
    engine: &NodeEngine,
    nearby: &[crate::discovery::NearbyDevice],
    pending: &[String],
) -> Vec<String> {
    static IN_PROGRESS: LazyLock<Mutex<std::collections::HashSet<String>>> =
        LazyLock::new(Mutex::default);
    let mut told = Vec::new();
    for device in nearby.iter().filter(|d| pending.contains(&d.node_id)) {
        let fresh = IN_PROGRESS
            .lock()
            .is_ok_and(|mut busy| busy.insert(device.node_id.clone()));
        if !fresh {
            continue;
        }
        let result = send_unpair(engine, device.addr.clone()).await;
        if let Ok(mut busy) = IN_PROGRESS.lock() {
            busy.remove(&device.node_id);
        }
        match result {
            Ok(()) => {
                info!("Told the removed device {} about it", device.node_id);
                told.push(device.node_id.clone());
            }
            Err(e) => debug!("Could not tell {} yet: {e:#}", device.node_id),
        }
    }
    told
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

    /// Progress of a fast transfer reaches the interfaces a few times per
    /// second: the first value and the end at once, held-back values later.
    #[tokio::test(start_paused = true)]
    async fn progress_events_are_limited() {
        let events = Mutex::new(Vec::new());
        let sink = |e: TransferEvent| events.lock().unwrap().push((e.done, e.status));
        let peer = iroh::SecretKey::generate().public();
        let tracker = Tracker::new(peer, "a.bin", 1000, Direction::Upload, &sink);
        let running = |events: &Mutex<Vec<(u64, TransferStatus)>>| {
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, s)| *s == TransferStatus::Running)
                .map(|(done, _)| *done)
                .collect::<Vec<_>>()
        };

        tracker
            .cancellable(async {
                for done in 1..=500 {
                    tracker.progress(done);
                }
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(running(&events), [1], "only the first of a burst");

        tracker
            .cancellable(async {
                tracker.progress(600);
                tokio::time::sleep(PROGRESS_INTERVAL * 3).await;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(running(&events), [1, 600], "held back, then emitted");

        pause_transfer(tracker.id(), true);
        tracker
            .cancellable(async {
                tracker.progress(700);
                tokio::time::sleep(PROGRESS_INTERVAL * 3).await;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(running(&events), [1, 600], "nothing while paused");

        tracker.progress(1000);
        assert_eq!(running(&events), [1, 600, 1000], "the end at once");
    }

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

    proptest::proptest! {
        /// Whatever a sender offers after an accepted batch, no more files and
        /// bytes are accepted than announced.
        #[test]
        fn batch_approval_holds_for_any_offers(
            files in 0u32..20,
            bytes in 0u64..10_000,
            sizes in proptest::collection::vec(0u64..4_000, 0..40),
        ) {
            let mut approval = BatchApproval {
                remaining_files: files,
                remaining_bytes: bytes,
                dest_dir: PathBuf::from("/tmp"),
            };
            let accepted: Vec<u64> = sizes.into_iter().filter(|s| approval.take(&offer(*s)).is_some()).collect();
            proptest::prop_assert!(accepted.len() as u64 <= u64::from(files));
            proptest::prop_assert!(accepted.iter().sum::<u64>() <= bytes);
        }
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
