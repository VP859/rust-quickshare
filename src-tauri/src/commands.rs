use qrcode::QrCode;
use qrcode::render::svg;
use rqs_p2p::config::Config;
use rqs_p2p::node::NodeTicket;
use rqs_p2p::transfer::sender::prepare_file;
use rqs_p2p::transfer::service;
use serde::Serialize;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::node::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Czas na sprawdzenie, czy urządzenie odpowiada.
const PING_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Serialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub ticket: String,
    pub qr_svg: String,
    pub download_dir: String,
}

#[derive(Serialize)]
pub struct TrustedPeer {
    pub node_id: String,
    pub alias: String,
    pub auto_accept: bool,
}

#[derive(Serialize)]
pub struct ShareInfo {
    pub file_name: String,
    pub ticket: String,
    pub qr_svg: String,
}

#[derive(Serialize)]
pub struct TicketInfo {
    pub node_id: String,
    pub has_share: bool,
}

#[derive(Clone, Serialize)]
struct TransferError {
    message: String,
}

fn qr_svg(text: &str) -> CmdResult<String> {
    let code = QrCode::new(text.as_bytes()).map_err(err)?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#0f172a"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

fn emit_error(app: &AppHandle, e: anyhow::Error) {
    let _ = app.emit(
        "transfer-error",
        TransferError {
            message: format!("{e:#}"),
        },
    );
}

#[tauri::command]
pub async fn get_node_info(state: State<'_, AppState>) -> CmdResult<NodeInfo> {
    let engine = state.engine().await?;
    let ticket = engine
        .ticket()
        .await
        .map_err(err)?
        .serialize_to_string()
        .map_err(err)?;
    Ok(NodeInfo {
        node_id: engine.node_id().to_string(),
        qr_svg: qr_svg(&ticket)?,
        ticket,
        download_dir: state.download_dir().await.display().to_string(),
    })
}

#[tauri::command]
pub async fn list_peers(state: State<'_, AppState>) -> CmdResult<Vec<TrustedPeer>> {
    let config = state.config.lock().await;
    let mut peers: Vec<_> = config
        .peers
        .iter()
        .map(|(id, p)| TrustedPeer {
            node_id: id.clone(),
            alias: p.alias.clone(),
            auto_accept: p.auto_accept,
        })
        .collect();
    peers.sort_by_key(|p| p.alias.to_lowercase());
    Ok(peers)
}

#[tauri::command]
pub async fn add_peer(
    input: String,
    alias: String,
    auto_accept: bool,
    state: State<'_, AppState>,
) -> CmdResult<String> {
    let mut config = state.config.lock().await;
    let node_id = config
        .add_from_input(&input, &alias, auto_accept)
        .map_err(err)?;
    if let Ok(engine) = state.engine().await
        && node_id == engine.node_id().to_string()
    {
        config.peers.remove(&node_id);
        return Err("To jest TICKET tego urządzenia".into());
    }
    config.save().map_err(err)?;
    Ok(node_id)
}

#[tauri::command]
pub async fn update_peer(
    node_id: String,
    alias: Option<String>,
    auto_accept: Option<bool>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    let peer = config
        .peers
        .get_mut(&node_id)
        .ok_or("Nie znaleziono urządzenia")?;
    if let Some(alias) = alias
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
    {
        peer.alias = alias;
    }
    if let Some(auto_accept) = auto_accept {
        peer.auto_accept = auto_accept;
    }
    config.save().map_err(err)
}

#[tauri::command]
pub async fn remove_peer(node_id: String, state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    config.peers.remove(&node_id);
    config.save().map_err(err)
}

#[tauri::command]
pub async fn peer_online(node_id: String, state: State<'_, AppState>) -> CmdResult<bool> {
    let target = state
        .config
        .lock()
        .await
        .resolve_target(&node_id)
        .map_err(err)?;
    Ok(state.engine().await?.ping(target, PING_TIMEOUT).await)
}

/// Wysyła pliki do urządzenia (alias / Node ID / TICKET). Postęp przez zdarzenia `transfer`.
#[tauri::command]
pub async fn send_files(
    paths: Vec<String>,
    target: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    if paths.is_empty() {
        return Err("Nie wybrano plików".into());
    }
    let addr = state
        .config
        .lock()
        .await
        .resolve_target(&target)
        .map_err(err)?;
    let engine = state.engine().await?.clone();
    let mut local = Vec::with_capacity(paths.len());
    for p in &paths {
        local.push(crate::platform::local_path(&app, p).await?);
    }
    let paths = local;
    tauri::async_runtime::spawn(async move {
        let sink = |ev| {
            let _ = app.emit("transfer", ev);
        };
        if let Err(e) = service::send_files(&engine, addr, &paths, &sink).await {
            emit_error(&app, e);
        }
    });
    Ok(())
}

/// Udostępnia plik przez jednorazowy TICKET (odbiorca: skan QR lub `rqs receive`).
#[tauri::command]
pub async fn share_file(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<ShareInfo> {
    let engine = state.engine().await?;
    let local = crate::platform::local_path(&app, &path).await?;
    let file = prepare_file(&local).await.map_err(err)?;
    let token = service::new_share_token();
    let ticket = NodeTicket::with_share(engine.node_addr().await.map_err(err)?, token)
        .serialize_to_string()
        .map_err(err)?;
    let file_name = file.name.clone();
    state.shares.lock().map_err(err)?.insert(token, file);
    Ok(ShareInfo {
        file_name,
        qr_svg: qr_svg(&ticket)?,
        ticket,
    })
}

#[tauri::command]
pub fn inspect_ticket(ticket: String) -> CmdResult<TicketInfo> {
    if let Ok(t) = NodeTicket::deserialize_from_str(&ticket) {
        return Ok(TicketInfo {
            node_id: t.addr.id.to_string(),
            has_share: t.share.is_some(),
        });
    }
    let id = iroh::EndpointId::from_str(ticket.trim())
        .map_err(|_| "Nieprawidłowy TICKET lub Node ID".to_string())?;
    Ok(TicketInfo {
        node_id: id.to_string(),
        has_share: false,
    })
}

/// Pobiera plik z TICKETU udostępnienia.
#[tauri::command]
pub async fn receive_from_ticket(
    ticket: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let ticket = NodeTicket::deserialize_from_str(&ticket).map_err(err)?;
    if ticket.share.is_none() {
        return Err("To TICKET urządzenia – dodaj je jako zaufane urządzenie".into());
    }
    let engine = state.engine().await?.clone();
    let dest_dir = state.download_dir().await;
    tauri::async_runtime::spawn(async move {
        let sink = |ev| {
            let _ = app.emit("transfer", ev);
        };
        if let Err(e) = service::pull_shared(&engine, &ticket, &dest_dir, &sink).await {
            emit_error(&app, e);
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn respond_to_offer(
    transfer_id: String,
    accept: bool,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    if let Some(tx) = state.pending.lock().await.remove(&transfer_id) {
        let _ = tx.send(accept);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_download_dir(path: String, state: State<'_, AppState>) -> CmdResult<()> {
    let dir = PathBuf::from(path);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let mut config = state.config.lock().await;
    config.default_download_dir = Some(dir);
    config.save().map_err(err)
}

/// Pliki przekazane z menu „Udostępnij” Androida (pusta lista na desktopie).
#[tauri::command]
pub fn take_shared_files(app: AppHandle) -> Vec<String> {
    crate::platform::take_shared_files(&app)
}

/// Ładuje konfigurację do stanu (po ustawieniu katalogów platformy).
pub fn load_config(app: &AppHandle) {
    let config = Config::load().unwrap_or_else(|e| {
        tracing::warn!("Błąd wczytywania konfiguracji: {e:#}");
        Config::default()
    });
    let state = app.state::<AppState>();
    tauri::async_runtime::block_on(async {
        *state.config.lock().await = config;
    });
}
