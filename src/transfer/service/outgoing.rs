//! Sending: files and folders (to one or several devices, resumed after a
//! broken connection), downloads by TICKET, texts and unpair notices.

use super::events::{Cancelled, Direction, EventSink, TransferStatus};
use super::tracker::{Tracker, discard_partial};
use crate::history::HistoryEntry;
use crate::node::{NodeEngine, NodeTicket};
use crate::protocol::codec::read_msg;
use crate::protocol::messages::{
    BatchDecision, BatchOffer, Hello, MAX_BATCH_NAMES, MAX_FILE_NAME_LEN, MAX_TEXT_LEN,
    TextDecision,
};
use crate::transfer::Channel;
use crate::transfer::receiver::{part_path_for, read_offer, receive_on_channel};
use crate::transfer::sender::{
    PreparedFile, SendItem, SendOutcome, expand_paths, prepare_file, push_file,
};
use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Pauses before the sender tries again after the connection broke: enough for
/// a phone waking up or a network coming back, short enough to wait for.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// Whether a failed transfer is worth resuming on a new connection: the
/// connection broke or timed out, as opposed to a refusal, a corrupted file or
/// a full disk.
pub(super) fn is_retryable(error: &anyhow::Error) -> bool {
    matches!(
        crate::error::kind(error),
        crate::error::ErrorKind::Connection | crate::error::ErrorKind::Timeout
    )
}

/// Turns a [`Cancelled`] error into success: cancelling is the user's choice,
/// not a failure.
pub(super) fn ignore_cancelled<T>(result: Result<T>) -> Result<()> {
    match result {
        Err(e) if !e.is::<Cancelled>() => Err(e),
        _ => Ok(()),
    }
}

/// Emits the final event of an upload; a rejection or cancellation is not an error.
pub(super) fn report_send(tracker: &Tracker<'_>, result: Result<SendOutcome>) -> Result<()> {
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
        // A photo goes as a copy without its location and other metadata
        // when the user asked for it; the copy is deleted after the transfer.
        let copy = crate::transfer::metadata::copy_without_metadata(&item.path).await;
        let size = match &copy {
            Ok(Some(copy)) => tokio::fs::metadata(copy.path())
                .await
                .map_or(*size, |m| m.len()),
            _ => *size,
        };
        let tracker = Tracker::new(peer, name, size, Direction::Upload, sink)
            .with_connection(&conn)
            .with_source(&item.path);
        tracker.emit(0, TransferStatus::Preparing);
        let prepared = tracker
            .cancellable(async {
                let send_path = match &copy {
                    Ok(Some(copy)) => copy.path(),
                    Ok(None) => item.path.as_path(),
                    Err(e) => anyhow::bail!("{e:#}"),
                };
                let mut file = prepare_file(send_path).await?;
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
/// ([`QuestionGate::keep_file`](super::gate::QuestionGate::keep_file)). `conn` is replaced by the new connection.
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
        tracker.emit(
            tracker.done(),
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
    discard_partial(&tracker, &result, part_path_for(dest_dir, &offer)).await;
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
