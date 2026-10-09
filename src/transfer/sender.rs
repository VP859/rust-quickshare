use anyhow::{Context, Result, anyhow};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tracing::info;

use crate::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    ChunkHeader, DEFAULT_CHUNK_SIZE, TransferComplete, TransferDecision, TransferOffer,
};
use crate::transfer::hash::compute_file_hash;
use crate::transfer::progress::create_progress_bar;

pub async fn run_sender(file_path: &Path) -> Result<()> {
    if !file_path.exists() || !file_path.is_file() {
        anyhow::bail!(
            "Podany plik nie istnieje lub nie jest plikiem: {:?}",
            file_path
        );
    }

    let metadata = tokio::fs::metadata(file_path).await?;
    let file_size = metadata.len();
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("Nieprawidłowa nazwa pliku"))?
        .to_string();

    println!("Obliczanie haszu BLAKE3 pliku '{}'...", file_name);
    let file_hash = compute_file_hash(file_path).await?;

    let secret_key = load_or_generate_secret_key(None)?;
    let engine = NodeEngine::new(secret_key).await?;

    let addr = engine.node_addr().await?;
    let ticket = NodeTicket::new(addr);
    let ticket_str = ticket.serialize_to_string()?;

    println!("\n==================================================================");
    println!(" Węzeł nadawcy jest gotowy.");
    println!(" Udostępnij poniższy TICKET odbiorcy:");
    println!("\n {}", ticket_str);
    println!("==================================================================\n");

    info!("Oczekiwanie na połączenie przychodzące od odbiorcy...");
    let conn = engine.accept().await?;
    info!("Nawiązano połączenie wejściowe z odbiorcą.");

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("Nie udało się otworzyć strumienia dwukierunkowego")?;

    info!("Zaakceptowano strumień dwukierunkowy.");

    let offer = TransferOffer {
        file_name: file_name.clone(),
        file_size,
        chunk_size: DEFAULT_CHUNK_SIZE,
        file_hash,
    };
    write_msg(&mut send, &offer).await?;
    send.flush()
        .await
        .context("Błąd spłukiwania strumienia oferty")?;
    info!("Wysłano ofertę transferu pliku.");

    let decision: TransferDecision = read_msg(&mut recv).await?;
    let start_chunk_index = match decision {
        TransferDecision::Accept { start_chunk_index } => {
            info!(
                "Odbiorca zaakceptował transfer od chunka: {}",
                start_chunk_index
            );
            start_chunk_index
        }
        TransferDecision::Reject { reason } => {
            anyhow::bail!("Odbiorca odrzucił transfer. Powód: {}", reason);
        }
    };

    let mut file = File::open(file_path).await?;
    let start_offset = start_chunk_index * (DEFAULT_CHUNK_SIZE as u64);
    if start_offset < file_size {
        file.seek(SeekFrom::Start(start_offset)).await?;
    }

    let total_chunks = file_size.div_ceil(DEFAULT_CHUNK_SIZE as u64);
    let pb = create_progress_bar(file_size, &format!("Wysyłanie: {}", file_name));
    pb.set_position(start_offset.min(file_size));

    let mut buffer = vec![0u8; DEFAULT_CHUNK_SIZE as usize];

    for chunk_idx in start_chunk_index..total_chunks {
        let mut bytes_read = 0;
        while bytes_read < DEFAULT_CHUNK_SIZE as usize {
            let n = file.read(&mut buffer[bytes_read..]).await?;
            if n == 0 {
                break;
            }
            bytes_read += n;
        }

        if bytes_read == 0 {
            break;
        }

        let payload = &buffer[..bytes_read];
        let chunk_hash = *blake3::hash(payload).as_bytes();

        let header = ChunkHeader {
            chunk_index: chunk_idx,
            payload_len: bytes_read as u32,
            chunk_hash,
        };

        write_msg(&mut send, &header).await?;
        send.write_all(payload).await?;
        send.flush().await?;

        pb.inc(bytes_read as u64);
    }

    pb.finish_with_message("Przesyłanie bajtów zakończone. Oczekiwanie na weryfikację...");

    send.finish().context("Błąd zamykania strumienia nadawcy")?;

    info!("Oczekiwanie na potwierdzenie TransferComplete od odbiorcy...");
    let _complete: TransferComplete = read_msg(&mut recv).await?;
    println!("\n[SUKCES] Plik został pomyślnie odebrany i zweryfikowany przez odbiorcę!");

    engine.close().await?;

    Ok(())
}