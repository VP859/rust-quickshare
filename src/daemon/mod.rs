use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tracing::{info, warn};

use crate::config::Config;
use crate::node::{NodeEngine, load_or_generate_secret_key};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{ChunkHeader, TransferComplete, TransferDecision, TransferOffer};
use crate::transfer::hash::compute_file_hash;

pub async fn run_daemon(downloads_dir_opt: Option<PathBuf>) -> Result<()> {
    let config = Config::load()?;
    let downloads_dir = downloads_dir_opt
        .or_else(|| config.default_download_dir.clone())
        .or_else(dirs::download_dir)
        .unwrap_or_else(|| PathBuf::from("."));

    tokio::fs::create_dir_all(&downloads_dir).await?;

    let secret_key = load_or_generate_secret_key(None)?;
    let engine = NodeEngine::new(secret_key).await?;
    let node_id = engine.node_id().to_string();

    let addr = engine.node_addr().await?;
    let ticket_str = crate::node::NodeTicket::new(addr).serialize_to_string()?;

    println!("==================================================================");
    println!(" DEMON RQS UROCHOMIONY W TLE");
    println!(" Twój Node ID: {}", node_id);
    println!(" Twój TICKET : {}", ticket_str);
    println!(" Katalog pobierania: {:?}", downloads_dir);
    println!("==================================================================\n");

    loop {
        let conn = match engine.accept().await {
            Ok(c) => c,
            Err(e) => {
                warn!("Błąd akceptacji połączenia wejściowego: {:?}", e);
                continue;
            }
        };

        let remote_node_id = conn.remote_id().to_string();
        let peer_info = config.find_peer(&remote_node_id).cloned();
        let target_dir = downloads_dir.clone();

        tokio::spawn(async move {
            if let Err(e) =
                handle_daemon_incoming(conn, remote_node_id, peer_info, target_dir).await
            {
                warn!("Błąd podczas obsługi połączenia w demonie: {:?}", e);
            }
        });
    }
}

async fn handle_daemon_incoming(
    conn: iroh::endpoint::Connection,
    remote_node_id: String,
    peer_info: Option<crate::config::PeerInfo>,
    downloads_dir: PathBuf,
) -> Result<()> {
    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .context("Błąd otwarcia strumienia bi-directional")?;

    let offer: TransferOffer = read_msg(&mut recv).await?;
    let alias = peer_info
        .as_ref()
        .map(|p| p.alias.as_str())
        .unwrap_or("Nieznane Urządzenie");

    info!(
        "Oferta transferu od '{}' ({}): {} ({} bajtów)",
        alias, remote_node_id, offer.file_name, offer.file_size
    );

    let is_auto_accept = peer_info.as_ref().map(|p| p.auto_accept).unwrap_or(false);

    if !is_auto_accept {
        println!(
            "\n[OFERTA] Otrzymano plik '{}' ({:.2} MB) od '{}' ({}).",
            offer.file_name,
            offer.file_size as f64 / 1_048_576.0,
            alias,
            remote_node_id
        );
        print!("Czy chcesz odebrać plik? [y/N]: ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            let decision = TransferDecision::Reject {
                reason: "Użytkownik odrzucił połączenie w konsoli demona".to_string(),
            };
            write_msg(&mut send, &decision).await?;
            println!("[ODRZUCONO] Transfer został odrzucony.");
            return Ok(());
        }
    } else {
        info!(
            "Automatyczna akceptacja (Auto-Accept) dla zaufanego urządzenia: {}",
            alias
        );
    }

    // Pobieranie pliku
    let dest_path = downloads_dir.join(&offer.file_name);
    let part_path = downloads_dir.join(format!("{}.part", offer.file_name));

    let mut start_chunk_index: u64 = 0;
    if part_path.exists() {
        let metadata = tokio::fs::metadata(&part_path).await?;
        let existing_bytes = metadata.len();
        let valid_chunks = existing_bytes / (offer.chunk_size as u64);
        let valid_bytes = valid_chunks * (offer.chunk_size as u64);

        if existing_bytes != valid_bytes {
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&part_path)
                .await?;
            file.set_len(valid_bytes).await?;
        }
        start_chunk_index = valid_chunks;
    }

    write_msg(&mut send, &TransferDecision::Accept { start_chunk_index }).await?;

    let mut part_file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&part_path)
        .await?;

    let start_offset = start_chunk_index * (offer.chunk_size as u64);
    part_file.seek(SeekFrom::Start(start_offset)).await?;

    let total_chunks = offer.file_size.div_ceil(offer.chunk_size as u64);

    for expected_idx in start_chunk_index..total_chunks {
        let header: ChunkHeader = read_msg(&mut recv).await?;
        if header.chunk_index != expected_idx {
            anyhow::bail!("Niezgodność indeksów chunków");
        }

        let mut payload = vec![0u8; header.payload_len as usize];
        recv.read_exact(&mut payload).await?;

        if *blake3::hash(&payload).as_bytes() != header.chunk_hash {
            anyhow::bail!("Błąd spójności chunka BLAKE3");
        }

        part_file.write_all(&payload).await?;
        part_file.flush().await?;
    }

    let file_hash = compute_file_hash(&part_path).await?;
    if file_hash != offer.file_hash {
        anyhow::bail!("Niezgodność końcowego haszu pliku BLAKE3");
    }

    tokio::fs::rename(&part_path, &dest_path).await?;
    write_msg(&mut send, &TransferComplete).await?;

    println!("\n[SUKCES] Zakończono pobieranie: {:?}", dest_path);
    Ok(())
}
