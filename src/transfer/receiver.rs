use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tracing::info;

use crate::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{ChunkHeader, TransferComplete, TransferDecision, TransferOffer};
use crate::transfer::hash::compute_file_hash;
use crate::transfer::progress::create_progress_bar;

pub async fn run_receiver(ticket_str: &str, output_path: Option<&Path>) -> Result<()> {
    let ticket =
        NodeTicket::deserialize_from_str(ticket_str).context("Nieprawidłowy bilet połączeniowy")?;

    let secret_key = load_or_generate_secret_key(None)?;
    let engine = NodeEngine::new(secret_key).await?;

    info!("Nawiązywanie połączenia z nadawcą...");
    let conn = engine.connect(ticket.addr).await?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("Nie udało się otworzyć strumienia dwukierunkowego")?;

    let offer: TransferOffer = read_msg(&mut recv).await?;
    info!(
        "Otrzymano ofertę: {} ({} bajtów)",
        offer.file_name, offer.file_size
    );

    let dest_path = match output_path {
        Some(p) => {
            if p.is_dir() {
                p.join(&offer.file_name)
            } else {
                p.to_path_buf()
            }
        }
        None => PathBuf::from(&offer.file_name),
    };

    let part_path = PathBuf::from(format!("{}.part", dest_path.display()));

    let mut start_chunk_index: u64 = 0;
    if part_path.exists() {
        let metadata = tokio::fs::metadata(&part_path).await?;
        let existing_bytes = metadata.len();
        let valid_chunks = existing_bytes / (offer.chunk_size as u64);
        let valid_bytes = valid_chunks * (offer.chunk_size as u64);

        if existing_bytes != valid_bytes {
            let file = OpenOptions::new().write(true).open(&part_path).await?;
            file.set_len(valid_bytes).await?;
        }

        start_chunk_index = valid_chunks;
        println!(
            "Wykryto częściowy plik. Wznowienie transferu od chunka #{} ({} MB)",
            start_chunk_index,
            valid_bytes / (1024 * 1024)
        );
    }

    let decision = TransferDecision::Accept { start_chunk_index };
    write_msg(&mut send, &decision).await?;

    let mut part_file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part_path)
        .await?;

    let start_offset = start_chunk_index * (offer.chunk_size as u64);
    part_file.seek(SeekFrom::Start(start_offset)).await?;

    let total_chunks = (offer.file_size + offer.chunk_size as u64 - 1) / (offer.chunk_size as u64);
    let pb = create_progress_bar(offer.file_size, &format!("Pobieranie: {}", offer.file_name));
    pb.set_position(start_offset.min(offer.file_size));

    for expected_chunk_idx in start_chunk_index..total_chunks {
        let header: ChunkHeader = read_msg(&mut recv).await?;

        if header.chunk_index != expected_chunk_idx {
            anyhow::bail!(
                "Niezgodność kolejności chunków: oczekiwano {}, odebrano {}",
                expected_chunk_idx,
                header.chunk_index
            );
        }

        let mut payload = vec![0u8; header.payload_len as usize];
        recv.read_exact(&mut payload)
            .await
            .context("Błąd odczytu danych chunka z sieci")?;

        let computed_hash = *blake3::hash(&payload).as_bytes();
        if computed_hash != header.chunk_hash {
            anyhow::bail!(
                "Błąd spójności chunka #{}. Hash BLAKE3 nie zgadza się!",
                header.chunk_index
            );
        }

        part_file.write_all(&payload).await?;
        part_file.flush().await?;

        pb.inc(payload.len() as u64);
    }

    pb.finish_with_message("Pobieranie zakończone. Weryfikacja końcowego haszu całego pliku...");

    let downloaded_file_hash = compute_file_hash(&part_path).await?;
    if downloaded_file_hash != offer.file_hash {
        anyhow::bail!("Niezgodność końcowego haszu BLAKE3 dla całego pliku! Plik uszkodzony.");
    }

    tokio::fs::rename(&part_path, &dest_path)
        .await
        .context("Błąd podczas zmiany nazwy pliku .part na docelową")?;

    write_msg(&mut send, &TransferComplete).await?;

    println!(
        "\n[SUKCES] Plik został odebrany i pomyślnie zweryfikowany: {:?}",
        dest_path
    );

    Ok(())
}
