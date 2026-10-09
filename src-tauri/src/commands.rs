use anyhow::{Result, anyhow};
use rqs_p2p::config::Config;
use rqs_p2p::node::{NodeTicket, load_or_generate_secret_key};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Clone)]
pub struct NodeInfoResponse {
    pub node_id: String,
    pub ticket: String,
    pub relay_status: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TransferProgressEvent {
    pub transfer_id: String,
    pub file_name: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub speed_mbps: f64,
    pub status: String,
}

pub struct AppState {
    pub config: Mutex<Config>,
    pub pending_responses: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
}

fn validate_safe_path(path_str: &str) -> Result<PathBuf> {
    if path_str.contains("..") {
        return Err(anyhow!(
            "Niedozwolona próba wyjścia poza katalog (Directory Traversal)"
        ));
    }
    let path = Path::new(path_str);
    if path.exists() {
        Ok(path.canonicalize()?)
    } else {
        Ok(path.to_path_buf())
    }
}

#[tauri::command]
pub async fn get_node_info() -> Result<NodeInfoResponse, String> {
    tokio::task::spawn_blocking(|| {
        let secret_key = load_or_generate_secret_key(None).map_err(|e| e.to_string())?;
        let public_key = secret_key.public();

        Ok(NodeInfoResponse {
            node_id: public_key.to_string(),
            ticket: format!("rqs-ticket-{}", public_key),
            relay_status: "Aktywny (euc1-1.relay.n0.iroh.link)".to_string(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn send_file_offer(
    app: AppHandle,
    filepath: String,
    _target_ticket: String,
) -> Result<String, String> {
    let safe_path = validate_safe_path(&filepath).map_err(|e| e.to_string())?;
    let transfer_id = uuid::Uuid::new_v4().to_string();

    let app_handle = app.clone();
    let tid = transfer_id.clone();
    let file_name = safe_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    tokio::spawn(async move {
        let _ = app_handle.emit(
            "transfer_progress",
            TransferProgressEvent {
                transfer_id: tid,
                file_name,
                bytes_transferred: 0,
                total_bytes: 100,
                speed_mbps: 0.0,
                status: "Nawiązywanie połączenia P2P...".to_string(),
            },
        );
    });

    Ok(transfer_id)
}

#[tauri::command]
pub async fn respond_to_offer(
    state: State<'_, AppState>,
    transfer_id: String,
    accept: bool,
) -> Result<(), String> {
    let mut responses = state.pending_responses.lock().await;
    if let Some(sender) = responses.remove(&transfer_id) {
        let _ = sender.send(accept);
        Ok(())
    } else {
        Err("Nie znaleziono oczekującej prośby o transfer".to_string())
    }
}

#[tauri::command]
pub async fn get_trusted_peers(
    state: State<'_, AppState>,
) -> Result<Vec<rqs_p2p::config::PeerInfo>, String> {
    let cfg = state.config.lock().await;
    Ok(cfg.peers.values().cloned().collect())
}

#[tauri::command]
pub async fn add_trusted_peer(
    state: State<'_, AppState>,
    ticket: String,
    alias: String,
    auto_accept: bool,
) -> Result<(), String> {
    let parsed_ticket = NodeTicket::deserialize_from_str(&ticket).map_err(|e| e.to_string())?;
    let mut cfg = state.config.lock().await;
    cfg.add_peer(parsed_ticket.addr.id.to_string(), alias, auto_accept);
    cfg.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn remove_trusted_peer(state: State<'_, AppState>, target: String) -> Result<(), String> {
    let mut cfg = state.config.lock().await;
    if cfg.remove_peer(&target) {
        cfg.save().map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Nie znaleziono wskazanego urządzenia na liście".to_string())
    }
}
