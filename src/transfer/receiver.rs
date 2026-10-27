//! Receiving half of the transfer protocol, with resume support.

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream};
use rust_i18n::t;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::{Channel, ProgressFn, short_hex};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    CHUNK_WINDOW, ChunkHeader, TransferDecision, TransferOffer, TransferResult,
};
use crate::transfer::hash::hash_prefix;
use crate::transfer::security::{
    resolve_secure_target_path, sanitize_filename, unique_destination,
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

/// Path of the partial download for `offer` inside `dest_dir`.
///
/// The name contains a fragment of the file hash, so resuming never appends
/// data of a different file that happens to have the same name.
///
/// # Errors
///
/// Fails when the offered file name is unsafe or `dest_dir` cannot be prepared.
pub fn part_path_for(dest_dir: &Path, offer: &TransferOffer) -> Result<PathBuf> {
    let safe = sanitize_filename(&offer.file_name)?;
    Ok(resolve_secure_target_path(
        dest_dir,
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
    let final_target = resolve_secure_target_path(dest_dir, &safe_name)?;
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
        anyhow::bail!(reason);
    }

    let final_path = unique_destination(&final_target);
    tokio::fs::rename(&part_path, &final_path)
        .await
        .with_context(|| t!("receiver.rename"))?;

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
async fn receive_chunks(
    conn: &Connection,
    offer: &TransferOffer,
    start_chunk_index: u64,
    mut done: u64,
    part_file: &mut File,
    hasher: &mut blake3::Hasher,
    progress: ProgressFn<'_>,
) -> Result<()> {
    let mut pending: VecDeque<ChunkTask> = VecDeque::with_capacity(CHUNK_WINDOW);

    for chunk_index in start_chunk_index..offer.total_chunks() {
        if pending.len() >= CHUNK_WINDOW
            && let Some(task) = pending.pop_front()
        {
            let payload = task.join().await?;
            hasher.update(&payload);
            part_file.write_all(&payload).await?;
            done += payload.len() as u64;
            progress(done);
        }

        let expected_len = offer
            .chunk_len(chunk_index)
            .with_context(|| t!("transfer.chunk_len"))?;
        let stream = conn
            .accept_uni()
            .await
            .with_context(|| t!("receiver.connection_lost"))?;
        pending.push_back(ChunkTask(tokio::spawn(read_chunk(
            stream,
            chunk_index,
            expected_len,
        ))));
    }

    while let Some(task) = pending.pop_front() {
        let payload = task.join().await?;
        hasher.update(&payload);
        part_file.write_all(&payload).await?;
        done += payload.len() as u64;
        progress(done);
    }
    part_file.flush().await?;
    Ok(())
}

/// Reads one chunk stream and checks its index, length and BLAKE3 hash.
async fn read_chunk(
    mut stream: RecvStream,
    chunk_index: u64,
    expected_len: u32,
) -> Result<Vec<u8>> {
    let header: ChunkHeader = read_msg(&mut stream).await?;
    if header.chunk_index != chunk_index {
        anyhow::bail!(t!(
            "receiver.chunk_order",
            expected = chunk_index,
            got = header.chunk_index
        ));
    }
    if header.payload_len != expected_len {
        anyhow::bail!(t!(
            "receiver.chunk_len",
            index = chunk_index,
            got = header.payload_len,
            expected = expected_len
        ));
    }

    let mut payload = vec![0u8; expected_len as usize];
    stream
        .read_exact(&mut payload)
        .await
        .with_context(|| t!("receiver.chunk_read"))?;

    if *blake3::hash(&payload).as_bytes() != header.chunk_hash {
        warn!("Chunk #{chunk_index} failed BLAKE3 verification");
        anyhow::bail!(t!("receiver.chunk_hash", index = chunk_index));
    }
    Ok(payload)
}
