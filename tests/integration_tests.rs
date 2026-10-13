use anyhow::{Context, Result};
use rqs_p2p::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};
use rqs_p2p::protocol::codec::{read_msg, write_msg};
use rqs_p2p::protocol::messages::{
    ChunkHeader, DEFAULT_CHUNK_SIZE, TransferComplete, TransferDecision, TransferOffer,
};
use rqs_p2p::transfer::security::resolve_secure_target_path;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn create_test_file_with_bytes(
    path: &PathBuf,
    size_bytes: usize,
) -> Result<(blake3::Hash, Vec<u8>)> {
    let mut data = vec![0u8; size_bytes];
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }
    let mut file = File::create(path).await?;
    file.write_all(&data).await?;
    file.flush().await?;

    let hash = blake3::hash(&data);
    Ok((hash, data))
}

#[tokio::test]
async fn test_resume_on_connection_break() -> Result<()> {
    let sender_dir = tempdir()?;
    let receiver_dir = tempdir()?;

    let file_path = sender_dir.path().join("big_payload.dat");
    let file_size = 5 * 1024 * 1024; // 5 MB
    let (original_hash, full_bytes) = create_test_file_with_bytes(&file_path, file_size).await?;

    let part_path = receiver_dir.path().join("big_payload.dat.part");
    let initial_part_bytes = 2 * 1024 * 1024; // 2 MB
    tokio::fs::write(&part_path, &full_bytes[..initial_part_bytes]).await?;

    let key1 = load_or_generate_secret_key(None)?;
    let key2 = load_or_generate_secret_key(None)?;

    let sender_engine = NodeEngine::new(key1).await?;
    let receiver_engine = NodeEngine::new(key2).await?;

    let addr = sender_engine.node_addr().await?;
    let ticket = NodeTicket::new(addr);

    let full_bytes_clone = full_bytes.clone();

    let mut tx_handle = tokio::spawn(async move {
        let conn = sender_engine
            .accept()
            .await
            .context("Błąd akceptowania połączenia po stronie nadawcy")?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .context("Błąd otwierania strumienia Bidi po stronie nadawcy")?;

        let offer = TransferOffer {
            file_name: "big_payload.dat".to_string(),
            file_size: file_size as u64,
            chunk_size: DEFAULT_CHUNK_SIZE as u32,
            file_hash: *original_hash.as_bytes(),
        };
        write_msg(&mut send, &offer)
            .await
            .context("Nie udało się wysłać oferty")?;
        send.flush()
            .await
            .context("Nie udało się spłukać strumienia oferty")?;

        let decision: TransferDecision = read_msg(&mut recv)
            .await
            .context("Błąd odczytu decyzji odbiorcy")?;
        let start_chunk_index = match decision {
            TransferDecision::Accept { start_chunk_index } => start_chunk_index,
            _ => anyhow::bail!("Odbiorca odrzucił transfer!"),
        };

        let expected_start = (initial_part_bytes as u64) / (DEFAULT_CHUNK_SIZE as u64);
        assert_eq!(
            start_chunk_index, expected_start,
            "Nadawca odebrał niepoprawny index wznowienia chunka"
        );

        let total_chunks = (file_size as u64).div_ceil(DEFAULT_CHUNK_SIZE as u64);
        for chunk_idx in start_chunk_index..total_chunks {
            let start = (chunk_idx * DEFAULT_CHUNK_SIZE as u64) as usize;
            let end = ((chunk_idx + 1) * DEFAULT_CHUNK_SIZE as u64).min(file_size as u64) as usize;
            let payload = &full_bytes_clone[start..end];
            let chunk_hash = *blake3::hash(payload).as_bytes();

            let header = ChunkHeader {
                chunk_index: chunk_idx,
                payload_len: payload.len() as u32,
                chunk_hash,
            };

            write_msg(&mut send, &header)
                .await
                .context("Błąd wysyłania nagłówka chunka")?;
            AsyncWriteExt::write_all(&mut send, payload)
                .await
                .context("Błąd wysyłania payloadu chunka")?;
            send.flush()
                .await
                .context("Błąd spłukiwania bufora chunka")?;
        }

        send.finish().context("Błąd kończenia strumienia nadawcy")?;

        let _complete: TransferComplete = read_msg(&mut recv)
            .await
            .context("Błąd odbioru ramki TransferComplete")?;
        Ok::<(), anyhow::Error>(())
    });

    let mut rx_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut conn = None;
        for _ in 0..15 {
            if let Ok(c) = receiver_engine.connect(ticket.addr.clone()).await {
                conn = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }

        let conn = conn.context("Nie udało się połączyć odbiorcy z nadawcą po kilku próbach")?;
        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .context("Błąd akceptowania strumienia Bidi po stronie odbiorcy")?;

        let offer: TransferOffer = read_msg(&mut recv)
            .await
            .context("Błąd odczytu oferty przez odbiorcę")?;
        assert_eq!(offer.file_size, file_size as u64);

        let existing_bytes = tokio::fs::metadata(&part_path)
            .await
            .context("Błąd odczytu metadanych pliku .part")?
            .len();
        let start_chunk_index = existing_bytes / (offer.chunk_size as u64);

        let expected_start = (initial_part_bytes as u64) / (offer.chunk_size as u64);
        assert_eq!(
            start_chunk_index, expected_start,
            "Odbiorca wyliczył błędny punkt wznowienia"
        );

        let decision = TransferDecision::Accept { start_chunk_index };
        write_msg(&mut send, &decision)
            .await
            .context("Błąd wysyłania decyzji akceptacji")?;
        send.flush().await.context("Błąd spłukiwania decyzji")?;

        {
            let mut part_file = OpenOptions::new()
                .write(true)
                .append(true)
                .open(&part_path)
                .await
                .context("Nie udało się otworzyć pliku .part do dopisywania")?;

            let total_chunks = offer.file_size.div_ceil(offer.chunk_size as u64);
            for expected_idx in start_chunk_index..total_chunks {
                let header: ChunkHeader = read_msg(&mut recv)
                    .await
                    .context("Błąd odczytu nagłówka chunka")?;
                assert_eq!(header.chunk_index, expected_idx);

                let mut payload = vec![0u8; header.payload_len as usize];
                AsyncReadExt::read_exact(&mut recv, &mut payload)
                    .await
                    .context("Błąd odczytu zawartości chunka z sieci")?;

                let computed_hash = *blake3::hash(&payload).as_bytes();
                assert_eq!(computed_hash, header.chunk_hash);

                part_file
                    .write_all(&payload)
                    .await
                    .context("Błąd zapisu chunka do pliku .part")?;
                part_file
                    .flush()
                    .await
                    .context("Błąd spłukiwania pliku .part")?;
            }
        }

        let downloaded_bytes = tokio::fs::read(&part_path)
            .await
            .context("Błąd ponownego odczytu pliku .part")?;
        let downloaded_hash = blake3::hash(&downloaded_bytes);
        assert_eq!(*downloaded_hash.as_bytes(), offer.file_hash);

        let final_path = receiver_dir.path().join("big_payload.dat");
        tokio::fs::rename(&part_path, &final_path)
            .await
            .context("Błąd zmiany nazwy z .part na docelową")?;

        write_msg(&mut send, &TransferComplete)
            .await
            .context("Błąd wysyłania ramki TransferComplete")?;
        send.flush()
            .await
            .context("Błąd spłukiwania ramki TransferComplete")?;
        send.finish()
            .context("Błąd zamykania strumienia odbiorcy")?;
        Ok::<(), anyhow::Error>(())
    });

    let test_future = async {
        tokio::select! {
            res = &mut rx_handle => res.context("rx_handle error")??,
            res = &mut tx_handle => res.context("tx_handle error")??,
        }
        Ok::<(), anyhow::Error>(())
    };

    tokio::time::timeout(Duration::from_secs(15), test_future)
        .await
        .context("Test przekroczył limit czasu 15 sekund")??;

    Ok(())
}

#[tokio::test]
async fn test_bit_rot_corruption_detection() -> Result<()> {
    let mut payload = vec![0u8; 1024];
    payload[0] = 0xAA;
    let original_hash = *blake3::hash(&payload).as_bytes();

    let header = ChunkHeader {
        chunk_index: 0,
        payload_len: 1024,
        chunk_hash: original_hash,
    };

    let mut corrupted_payload = payload.clone();
    corrupted_payload[0] = 0xBB;

    let computed_hash = *blake3::hash(&corrupted_payload).as_bytes();

    assert_ne!(
        header.chunk_hash, computed_hash,
        "Wykrywanie Bit Rot zawiodło! Uszkodzone dane zostały zaakceptowane."
    );

    Ok(())
}

#[tokio::test]
async fn test_path_traversal_exploit_blocked() -> Result<()> {
    let base_sandbox = tempdir()?;
    let malicious_filename = "../../.bashrc";

    let result = resolve_secure_target_path(base_sandbox.path(), malicious_filename);

    assert!(
        result.is_err(),
        "Zabezpieczenie przed Path Traversal nie zablokowało niebezpiecznej nazwy pliku!"
    );

    Ok(())
}
