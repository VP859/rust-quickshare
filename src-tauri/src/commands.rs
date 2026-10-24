//! Tauri commands invoked by the frontend with `invoke("<name>", { … })`.
//!
//! Argument names are camelCase on the JavaScript side (`nodeId` for `node_id`).
//! Errors are returned as localized strings. Long operations (sending,
//! downloading, pairing after adding a device) run in the background and report
//! through events: `transfer`, `transfer-error` and `pair-result`.

use qrcode::QrCode;
use qrcode::render::svg;
use rqs_p2p::config::Config;
use rqs_p2p::discovery::NearbyDevice;
use rqs_p2p::i18n::{self, Language};
use rqs_p2p::node::NodeTicket;
use rqs_p2p::pairing::{PairOutcome, clean_device_name, pairing_code, request_pairing};
use rqs_p2p::transfer::sender::prepare_file;
use rqs_p2p::transfer::service;
use rust_i18n::t;
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

/// How long to wait when checking whether a device responds.
const PING_TIMEOUT: Duration = Duration::from_secs(6);

/// Information about this device.
#[derive(Serialize)]
pub struct NodeInfo {
    /// Node ID.
    pub node_id: String,
    /// Pairing TICKET.
    pub ticket: String,
    /// QR code of the TICKET as an SVG document.
    pub qr_svg: String,
    /// Folder for received files.
    pub download_dir: String,
}

/// A trusted device as shown in the frontend.
#[derive(Serialize)]
pub struct TrustedPeer {
    /// Node ID.
    pub node_id: String,
    /// Device name chosen by the user.
    pub alias: String,
    /// Whether files are accepted without asking.
    pub auto_accept: bool,
}

/// A file shared by TICKET.
#[derive(Serialize)]
pub struct ShareInfo {
    /// Shared file name.
    pub file_name: String,
    /// Share TICKET.
    pub ticket: String,
    /// QR code of the TICKET as an SVG document.
    pub qr_svg: String,
}

/// What a pasted or scanned TICKET refers to.
#[derive(Serialize)]
pub struct TicketInfo {
    /// Node ID of the device.
    pub node_id: String,
    /// `true` for a file share TICKET, `false` for a pairing TICKET or Node ID.
    pub has_share: bool,
}

/// Name of this device.
#[derive(Serialize)]
pub struct DeviceName {
    /// Effective name.
    pub name: String,
    /// `true` when the user has not set a name (the Android frontend then uses the phone model).
    pub is_default: bool,
}

/// A selectable interface language.
#[derive(Serialize)]
pub struct LanguageOption {
    /// Language code (`en`, `pl`).
    pub code: &'static str,
    /// Name of the language in that language.
    pub name: &'static str,
}

/// Current interface language and the supported ones.
#[derive(Serialize)]
pub struct LanguageInfo {
    /// Code of the active language.
    pub code: &'static str,
    /// All supported languages.
    pub available: Vec<LanguageOption>,
}

#[derive(Clone, Serialize)]
struct TransferError {
    message: String,
}

#[derive(Clone, Serialize)]
struct PairResult {
    node_id: String,
    alias: String,
    ok: bool,
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

/// Node ID, TICKET (with QR code) and download folder of this device.
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

/// Trusted devices sorted by alias.
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

/// Adds a device from a TICKET or Node ID and returns its Node ID.
///
/// A pairing request is then sent in the background so that, once accepted,
/// the other device trusts this one too; the result arrives as a
/// `pair-result` event.
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
        .map_err(|e| format!("{e:#}"))?;
    if let Ok(engine) = state.engine().await
        && node_id == engine.node_id().to_string()
    {
        config.peers.remove(&node_id);
        return Err(t!("tui.own_ticket").into_owned());
    }
    config.save().map_err(err)?;
    let saved_alias = config
        .find_peer(&node_id)
        .map(|p| p.alias.clone())
        .unwrap_or_default();
    drop(config);
    start_pairing_after_add(app, node_id.clone(), saved_alias, auto_accept);
    Ok(node_id)
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
                    Ok(()) => (true, t!("tui.paired_mutual", name = &alias).into_owned()),
                    Err(e) => (
                        false,
                        t!("tui.error", error = format!("{e:#}")).into_owned(),
                    ),
                }
            }
            Ok((_, PairOutcome::Rejected)) => (
                false,
                t!("gui.pair_one_sided_rejected", alias = &alias).into_owned(),
            ),
            Err(e) => (
                false,
                t!("gui.pair_one_sided_failed", alias = &alias, error = e).into_owned(),
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

/// Opens a received file, or its folder when `reveal` is set.
///
/// Only files received during this session may be opened.
#[tauri::command]
pub fn open_received(path: String, reveal: bool, state: State<'_, AppState>) -> CmdResult<()> {
    let path = PathBuf::from(path);
    let allowed = state
        .received
        .lock()
        .map(|r| r.contains(&path))
        .unwrap_or(false);
    if !allowed {
        return Err(t!("gui.open_only_received").into_owned());
    }
    let result = if reveal {
        rqs_p2p::system::reveal_path(&path)
    } else {
        rqs_p2p::system::open_path(&path)
    };
    result.map_err(err)
}

/// Explanation why no nearby devices are visible, if discovery has a problem.
#[tauri::command]
pub fn discovery_health(state: State<'_, AppState>) -> Option<String> {
    match state.discovery.get() {
        Some(d) => d.health().hint(),
        None => Some(t!("tui.discovery_unavailable").into_owned()),
    }
}

/// Changes the alias and/or Auto-Accept setting of a trusted device.
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
        .ok_or_else(|| t!("peer.not_found", target = &node_id).into_owned())?;
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

/// Removes a trusted device.
#[tauri::command]
pub async fn remove_peer(node_id: String, state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    config.peers.remove(&node_id);
    config.save().map_err(err)
}

/// Whether a trusted device responds within a few seconds.
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

/// Sends files to a device given by alias, nearby name, Node ID or TICKET.
///
/// Returns immediately; progress arrives as `transfer` events and a failure as
/// a `transfer-error` event. On Android `content://` URIs are accepted.
#[tauri::command]
pub async fn send_files(
    paths: Vec<String>,
    target: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    if paths.is_empty() {
        return Err(t!("gui.no_files").into_owned());
    }
    let addr = resolve_with_nearby(&state, &target).await?;
    let engine = state.engine().await?.clone();
    let mut local = Vec::with_capacity(paths.len());
    for p in &paths {
        local.push(crate::platform::local_path(&app, p).await?);
    }
    tauri::async_runtime::spawn(async move {
        let sink = |ev| {
            let _ = app.emit("transfer", ev);
        };
        if let Err(e) = service::send_files(&engine, addr, &local, &sink).await {
            emit_error(&app, e);
        }
    });
    Ok(())
}

/// Shares a file by a one-time TICKET (the receiver scans the QR code or runs `rqs receive`).
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

/// Tells whether a TICKET shares a file or identifies a device to pair with.
#[tauri::command]
pub fn inspect_ticket(ticket: String) -> CmdResult<TicketInfo> {
    if let Ok(t) = NodeTicket::deserialize_from_str(&ticket) {
        return Ok(TicketInfo {
            node_id: t.addr.id.to_string(),
            has_share: t.share.is_some(),
        });
    }
    let id = iroh::EndpointId::from_str(ticket.trim())
        .map_err(|_| t!("ticket.invalid_target").into_owned())?;
    Ok(TicketInfo {
        node_id: id.to_string(),
        has_share: false,
    })
}

/// Downloads a file from a share TICKET in the background (`transfer` events).
#[tauri::command]
pub async fn receive_from_ticket(
    ticket: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let ticket = NodeTicket::deserialize_from_str(&ticket).map_err(err)?;
    if ticket.share.is_none() {
        return Err(t!("gui.pairing_ticket").into_owned());
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

/// Answers an `incoming-offer` or `pair-request` event by its ID.
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

/// Sets the folder for received files, creating it if needed.
#[tauri::command]
pub async fn set_download_dir(path: String, state: State<'_, AppState>) -> CmdResult<()> {
    let dir = PathBuf::from(path);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let mut config = state.config.lock().await;
    config.default_download_dir = Some(dir);
    config.save().map_err(err)
}

/// Address of a device: a fresh LAN address from discovery when available
/// (works without internet), otherwise the stored one. Accepts a Node ID,
/// alias, nearby device name or TICKET.
async fn resolve_with_nearby(state: &AppState, target: &str) -> CmdResult<iroh::EndpointAddr> {
    let resolved = state.config.lock().await.resolve_target(target);
    let nearby = state.discovery.get().and_then(|d| match &resolved {
        Ok(addr) => d.addr_of(&addr.id),
        Err(_) => d.find(target).map(|n| n.addr),
    });
    match (nearby, resolved) {
        (Some(addr), _) => Ok(addr),
        (None, Ok(addr)) => Ok(addr),
        (None, Err(e)) => Err(format!("{e:#}")),
    }
}

/// Devices currently visible on the local network (updates arrive as `nearby` events).
#[tauri::command]
pub fn list_nearby(state: State<'_, AppState>) -> Vec<NearbyDevice> {
    state
        .discovery
        .get()
        .map(|d| d.devices())
        .unwrap_or_default()
}

/// Announces this device immediately ("Search" button).
#[tauri::command]
pub fn refresh_nearby(state: State<'_, AppState>) -> CmdResult<()> {
    let discovery = state
        .discovery
        .get()
        .ok_or_else(|| t!("tui.discovery_unavailable").into_owned())?;
    discovery.probe();
    Ok(())
}

/// Verification code shown before a pairing request is sent.
#[tauri::command]
pub async fn pair_code(node_id: String, state: State<'_, AppState>) -> CmdResult<String> {
    let peer = iroh::EndpointId::from_str(&node_id).map_err(err)?;
    Ok(pairing_code(&state.engine().await?.node_id(), &peer))
}

/// One-button pairing; waits for the other device and returns the saved alias.
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
        PairOutcome::Rejected => Err(t!("pair.rejected").into_owned()),
    }
}

/// Name of this device.
#[tauri::command]
pub async fn get_device_name(state: State<'_, AppState>) -> CmdResult<DeviceName> {
    let config = state.config.lock().await;
    Ok(DeviceName {
        name: config.device_name(),
        is_default: config.device_name.is_none(),
    })
}

/// Changes the name of this device and announces it immediately.
#[tauri::command]
pub async fn set_device_name(name: String, state: State<'_, AppState>) -> CmdResult<String> {
    let clean = clean_device_name(&name);
    if clean.is_empty() {
        return Err(t!("settings.name_empty").into_owned());
    }
    let mut config = state.config.lock().await;
    config.device_name = Some(clean.clone());
    config.save().map_err(err)?;
    if let Some(d) = state.discovery.get() {
        d.set_name(&clean);
    }
    Ok(clean)
}

/// The active interface language and the supported ones.
#[tauri::command]
pub fn get_language() -> LanguageInfo {
    LanguageInfo {
        code: i18n::current().code(),
        available: Language::ALL
            .iter()
            .map(|l| LanguageOption {
                code: l.code(),
                name: l.native_name(),
            })
            .collect(),
    }
}

/// Switches and saves the interface language; backend messages, notifications
/// and the tray menu follow it too.
#[tauri::command]
pub async fn set_language(
    code: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<&'static str> {
    let language: Language = code.parse().map_err(err)?;
    i18n::set_language(language);
    let mut config = state.config.lock().await;
    config.language = Some(language.code().to_string());
    config.save().map_err(err)?;
    #[cfg(desktop)]
    crate::tray::refresh(&app);
    #[cfg(not(desktop))]
    let _ = app;
    Ok(language.code())
}

/// Name, version and source code address of the application.
#[derive(Serialize)]
pub struct AppInfo {
    /// Product name.
    pub name: &'static str,
    /// Version of the GUI.
    pub version: &'static str,
    /// Source code repository.
    pub repository: &'static str,
}

/// Name, version and repository shown in the settings.
#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Ferry",
        version: env!("CARGO_PKG_VERSION"),
        repository: env!("CARGO_PKG_REPOSITORY"),
    }
}

/// Opens the project repository in the default browser (desktop only).
#[tauri::command]
pub fn open_repository() -> CmdResult<()> {
    rqs_p2p::system::open_url(env!("CARGO_PKG_REPOSITORY")).map_err(err)
}

/// Paths of files shared from the Android "Share" menu (empty on desktop).
#[tauri::command]
pub fn take_shared_files(app: AppHandle) -> Vec<String> {
    crate::platform::take_shared_files(&app)
}

/// Loads the configuration into the state and activates the saved language.
///
/// Called after the platform directories are set.
pub fn load_config(app: &AppHandle) {
    let config = Config::load().unwrap_or_else(|e| {
        tracing::warn!("Could not load the configuration: {e:#}");
        Config::default()
    });
    i18n::init(None, config.language.as_deref());
    let state = app.state::<AppState>();
    tauri::async_runtime::block_on(async {
        *state.config.lock().await = config;
    });
}
