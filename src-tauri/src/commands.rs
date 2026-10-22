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
use rqs_p2p::discovery::NearbyDevice;
use rqs_p2p::pairing::{PairOutcome, clean_device_name, pairing_code, request_pairing};

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
    app: AppHandle,
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
    let saved_alias = config
        .find_peer(&node_id)
        .map(|p| p.alias.clone())
        .unwrap_or_default();
    drop(config);
    // Prośba o sparowanie: po akceptacji druga strona też zapisze to urządzenie
    // jako zaufane (bez niej zaufanie byłoby tylko jednostronne).
    start_pairing_after_add(app, node_id.clone(), saved_alias, auto_accept);
    Ok(node_id)
}

#[derive(Clone, Serialize)]
struct PairResult {
    node_id: String,
    alias: String,
    ok: bool,
    message: String,
}

fn start_pairing_after_add(app: AppHandle, node_id: String, alias: String, auto_accept: bool) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let outcome = async {
            let addr = resolve_with_nearby(&state, &node_id).await?;
            let engine = state.engine().await?;
            let my_name = state.config.lock().await.device_name();
            let result = request_pairing(engine, addr.clone(), &my_name)
                .await
                .map_err(|e| format!("{e:#}"))?;
            Ok::<_, String>((addr, result))
        }
        .await;
        let (ok, message) = match outcome {
            Ok((addr, PairOutcome::Accepted { addr: remote, .. })) => {
                let mut config = state.config.lock().await;
                let saved = config
                    .add_paired(addr.id, &alias, remote, auto_accept)
                    .and_then(|()| config.save());
                match saved {
                    Ok(()) => (true, format!("Sparowano obustronnie z „{alias}”")),
                    Err(e) => (false, format!("Błąd zapisu: {e:#}")),
                }
            }
            Ok((_, PairOutcome::Rejected)) => (
                false,
                format!("„{alias}” odrzuciło parowanie – dodano tylko na tym urządzeniu"),
            ),
            Err(e) => (
                false,
                format!("„{alias}” dodano tylko na tym urządzeniu (parowanie nieudane: {e})"),
            ),
        };
        let _ = app.emit(
            "pair-result",
            PairResult {
                node_id,
                alias,
                ok,
                message,
            },
        );
    });
}

/// Otwiera odebrany plik (`reveal` – folder z plikiem). Tylko pliki odebrane w tej sesji.
#[tauri::command]
pub fn open_received(path: String, reveal: bool, state: State<'_, AppState>) -> CmdResult<()> {
    let path = PathBuf::from(path);
    let allowed = state
        .received
        .lock()
        .map(|r| r.contains(&path))
        .unwrap_or(false);
    if !allowed {
        return Err("Można otwierać tylko odebrane pliki".into());
    }
    let result = if reveal {
        rqs_p2p::system::reveal_path(&path)
    } else {
        rqs_p2p::system::open_path(&path)
    };
    result.map_err(err)
}

#[tauri::command]
pub fn discovery_health(state: State<'_, AppState>) -> Option<String> {
    match state.discovery.get() {
        Some(d) => d.health().hint(),
        None => Some("Wykrywanie w sieci lokalnej jest niedostępne na tym urządzeniu.".into()),
    }
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
    let addr = resolve_with_nearby(&state, &target).await?;
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
        let sink = |ev: service::TransferEvent| {
            app.state::<AppState>().record_download(&ev);
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

/// Adres urządzenia: najpierw świeży z sieci lokalnej (działa bez internetu),
/// potem zapisany w konfiguracji. Przyjmuje Node ID, alias, nazwę z „W pobliżu” lub TICKET.
async fn resolve_with_nearby(state: &AppState, target: &str) -> CmdResult<iroh::EndpointAddr> {
    let resolved = state.config.lock().await.resolve_target(target);
    let nearby = state.discovery.get().and_then(|d| match &resolved {
        Ok(addr) => d.addr_of(&addr.id),
        Err(_) => d.find(target).map(|n| n.addr),
    });
    match (nearby, resolved) {
        (Some(addr), _) => Ok(addr),
        (None, Ok(addr)) => Ok(addr),
        (None, Err(e)) => Err(err(e)),
    }
}

#[tauri::command]
pub fn list_nearby(state: State<'_, AppState>) -> Vec<NearbyDevice> {
    state
        .discovery
        .get()
        .map(|d| d.devices())
        .unwrap_or_default()
}

/// Natychmiastowe ogłoszenie w sieci (przycisk „Szukaj”).
#[tauri::command]
pub fn refresh_nearby(state: State<'_, AppState>) -> CmdResult<()> {
    let discovery = state
        .discovery
        .get()
        .ok_or("Wykrywanie w sieci lokalnej jest niedostępne")?;
    discovery.probe();
    Ok(())
}

/// Kod weryfikacyjny wyświetlany przed wysłaniem prośby o sparowanie.
#[tauri::command]
pub async fn pair_code(node_id: String, state: State<'_, AppState>) -> CmdResult<String> {
    let peer = iroh::EndpointId::from_str(&node_id).map_err(err)?;
    Ok(pairing_code(&state.engine().await?.node_id(), &peer))
}

/// Paruje z urządzeniem jednym przyciskiem; zwraca nazwę zapisanego urządzenia.
#[tauri::command]
pub async fn pair_device(node_id: String, state: State<'_, AppState>) -> CmdResult<String> {
    let addr = resolve_with_nearby(&state, &node_id).await?;
    let engine = state.engine().await?;
    let my_name = state.config.lock().await.device_name();
    match request_pairing(engine, addr.clone(), &my_name)
        .await
        .map_err(|e| format!("{e:#}"))?
    {
        PairOutcome::Accepted { name, addr: remote } => {
            let mut config = state.config.lock().await;
            config
                .add_paired(addr.id, &name, remote.or(Some(addr.clone())), false)
                .map_err(err)?;
            config.save().map_err(err)?;
            Ok(config
                .find_peer(&addr.id.to_string())
                .map(|p| p.alias.clone())
                .unwrap_or(name))
        }
        PairOutcome::Rejected => Err("Drugie urządzenie odrzuciło parowanie".into()),
    }
}

#[derive(Serialize)]
pub struct DeviceName {
    pub name: String,
    /// `true`, gdy użytkownik nie ustawił nazwy (frontend Androida podstawia model telefonu).
    pub is_default: bool,
}

#[tauri::command]
pub async fn get_device_name(state: State<'_, AppState>) -> CmdResult<DeviceName> {
    let config = state.config.lock().await;
    Ok(DeviceName {
        name: config.device_name(),
        is_default: config.device_name.is_none(),
    })
}

#[tauri::command]
pub async fn set_device_name(name: String, state: State<'_, AppState>) -> CmdResult<String> {
    let clean = clean_device_name(&name);
    if clean.is_empty() {
        return Err("Nazwa nie może być pusta".into());
    }
    let mut config = state.config.lock().await;
    config.device_name = Some(clean.clone());
    config.save().map_err(err)?;
    if let Some(d) = state.discovery.get() {
        d.set_name(&clean);
    }
    Ok(clean)
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
