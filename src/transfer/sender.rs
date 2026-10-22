use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::task::JoinSet;
use tracing::info;

use super::{Channel, ProgressFn};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    CHUNK_WINDOW, ChunkHeader, DEFAULT_CHUNK_SIZE, Hello, TransferDecision, TransferOffer,
    TransferResult,
};
use crate::transfer::hash::compute_file_hash;
use crate::transfer::security::sanitize_filename;

/// Plik gotowy do wysłania – z policzonym haszem BLAKE3 (liczonym raz, także przy wznowieniu).
#[derive(Debug, Clone)]
pub struct PreparedFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub hash: [u8; 32],
}

pub async fn prepare_file(path: &Path) -> Result<PreparedFile> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("Nie można odczytać pliku {:?}", path))?;
    if !metadata.is_file() {
        anyhow::bail!("Podana ścieżka nie jest plikiem: {:?}", path);
    }
    let raw_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = sanitize_filename(&raw_name).unwrap_or_else(|_| "plik".to_string());
    let hash = compute_file_hash(path).await?;
    Ok(PreparedFile {
        path: path.to_path_buf(),
        name,
        size: metadata.len(),
        hash,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    Rejected(String),
}

/// Otwiera nowy strumień kontrolny (`Hello::Push`) i wysyła plik.
pub async fn push_file(
    conn: &Connection,
    file: &PreparedFile,
    progress: ProgressFn<'_>,
) -> Result<SendOutcome> {
    let mut channel = Channel::open(conn, &Hello::Push).await?;
    let outcome = send_on_channel(conn, &mut channel, file, progress).await?;
    channel.finish().await;
    Ok(outcome)
}

/// Strona wysyłająca protokołu: oferta → decyzja → chunki na osobnych strumieniach
/// jednokierunkowych (do [`CHUNK_WINDOW`] równolegle) → werdykt odbiorcy.
pub async fn send_on_channel(
    conn: &Connection,
    channel: &mut Channel,
    file: &PreparedFile,
    progress: ProgressFn<'_>,
) -> Result<SendOutcome> {
    let offer = TransferOffer {
        file_name: file.name.clone(),
        file_size: file.size,
        chunk_size: DEFAULT_CHUNK_SIZE,
        file_hash: file.hash,
    };
    write_msg(&mut channel.send, &offer).await?;

    let start_chunk_index = match read_msg(&mut channel.recv).await? {
        TransferDecision::Accept { start_chunk_index } => start_chunk_index,
        TransferDecision::Reject { reason } => return Ok(SendOutcome::Rejected(reason)),
    };
    let total_chunks = offer.total_chunks();
    if start_chunk_index > total_chunks {
        anyhow::bail!("Odbiorca zażądał nieistniejącego chunka #{start_chunk_index}");
    }
    info!(
        "Wysyłanie '{}' od chunka {}/{}",
        file.name, start_chunk_index, total_chunks
    );

    let mut source = File::open(&file.path).await?;
    let start_offset = start_chunk_index * offer.chunk_size as u64;
    source.seek(SeekFrom::Start(start_offset)).await?;
    let mut done = start_offset.min(file.size);
    progress(done);

    // Upuszczenie JoinSet (np. przy błędzie) przerywa wszystkie zadania w locie.
    let mut in_flight: JoinSet<Result<u32>> = JoinSet::new();
    for chunk_index in start_chunk_index..total_chunks {
        while in_flight.len() >= CHUNK_WINDOW {
            done += join_next(&mut in_flight).await? as u64;
            progress(done);
        }

        let len = offer
            .chunk_len(chunk_index)
            .context("Błąd wyliczenia długości chunka")?;
        let mut payload = vec![0u8; len as usize];
        source
            .read_exact(&mut payload)
            .await
            .context("Plik źródłowy zmienił się w trakcie wysyłania")?;

        // Strumienie są otwierane po kolei, więc odbiorca przyjmuje je w kolejności chunków.
        let mut stream = conn
            .open_uni()
            .await
            .context("Nie udało się otworzyć strumienia chunka")?;
        in_flight.spawn(async move {
            let header = ChunkHeader {
                chunk_index,
                payload_len: len,
                chunk_hash: *blake3::hash(&payload).as_bytes(),
            };
            write_msg(&mut stream, &header).await?;
            stream.write_all(&payload).await?;
            stream.finish()?;
            Ok(len)
        });
    }
    while !in_flight.is_empty() {
        done += join_next(&mut in_flight).await? as u64;
        progress(done);
    }

    match read_msg(&mut channel.recv).await? {
        TransferResult::Verified => Ok(SendOutcome::Delivered),
        TransferResult::Failed { reason } => {
            anyhow::bail!("Odbiorca odrzucił plik po weryfikacji: {reason}")
        }
    }
}

async fn join_next(set: &mut JoinSet<Result<u32>>) -> Result<u32> {
    set.join_next()
        .await
        .context("Brak zadań do oczekiwania")?
        .context("Zadanie wysyłania chunka zostało przerwane")?
}
