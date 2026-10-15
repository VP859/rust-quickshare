use anyhow::Context;
use chrono::Utc;
use rqs_p2p::config::{Config, PeerInfo};
use rqs_p2p::node::{NodeEngine, NodeTicket, load_or_generate_secret_key};
use rqs_p2p::protocol::codec::{read_msg, write_msg};
use rqs_p2p::protocol::messages::{
    ChunkHeader, DEFAULT_CHUNK_SIZE, TransferComplete, TransferDecision, TransferOffer,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::error;

pub struct AppState {
    pub config: Mutex<Config>,
    pub pending_responses: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub ticket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub node_id: String,
    pub alias: String,
    pub auto_accept: bool,
    pub is_online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferResult {
    pub message: String,
    pub ticket: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressPayload {
    pub message: String,
    pub step: String,
}

#[tauri::command]
pub async fn pick_file_dialog(app: AppHandle) -> Result<Option<String>, String> {
    let file_path = app.dialog().file().blocking_pick_file();
    Ok(file_path.map(|p| p.to_string()))
}

#[tauri::command]
pub async fn get_node_info(state: State<'_, AppState>) -> Result<NodeInfo, String> {
    let _config = state.config.lock().await;
    Ok(NodeInfo {
        node_id: "local-node-id".into(),
        ticket: "rqs://ticket-example".into(),
    })
}

#[tauri::command]
pub async fn discover_devices(state: State<'_, AppState>) -> Result<Vec<TrustedPeer>, String> {
    let config = state.config.lock().await;
    let mut list = Vec::new();

    for (node_id, info) in config.peers.iter() {
        list.push(TrustedPeer {
            node_id: node_id.clone(),
            alias: info.alias.clone(),
            auto_accept: info.auto_accept,
            is_online: true,
        });
    }

    Ok(list)
}

#[tauri::command]
pub async fn get_trusted_peers(state: State<'_, AppState>) -> Result<Vec<TrustedPeer>, String> {
    let config = state.config.lock().await;
    let mut list = Vec::new();

    for (node_id, info) in config.peers.iter() {
        list.push(TrustedPeer {
            node_id: node_id.clone(),
            alias: info.alias.clone(),
            auto_accept: info.auto_accept,
            is_online: true,
        });
    }

    Ok(list)
}

#[tauri::command]
pub async fn add_trusted_peer(
    ticket_or_id: String,
    alias: String,
    auto_accept: bool,
    state: State<'_, AppState>,
) -> Result<TrustedPeer, String> {
    let clean_input = ticket_or_id.trim();

    if clean_input.is_empty() {
        return Err("Ticket lub Node ID nie może być pusty!".into());
    }

    let node_id = if let Ok(ticket) = NodeTicket::deserialize_from_str(clean_input) {
        ticket.addr.id.to_string()
    } else if clean_input.len() >= 16 {
        clean_input.to_string()
    } else {
        return Err("Nieprawidłowy format Ticketu lub Node ID!".into());
    };

    let mut config = state.config.lock().await;

    let peer_alias = if alias.trim().is_empty() {
        format!("Węzeł {}", &node_id[..node_id.len().min(8)])
    } else {
        alias.trim().to_string()
    };

    let peer_info = PeerInfo {
        alias: peer_alias.clone(),
        auto_accept,
        added_at: Utc::now(),
    };

    config.peers.insert(node_id.clone(), peer_info);
    config.save().map_err(|e| e.to_string())?;

    Ok(TrustedPeer {
        node_id,
        alias: peer_alias,
        auto_accept,
        is_online: true,
    })
}

#[tauri::command]
pub async fn remove_trusted_peer(
    node_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.lock().await;
    config.peers.remove(&node_id);
    config.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn send_file_offer(
    file_path: String,
    target_node_id: Option<String>,
    app: AppHandle,
) -> Result<OfferResult, String> {
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err(format!("Plik nie istnieje: {}", file_path));
    }

    let file_size = std::fs::metadata(&path)
        .map_err(|e| format!("Błąd odczytu metadanych: {e}"))?
        .len();

    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let _ = app.emit(
        "transfer-progress",
        ProgressPayload {
            message: format!("Obliczanie sumy kontrolnej BLAKE3 dla {}", file_name),
            step: "hashing".into(),
        },
    );

    let file_bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Błąd odczytu pliku z dysku: {e}"))?;
    let file_hash = *blake3::hash(&file_bytes).as_bytes();

    let secret_key =
        load_or_generate_secret_key(None).map_err(|e| format!("Błąd klucza Iroh: {e}"))?;
    let engine = NodeEngine::new(secret_key)
        .await
        .map_err(|e| format!("Błąd uruchamiania silnika P2P: {e}"))?;

    if let Some(target_raw) = target_node_id {
        let target_addr = if let Ok(ticket) = NodeTicket::deserialize_from_str(&target_raw) {
            ticket.addr
        } else if let Ok(endpoint_id) = iroh::EndpointId::from_str(&target_raw) {
            iroh::EndpointAddr {
                id: endpoint_id,
                addrs: std::collections::BTreeSet::new(),
            }
        } else {
            return Err("Nieprawidłowy adres docelowego węzła!".into());
        };

        let app_clone = app.clone();
        tokio::spawn(async move {
            let _ = app_clone.emit(
                "transfer-progress",
                ProgressPayload {
                    message: "Nawiązywanie połączenia P2P...".into(),
                    step: "connecting".into(),
                },
            );

            match engine.connect(target_addr).await {
                Ok(conn) => {
                    if let Err(e) = stream_chunks_to_conn(
                        conn,
                        file_name,
                        file_size,
                        file_hash,
                        file_bytes,
                        app_clone.clone(),
                    )
                    .await
                    {
                        error!("Błąd przesyłania danych: {e}");
                    }
                }
                Err(e) => {
                    let _ = app_clone.emit(
                        "transfer-progress",
                        ProgressPayload {
                            message: format!("Nie udało się połączyć z urządzeniem: {e}"),
                            step: "idle".into(),
                        },
                    );
                }
            }
        });

        Ok(OfferResult {
            message: "Rozpoczęto próbę wysyłania do wybranego urządzenia.".into(),
            ticket: None,
        })
    } else {
        let addr = engine
            .node_addr()
            .await
            .map_err(|e| format!("Błąd pobierania adresu węzła: {e}"))?;
        let ticket = NodeTicket::new(addr);
        let ticket_str = ticket.serialize_to_string();

        let app_clone = app.clone();
        tokio::spawn(async move {
            let _ = app_clone.emit(
                "transfer-progress",
                ProgressPayload {
                    message: "Oczekiwanie na przychodzące połączenie...".into(),
                    step: "connecting".into(),
                },
            );

            match engine.accept().await {
                Ok(conn) => {
                    if let Err(e) = stream_chunks_to_conn(
                        conn,
                        file_name,
                        file_size,
                        file_hash,
                        file_bytes,
                        app_clone.clone(),
                    )
                    .await
                    {
                        error!("Błąd w sesji odbioru: {e}");
                    }
                }
                Err(e) => {
                    let _ = app_clone.emit(
                        "transfer-progress",
                        ProgressPayload {
                            message: format!("Połączenie nie zostało nawiązane: {e}"),
                            step: "idle".into(),
                        },
                    );
                }
            }
        });

        Ok(OfferResult {
            message: "Plik gotowy do udostępnienia.".into(),
            ticket: Some(ticket_str.unwrap()),
        })
    }
}

async fn stream_chunks_to_conn(
    conn: iroh::endpoint::Connection,
    file_name: String,
    file_size: u64,
    file_hash: [u8; 32],
    file_bytes: Vec<u8>,
    app: AppHandle,
) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("Błąd otwierania strumienia dwukierunkowego")?;

    let offer = TransferOffer {
        file_name,
        file_size,
        chunk_size: DEFAULT_CHUNK_SIZE,
        file_hash,
    };
    write_msg(&mut send, &offer).await?;
    send.flush().await?;

    let decision: TransferDecision = read_msg(&mut recv).await?;
    let start_chunk_index = match decision {
        TransferDecision::Accept { start_chunk_index } => start_chunk_index,
        TransferDecision::Reject { .. } => {
            let _ = app.emit(
                "transfer-progress",
                ProgressPayload {
                    message: "Odbiorca odrzucił ofertę transferu.".into(),
                    step: "idle".into(),
                },
            );
            anyhow::bail!("Transfer został odrzucony przez odbiorcę.");
        }
    };

    let total_chunks = file_size.div_ceil(DEFAULT_CHUNK_SIZE as u64);

    for chunk_idx in start_chunk_index..total_chunks {
        let start = (chunk_idx * DEFAULT_CHUNK_SIZE as u64) as usize;
        let end = ((chunk_idx + 1) * DEFAULT_CHUNK_SIZE as u64).min(file_size) as usize;
        let payload = &file_bytes[start..end];
        let chunk_hash = *blake3::hash(payload).as_bytes();

        let header = ChunkHeader {
            chunk_index: chunk_idx,
            payload_len: payload.len() as u32,
            chunk_hash,
        };

        write_msg(&mut send, &header).await?;
        send.write_all(payload).await?;
        send.flush().await?;

        let progress_pct = (((chunk_idx + 1) as f32 / total_chunks as f32) * 100.0) as u32;
        let _ = app.emit(
            "transfer-progress",
            ProgressPayload {
                message: format!("Wysyłanie pliku... {}%", progress_pct),
                step: "transferring".into(),
            },
        );
    }

    send.finish().context("Błąd zamykania strumienia")?;

    let _complete: TransferComplete = read_msg(&mut recv).await?;

    let _ = app.emit(
        "transfer-progress",
        ProgressPayload {
            message: "Plik przesłany pomyślnie!".into(),
            step: "idle".into(),
        },
    );

    Ok(())
}

#[tauri::command]
pub async fn respond_to_offer(
    transfer_id: String,
    accept: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut pending = state.pending_responses.lock().await;
    if let Some(tx) = pending.remove(&transfer_id) {
        let _ = tx.send(accept);
    }
    Ok(())
}
