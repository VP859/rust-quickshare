use chrono::Utc;
use rqs_p2p::config::{Config, PeerInfo};
use rqs_p2p::node::{NodeEngine, NodeTicket};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use tracing::info;

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
pub async fn discover_devices() -> Result<Vec<TrustedPeer>, String> {
    let discovered = vec![
        TrustedPeer {
            node_id: "5x9a2f11bc890e11".into(),
            alias: "Telefon (LAN)".into(),
            auto_accept: false,
            is_online: true,
        },
    ];
    Ok(discovered)
}

#[tauri::command]
pub async fn get_trusted_peers(state: State<'_, AppState>) -> Result<Vec<TrustedPeer>, String> {
    let config = state.config.lock().await;
    let list = config
        .peers
        .iter()
        .map(|(node_id, info)| TrustedPeer {
            node_id: node_id.clone(),
            alias: info.alias.clone(),
            auto_accept: info.auto_accept,
            is_online: true,
        })
        .collect();
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
    let _ = config.save().map_err(|e| e.to_string())?;

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
    let _ = config.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn send_file_offer(
    file_path: String,
    target_node_id: Option<String>,
    _app: AppHandle,
) -> Result<OfferResult, String> {
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err(format!("Plik nie istnieje: {}", file_path));
    }

    let file_name = path.file_name().unwrap_or_default().to_string_lossy();

    if let Some(target) = target_node_id {
        info!("Wysyłanie bezpośredniej oferty pliku {:?} do {}", path, target);
        Ok(OfferResult {
            message: format!("Wysłano ofertę pliku „{}” do węzła {}", file_name, target),
            ticket: None,
        })
    } else {
        let generated_ticket = format!("rqs://ticket-{}", blake3::hash(file_name.as_bytes()));
        info!("Wygenerowano ticket udostępniania dla pliku {:?}: {}", path, generated_ticket);
        
        Ok(OfferResult {
            message: format!("Plik „{}” jest gotowy do pobrania.", file_name),
            ticket: Some(generated_ticket),
        })
    }
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