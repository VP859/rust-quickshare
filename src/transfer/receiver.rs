//! Receiving half of the transfer protocol, with resume support.

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use rust_i18n::t;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::{Channel, ProgressFn, short_hex};
use crate::error::{IntegrityError, NotEnoughSpace, ProtocolViolation};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    ChunkHeader, TransferDecision, TransferOffer, TransferResult, chunk_window,
};
use crate::size::human_size;
use crate::transfer::hash::hash_prefix;
use crate::transfer::security::{
    resolve_secure_folder, resolve_secure_target_path, sanitize_filename, unique_destination,
};

/// Reads the [`TransferOffer`] from `channel` and validates it.
///
/// # Errors
///
/// Fails on network errors, offers exceeding protocol limits and unsafe file names.
pub async fn read_offer(channel: &mut Channel) -> Result<TransferOffer> {
    let offer: TransferOffer = read_msg(&mut channel.recv).await?;
    offer.validate()?;
    sanitize_filename(&offer.file_name)?;
    for component in &offer.folder {
        sanitize_filename(component)?;
    }
    Ok(offer)
}

/// Declines the offer with `reason` and closes the control stream.
///
/// # Errors
///
/// Fails when the decision cannot be sent.
pub async fn reject(mut channel: Channel, reason: &str) -> Result<()> {
    let decision = TransferDecision::Reject {
        reason: reason.to_string(),
    };
    write_msg(&mut channel.send, &decision).await?;
    channel.finish().await;
    Ok(())
}

/// Free space kept on the disk after a transfer (256 MiB), so a received file
/// never fills it completely.
pub const DISK_RESERVE: u64 = 256 * 1024 * 1024;

/// Checks that `needed` more bytes fit on the disk holding `dir`, keeping
/// [`DISK_RESERVE`] free; the error is the localized reason for the sender.
/// When the free space cannot be determined the transfer is allowed.
///
/// # Errors
///
/// Returns the reason when the disk is too full.
pub fn check_free_space(dir: &Path, needed: u64) -> Result<(), String> {
    // The folder may not exist yet (e.g. a batch's destination); the disk is
    // the one of its nearest existing parent.
    let Some(existing) = dir.ancestors().find(|p| p.exists()) else {
        return Ok(());
    };
    let Ok(free) = fs4::available_space(existing) else {
        return Ok(());
    };
    if needed.saturating_add(DISK_RESERVE) > free {
        return Err(t!(
            "receiver.no_space",
            needed = human_size(needed),
            free = human_size(free)
        )
        .into_owned());
    }
    Ok(())
}

/// Folder inside `dest_dir` where `offer` is saved ([`TransferOffer::folder`]),
/// created when missing.
///
/// # Errors
///
/// Fails when a folder name is unsafe or the folder cannot be created.
pub fn target_dir_for(dest_dir: &Path, offer: &TransferOffer) -> Result<PathBuf> {
    Ok(resolve_secure_folder(dest_dir, &offer.folder)?)
}

/// Path of the partial download for `offer` inside its folder in `dest_dir`.
///
/// The name contains a fragment of the file hash, so resuming never appends
/// data of a different file that happens to have the same name.
///
/// # Errors
///
/// Fails when the offered file or folder name is unsafe or the folder cannot
/// be prepared.
pub fn part_path_for(dest_dir: &Path, offer: &TransferOffer) -> Result<PathBuf> {
    let safe = sanitize_filename(&offer.file_name)?;
    Ok(resolve_secure_target_path(
        &target_dir_for(dest_dir, offer)?,
        &format!("{safe}.{}.part", short_hex(&offer.file_hash[..6])),
    )?)
}

/// Runs the receiving side of the protocol after the offer was accepted.
///
/// Data is written to a `.part` file. If one already exists the transfer resumes
/// from its last complete chunk. Every chunk and finally the whole file are
/// verified with BLAKE3; the verified file is renamed to a destination that
/// never overwrites an existing file. Returns the final path.
///
/// # Errors
///
/// Fails on network or I/O errors and hash mismatches. A corrupted `.part`
/// file is deleted so that the next attempt starts from scratch.
pub async fn receive_on_channel(
    conn: &Connection,
    mut channel: Channel,
    offer: &TransferOffer,
    dest_dir: &Path,
    progress: ProgressFn<'_>,
) -> Result<PathBuf> {
    let safe_name = sanitize_filename(&offer.file_name)?;
    tokio::fs::create_dir_all(dest_dir)
        .await
        .with_context(|| t!("receiver.create_dir", path = dest_dir.display()))?;
    let target_dir = target_dir_for(dest_dir, offer)?;
    let final_target = resolve_secure_target_path(&target_dir, &safe_name)?;
    let part_path = part_path_for(dest_dir, offer)?;

    let total_chunks = offer.total_chunks();
    let chunk_size = offer.chunk_size as u64;
    let existing = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let start_chunk_index = (existing / chunk_size).min(total_chunks);
    let valid_bytes = (start_chunk_index * chunk_size).min(offer.file_size);
    let mut hasher = if start_chunk_index > 0 {
        info!("Resuming '{safe_name}' from chunk #{start_chunk_index} ({valid_bytes} B on disk)");
        hash_prefix(&part_path, valid_bytes).await?
    } else {
        blake3::Hasher::new()
    };

    // Refuse what does not fit instead of failing near the end of a long
    // transfer with a full disk (which would also fill it for everyone else).
    if let Err(reason) = check_free_space(&target_dir, offer.file_size - valid_bytes) {
        write_msg(
            &mut channel.send,
            &TransferDecision::Reject {
                reason: reason.clone(),
            },
        )
        .await?;
        channel.finish().await;
        return Err(NotEnoughSpace(reason).into());
    }

    write_msg(
        &mut channel.send,
        &TransferDecision::Accept { start_chunk_index },
    )
    .await?;

    let mut part_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&part_path)
        .await
        .with_context(|| t!("receiver.open_part", path = part_path.display()))?;
    part_file.set_len(valid_bytes).await?;
    part_file.seek(SeekFrom::Start(valid_bytes)).await?;
    progress(valid_bytes);

    receive_chunks(
        conn,
        &mut channel.send,
        offer,
        start_chunk_index,
        valid_bytes,
        &mut part_file,
        &mut hasher,
        progress,
    )
    .await?;
    part_file.sync_all().await?;
    drop(part_file);

    if *hasher.finalize().as_bytes() != offer.file_hash {
        let _ = tokio::fs::remove_file(&part_path).await;
        let reason = t!("receiver.hash_mismatch").into_owned();
        write_msg(
            &mut channel.send,
            &TransferResult::Failed {
                reason: reason.clone(),
            },
        )
        .await?;
        channel.finish().await;
        return Err(IntegrityError(reason).into());
    }

    let final_path = unique_destination(&final_target);
    tokio::fs::rename(&part_path, &final_path)
        .await
        .with_context(|| t!("receiver.rename"))?;

    // Windows: Mark-of-the-Web, so SmartScreen and Office warn before it is opened.
    super::risk::mark_from_network(&final_path);

    write_msg(&mut channel.send, &TransferResult::Verified).await?;
    channel.finish().await;
    info!("Received and verified {}", final_path.display());
    Ok(final_path)
}

/// Task reading one chunk; aborted when dropped (e.g. when the transfer fails).
struct ChunkTask(JoinHandle<Result<Vec<u8>>>);

impl Drop for ChunkTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl ChunkTask {
    /// Waits for the chunk; fails when the task failed or was aborted.
    async fn join(mut self) -> Result<Vec<u8>> {
        (&mut self.0)
            .await
            .with_context(|| t!("transfer.task_aborted"))?
    }
}

/// Reads chunks concurrently but writes them strictly in order, so the `.part`
/// file always holds a contiguous prefix of the file (the basis for resuming).
///
/// Every written chunk is also fed to `hasher`, which therefore ends up with
/// the hash of the whole file without reading it back from disk.
#[allow(clippy::too_many_arguments)] // the state of one transfer, passed down once
async fn receive_chunks(
    conn: &Connection,
    feedback: &mut SendStream,
    offer: &TransferOffer,
    start_chunk_index: u64,
    done: u64,
    part_file: &mut File,
    hasher: &mut blake3::Hasher,
    progress: ProgressFn<'_>,
) -> Result<()> {
    // The same window as the sender's; it also bounds the memory of the
    // chunks being read to about IN_FLIGHT_BYTES.
    let window = chunk_window(offer.chunk_size);
    let mut pending: VecDeque<ChunkTask> = VecDeque::with_capacity(window);
    let mut meter = ArrivalMeter::new(done, progress, feedback);

    for chunk_index in start_chunk_index..offer.total_chunks() {
        if pending.len() >= window
            && let Some(task) = pending.pop_front()
        {
            let payload = meter.while_waiting(task.join()).await??;
            hasher.update(&payload);
            part_file.write_all(&payload).await?;
            meter.report().await?;
        }

        let expected_len = offer
            .chunk_len(chunk_index)
            .with_context(|| t!("transfer.chunk_len"))?;
        let stream = meter
            .while_waiting(conn.accept_uni())
            .await?
            .with_context(|| t!("receiver.connection_lost"))?;
        pending.push_back(ChunkTask(tokio::spawn(read_chunk(
            stream,
            chunk_index,
            expected_len,
            meter.arrived.clone(),
        ))));
    }

    while let Some(task) = pending.pop_front() {
        let payload = meter.while_waiting(task.join()).await??;
        hasher.update(&payload);
        part_file.write_all(&payload).await?;
        meter.report().await?;
    }
    part_file.flush().await?;
    Ok(())
}

/// Reports bytes as they arrive, not only after whole chunks: with 1 MiB
/// chunks a small file on a slow link would otherwise stay at 0% for minutes.
/// Every change goes to the local progress callback and, as
/// [`TransferResult::Progress`], to the sender, so both screens agree.
struct ArrivalMeter<'a> {
    /// Bytes that were already on disk when the transfer started (resume).
    base: u64,
    /// Payload bytes received so far by all chunk tasks.
    arrived: Arc<AtomicU64>,
    /// Last reported value, to avoid repeating it.
    last: u64,
    /// Progress callback of the transfer.
    progress: ProgressFn<'a>,
    /// Control stream to the sender.
    feedback: &'a mut SendStream,
}

impl<'a> ArrivalMeter<'a> {
    /// Interval of progress updates while waiting for data.
    const TICK: std::time::Duration = std::time::Duration::from_millis(250);

    /// Starts counting at `base` bytes.
    fn new(base: u64, progress: ProgressFn<'a>, feedback: &'a mut SendStream) -> Self {
        Self {
            base,
            arrived: Arc::new(AtomicU64::new(0)),
            last: base,
            progress,
            feedback,
        }
    }

    /// Reports the current count to this device and to the sender if it changed.
    ///
    /// # Errors
    ///
    /// Fails when the control stream to the sender is broken.
    async fn report(&mut self) -> Result<()> {
        let now = self.base + self.arrived.load(Ordering::Relaxed);
        if now != self.last {
            self.last = now;
            (self.progress)(now);
            write_msg(self.feedback, &TransferResult::Progress { received: now }).await?;
        }
        Ok(())
    }

    /// Awaits `fut`, reporting progress periodically in the meantime.
    ///
    /// # Errors
    ///
    /// Fails when a progress report cannot be sent; `fut` is then dropped.
    async fn while_waiting<T>(&mut self, fut: impl std::future::Future<Output = T>) -> Result<T> {
        let mut fut = std::pin::pin!(fut);
        let mut tick = tokio::time::interval(Self::TICK);
        loop {
            tokio::select! {
                result = &mut fut => return Ok(result),
                _ = tick.tick() => self.report().await?,
            }
        }
    }
}

/// Reads one chunk stream and checks its index, length and BLAKE3 hash.
async fn read_chunk(
    mut stream: RecvStream,
    chunk_index: u64,
    expected_len: u32,
    arrived: Arc<AtomicU64>,
) -> Result<Vec<u8>> {
    let header: ChunkHeader = read_msg(&mut stream).await?;
    if header.chunk_index != chunk_index {
        return Err(ProtocolViolation(
            t!(
                "receiver.chunk_order",
                expected = chunk_index,
                got = header.chunk_index
            )
            .into_owned(),
        )
        .into());
    }
    if header.payload_len != expected_len {
        return Err(ProtocolViolation(
            t!(
                "receiver.chunk_len",
                index = chunk_index,
                got = header.payload_len,
                expected = expected_len
            )
            .into_owned(),
        )
        .into());
    }

    let mut payload = vec![0u8; expected_len as usize];
    let mut filled = 0;
    while filled < payload.len() {
        let n = stream
            .read(&mut payload[filled..])
            .await
            .with_context(|| t!("receiver.chunk_read"))?
            .with_context(|| t!("receiver.chunk_read"))?;
        filled += n;
        arrived.fetch_add(n as u64, Ordering::Relaxed);
    }
    // Read up to the end of the stream: dropping it before the sender's FIN
    // arrived would send STOP_SENDING, which the sender reports as a failure.
    if stream
        .read(&mut [0u8; 1])
        .await
        .with_context(|| t!("receiver.chunk_read"))?
        .is_some()
    {
        return Err(ProtocolViolation(
            t!(
                "receiver.chunk_len",
                index = chunk_index,
                got = "> expected",
                expected = expected_len
            )
            .into_owned(),
        )
        .into());
    }

    if *blake3::hash(&payload).as_bytes() != header.chunk_hash {
        warn!("Chunk #{chunk_index} failed BLAKE3 verification");
        return Err(
            IntegrityError(t!("receiver.chunk_hash", index = chunk_index).into_owned()).into(),
        );
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_checked_with_a_reserve() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_free_space(dir.path(), 1).is_ok());
        assert!(check_free_space(dir.path(), u64::MAX).is_err());
        let missing = dir.path().join("not/yet/created");
        assert!(
            check_free_space(&missing, 1).is_ok(),
            "uses the existing parent"
        );
        assert!(check_free_space(&missing, u64::MAX).is_err());
    }
}
