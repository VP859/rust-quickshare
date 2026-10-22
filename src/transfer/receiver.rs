use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream};
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
use crate::transfer::hash::compute_file_hash;
use crate::transfer::security::{
    resolve_secure_target_path, sanitize_filename, unique_destination,
};

/// Odczytuje i weryfikuje ofertę (limity rozmiaru, bezpieczna nazwa pliku).
pub async fn read_offer(channel: &mut Channel) -> Result<TransferOffer> {
    let offer: TransferOffer = read_msg(&mut channel.recv).await?;
    offer.validate()?;
    sanitize_filename(&offer.file_name)?;
    Ok(offer)
}

pub async fn reject(mut channel: Channel, reason: &str) -> Result<()> {
    let decision = TransferDecision::Reject {
        reason: reason.to_string(),
    };
    write_msg(&mut channel.send, &decision).await?;
    channel.finish().await;
    Ok(())
}

/// Ścieżka pliku tymczasowego. Fragment haszu w nazwie gwarantuje, że wznowienie
/// nigdy nie doklei danych innego pliku o tej samej nazwie.
pub fn part_path_for(dest_dir: &Path, offer: &TransferOffer) -> Result<PathBuf> {
    let safe = sanitize_filename(&offer.file_name)?;
    Ok(resolve_secure_target_path(
        dest_dir,
        &format!("{safe}.{}.part", short_hex(&offer.file_hash[..6])),
    )?)
}

/// Strona odbierająca protokołu. Zapisuje do `.part`, wznawia od ostatniego pełnego
/// chunka, weryfikuje BLAKE3 każdego chunka i całego pliku, po czym zmienia nazwę
/// na docelową (bez nadpisywania istniejących plików).
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
        .with_context(|| format!("Nie można utworzyć katalogu {:?}", dest_dir))?;
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
    if start_chunk_index > 0 {
        info!(
            "Wznawianie '{}' od chunka #{} ({} B już na dysku)",
            safe_name, start_chunk_index, valid_bytes
        );
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
        .with_context(|| format!("Nie można otworzyć pliku tymczasowego {:?}", part_path))?;
    part_file.set_len(valid_bytes).await?;
    part_file.seek(SeekFrom::Start(valid_bytes)).await?;
    progress(valid_bytes);

    receive_chunks(
        conn,
        offer,
        start_chunk_index,
        valid_bytes,
        &mut part_file,
        progress,
    )
    .await?;
    part_file.sync_all().await?;
    drop(part_file);

    if compute_file_hash(&part_path).await? != offer.file_hash {
        // Uszkodzony plik nie może zostać wznowiony – usuwamy go.
        let _ = tokio::fs::remove_file(&part_path).await;
        let reason = "Niezgodność końcowego haszu BLAKE3 całego pliku".to_string();
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
        .context("Błąd podczas zmiany nazwy pliku .part na docelową")?;

    write_msg(&mut channel.send, &TransferResult::Verified).await?;
    channel.finish().await;
    info!("Odebrano i zweryfikowano: {:?}", final_path);
    Ok(final_path)
}

/// Przerywa zadanie przy upuszczeniu (np. gdy transfer kończy się błędem).
struct ChunkTask(JoinHandle<Result<Vec<u8>>>);

impl Drop for ChunkTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl ChunkTask {
    async fn join(mut self) -> Result<Vec<u8>> {
        (&mut self.0)
            .await
            .context("Zadanie odbioru chunka zostało przerwane")?
    }
}

/// Chunki są odczytywane równolegle, ale zapisywane ściśle po kolei, dzięki czemu
/// `.part` zawsze zawiera ciągły prefiks pliku (podstawa wznawiania).
async fn receive_chunks(
    conn: &Connection,
    offer: &TransferOffer,
    start_chunk_index: u64,
    mut done: u64,
    part_file: &mut File,
    progress: ProgressFn<'_>,
) -> Result<()> {
    let mut pending: VecDeque<ChunkTask> = VecDeque::with_capacity(CHUNK_WINDOW);

    for chunk_index in start_chunk_index..offer.total_chunks() {
        if pending.len() >= CHUNK_WINDOW
            && let Some(task) = pending.pop_front()
        {
            let payload = task.join().await?;
            part_file.write_all(&payload).await?;
            done += payload.len() as u64;
            progress(done);
        }

        let expected_len = offer
            .chunk_len(chunk_index)
            .context("Błąd wyliczenia długości chunka")?;
        let stream = conn
            .accept_uni()
            .await
            .context("Połączenie przerwane podczas odbioru chunków")?;
        pending.push_back(ChunkTask(tokio::spawn(read_chunk(
            stream,
            chunk_index,
            expected_len,
        ))));
    }

    while let Some(task) = pending.pop_front() {
        let payload = task.join().await?;
        part_file.write_all(&payload).await?;
        done += payload.len() as u64;
        progress(done);
    }
    part_file.flush().await?;
    Ok(())
}

async fn read_chunk(
    mut stream: RecvStream,
    chunk_index: u64,
    expected_len: u32,
) -> Result<Vec<u8>> {
    let header: ChunkHeader = read_msg(&mut stream).await?;
    if header.chunk_index != chunk_index {
        anyhow::bail!(
            "Niezgodność kolejności chunków: oczekiwano {}, odebrano {}",
            chunk_index,
            header.chunk_index
        );
    }
    if header.payload_len != expected_len {
        anyhow::bail!(
            "Chunk #{} ma nieprawidłową długość {} (oczekiwano {})",
            chunk_index,
            header.payload_len,
            expected_len
        );
    }

    let mut payload = vec![0u8; expected_len as usize];
    stream
        .read_exact(&mut payload)
        .await
        .context("Błąd odczytu danych chunka z sieci")?;

    if *blake3::hash(&payload).as_bytes() != header.chunk_hash {
        warn!("Chunk #{} uszkodzony (BLAKE3)", chunk_index);
        anyhow::bail!(
            "Błąd spójności chunka #{}. Hash BLAKE3 nie zgadza się!",
            chunk_index
        );
    }
    Ok(payload)
}
