//! [`Tracker`]: one transfer as the interfaces see it – its progress events
//! (at most every [`PROGRESS_INTERVAL`]), pausing and cancelling.

use super::events::{Cancelled, Direction, EventSink, Route, TransferEvent, TransferStatus};
use anyhow::Result;
use iroh::EndpointId;
use iroh::endpoint::Connection;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Shortest interval between two progress events of one transfer. Data moves
/// in pieces of up to a few MiB, so a fast transfer would otherwise report
/// hundreds of times per second, and every event costs the interfaces work
/// (the GUI redraws the transfer).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Controls of one transfer in progress.
struct ActiveTransfer {
    /// Ends the transfer ([`cancel_transfer`]).
    cancel: CancellationToken,
    /// `true` while it is paused ([`pause_transfer`]).
    pause: tokio::sync::watch::Sender<bool>,
    /// Set by [`cancel_and_discard`]: the partly received file goes too.
    discard: Arc<std::sync::atomic::AtomicBool>,
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

/// Stops a running download like [`cancel_transfer`] and deletes what was
/// received so far (the `.part` file), so it is not resumed later and does not
/// stay on the disk; returns whether the transfer was found.
///
/// Only Ferry downloads keep a partial file; LocalSend and browser uploads
/// delete theirs anyway when they end early. For an upload this is the same as
/// [`cancel_transfer`]: the partial file is on the other device.
pub fn cancel_and_discard(id: &str) -> bool {
    if let Ok(active) = ACTIVE_TRANSFERS.lock()
        && let Some(transfer) = active.get(id)
    {
        transfer
            .discard
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    cancel_transfer(id)
}

/// Deletes the partial file of a download that was cancelled with
/// [`cancel_and_discard`]. On Windows a write still in progress keeps the file
/// open for a moment, so deleting is tried a few times.
pub(super) async fn discard_partial(
    tracker: &Tracker<'_>,
    result: &Result<PathBuf>,
    part: Result<PathBuf>,
) {
    let cancelled = result.as_ref().is_err_and(|e| e.is::<Cancelled>());
    let Ok(part) = part else {
        return;
    };
    if !cancelled || !tracker.discard_requested() {
        return;
    }
    for attempt in 0..10 {
        match tokio::fs::remove_file(&part).await {
            Ok(()) => {
                info!("Deleted the partial file {}", part.display());
                return;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) if attempt == 9 => warn!("Could not delete {}: {e}", part.display()),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
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
    /// The file being sent (uploads), kept in the history.
    source: Option<PathBuf>,
    /// Set by [`cancel_and_discard`].
    discard: Arc<std::sync::atomic::AtomicBool>,
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
        let discard = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Ok(mut active) = ACTIVE_TRANSFERS.lock() {
            active.insert(
                id.clone(),
                ActiveTransfer {
                    cancel: cancel.clone(),
                    pause: pause_tx,
                    discard: discard.clone(),
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
            source: None,
            discard,
        }
    }

    /// Whether the transfer was cancelled with [`cancel_and_discard`].
    pub fn discard_requested(&self) -> bool {
        self.discard.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Records `path` as the file sent, so the history can send it again.
    pub fn with_source(mut self, path: &Path) -> Self {
        self.source = Some(path.to_path_buf());
        self
    }

    /// Reports the path of `conn` ([`Route`]) in the following events.
    pub fn with_connection(self, conn: &Connection) -> Self {
        self.reconnected(conn);
        self
    }

    /// Reports the path of a new connection after a reconnection.
    pub(super) fn reconnected(&self, conn: &Connection) {
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
    /// wait in `control::pace` ([`crate::transfer::control`]), and every change is
    /// reported as [`TransferStatus::Paused`] or [`TransferStatus::Running`].
    pub async fn cancellable<T>(&self, transfer: impl Future<Output = Result<T>>) -> Result<T> {
        let transfer = crate::transfer::control::with_pause(self.pause.clone(), transfer);
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

    /// The last progress reported (bytes).
    pub(super) fn done(&self) -> u64 {
        self.last_done.load(std::sync::atomic::Ordering::Relaxed)
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
            crate::history::record(&ev, self.source.as_deref());
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
}
