//! Tauri commands invoked by the frontend with `invoke("<name>", { … })`.
//!
//! Argument names are camelCase on the JavaScript side (`nodeId` for `node_id`).
//! Errors are returned as localized strings. Long operations (sending,
//! downloading, pairing after adding a device) run in the background and report
//! through events: `transfer`, `transfer-error`, `pairing-code` and `pair-result`.

use ferry::config::Config;
use ferry::discovery::NearbyDevice;
use ferry::i18n::{self, Language};
use ferry::node::NodeTicket;
use ferry::pairing::{PairOutcome, clean_device_name, request_pairing};
use ferry::protocol::messages::TextDecision;
use ferry::transfer::sender::prepare_file;
use ferry::transfer::service;
use ferry::transfer::share::{Share, ShareOptions, ShareSink};
use qrcode::QrCode;
use qrcode::render::svg;
use rust_i18n::t;
use serde::Serialize;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::node::AppState;

/// Result of a command; the error is a message shown to the user.
type CmdResult<T> = Result<T, String>;

/// Converts any error into the message returned to the frontend.
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
    /// The user compared the safety number with the other device.
    pub verified: bool,
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

/// Payload of the `transfer-error` event.
#[derive(Clone, Serialize)]
struct TransferError {
    /// Error description including its causes.
    message: String,
}

/// Payload of the `pairing-code` event, emitted by an outgoing pairing as soon
/// as the verification code is known.
#[derive(Clone, Serialize)]
struct PairingCode {
    /// Node ID of the device being paired.
    node_id: String,
    /// Its name or alias, for display.
    name: String,
    /// Six-digit code to compare with the other screen.
    code: String,
}

/// Emits the `pairing-code` event for an outgoing pairing with `node_id`.
fn emit_pairing_code(app: &AppHandle, node_id: &str, name: &str, code: &str) {
    let _ = app.emit(
        "pairing-code",
        PairingCode {
            node_id: node_id.to_string(),
            name: name.to_string(),
            code: code.to_string(),
        },
    );
}

/// Payload of the `pair-result` event.
#[derive(Clone, Serialize)]
struct PairResult {
    /// Node ID of the added device.
    node_id: String,
    /// Its alias on this device.
    alias: String,
    /// Whether both devices now trust each other.
    ok: bool,
    /// Localized result shown to the user.
    message: String,
}

/// Renders `text` as a QR code SVG document (dark modules on white).
fn qr_svg(text: &str) -> CmdResult<String> {
    let code = QrCode::new(text.as_bytes()).map_err(err)?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#0f172a"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// Reports a failed background transfer with a `transfer-error` event; a
/// cancellation is already shown on the transfer itself.
fn emit_error(app: &AppHandle, e: anyhow::Error) {
    if ferry::error::kind(&e) == ferry::error::ErrorKind::Cancelled {
        return;
    }
    let _ = app.emit(
        "transfer-error",
        TransferError {
            message: format!("{e:#}"),
        },
    );
}

/// Node ID, TICKET (with QR code) and download folder of this device.
///
/// # Errors
///
/// When the P2P node cannot start or the QR code cannot be generated.
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
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
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
            verified: p.verified,
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
///
/// # Errors
///
/// When the input is neither a TICKET nor a Node ID, it is this device's own
/// TICKET, or the configuration cannot be saved.
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
    state.sync_visibility(&config);
    let saved_alias = config
        .find_peer(&node_id)
        .map(|p| p.alias.clone())
        .unwrap_or_default();
    drop(config);
    start_pairing_after_add(app, node_id.clone(), saved_alias, auto_accept);
    Ok(node_id)
}

/// Sends a pairing request to a device just added by TICKET or Node ID in the
/// background and reports the result with a `pair-result` event. When the
/// other device accepts, its current address is stored as well.
fn start_pairing_after_add(app: AppHandle, node_id: String, alias: String, auto_accept: bool) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let outcome = async {
            let addr = resolve_with_nearby(&state, &node_id).await?;
            let engine = state.engine().await?;
            let my_name = state.config.lock().await.device_name();
            let show_code = |code: &str| emit_pairing_code(&app, &node_id, &alias, code);
            let result = request_pairing(engine, addr.clone(), &my_name, show_code)
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
///
/// # Errors
///
/// When the file was not received in this session, no longer exists or no
/// opener program is available.
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
        ferry::system::reveal_path(&path)
    } else {
        ferry::system::open_path(&path)
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
///
/// # Errors
///
/// When the device is not trusted or the configuration cannot be saved.
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
///
/// # Errors
///
/// When the configuration cannot be saved.
#[tauri::command]
pub async fn remove_peer(node_id: String, state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    config.peers.remove(&node_id);
    config.save().map_err(err)?;
    state.sync_visibility(&config);
    Ok(())
}

/// Whether a trusted device responds within a few seconds.
///
/// # Errors
///
/// When `node_id` is not a valid Node ID or the node cannot start.
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
///
/// # Errors
///
/// When no files were given, the device cannot be resolved, the node cannot
/// start or an Android `content://` file cannot be copied. Transfer failures are
/// reported later through `transfer-error`.
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
        let sink = |ev: service::TransferEvent| {
            app.state::<AppState>().track_transfer(&ev);
            let _ = app.emit("transfer", ev);
        };
        if let Err(e) = service::send_files(&engine, addr, &local, &sink).await {
            emit_error(&app, e);
        }
    });
    Ok(())
}

/// Sends text or a link to a device given by alias, nearby name, Node ID or
/// TICKET and waits until the receiver accepts it (it then lands in the
/// receiver's clipboard).
///
/// Returns a localized error when the receiver declined or did not answer.
///
/// # Errors
///
/// When the text is empty or too long, the device cannot be reached or does
/// not answer, or the receiver declined it.
#[tauri::command]
pub async fn send_text(text: String, target: String, state: State<'_, AppState>) -> CmdResult<()> {
    let addr = resolve_with_nearby(&state, &target).await?;
    let engine = state.engine().await?.clone();
    match service::send_text(&engine, addr, &text)
        .await
        .map_err(|e| format!("{e:#}"))?
    {
        TextDecision::Accepted => Ok(()),
        TextDecision::Rejected { reason } => {
            Err(t!("gui.text_rejected", reason = reason).into_owned())
        }
    }
}

/// How long a file shared by TICKET can be downloaded.
const SHARE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Shares a file by a TICKET (the receiver scans the QR code or runs `ferry receive`).
///
/// A node with a new, random identity serves the file for one hour, so the
/// TICKET reveals nothing about this device's permanent Node ID.
///
/// # Errors
///
/// When the file cannot be read and hashed, the node cannot start or the TICKET
/// cannot be built.
#[tauri::command]
pub async fn share_file(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<ShareInfo> {
    let mode = state.config.lock().await.network_mode;
    let local = crate::platform::local_path(&app, &path).await?;
    let file = prepare_file(&local).await.map_err(err)?;
    let events = app.clone();
    let sink: ShareSink = Arc::new(move |event| {
        events.state::<AppState>().track_transfer(&event);
        let _ = events.emit("transfer", event);
    });
    let options = ShareOptions {
        lifetime: Some(SHARE_LIFETIME),
        single_use: false,
    };
    let share = Share::start(file, mode, options, sink).await.map_err(err)?;
    let ticket = share.ticket().map_err(err)?;
    let file_name = share.file_name().to_string();
    {
        // Finished shares (expired) have released their node; forget them.
        let mut shares = state.shares.lock().map_err(err)?;
        shares.retain(|s| !s.is_finished());
        shares.push(share);
    }
    Ok(ShareInfo {
        file_name,
        qr_svg: qr_svg(&ticket)?,
        ticket,
    })
}

/// A running browser session, as shown in the GUI.
#[derive(Serialize)]
pub struct WebShareInfo {
    /// Link to open (also in the QR code).
    url: String,
    /// Links for every local network address.
    urls: Vec<String>,
    /// QR code of `url` (SVG).
    qr_svg: String,
    /// Seconds until the link stops working.
    expires_in: Option<u64>,
    /// Number of shared files.
    files: usize,
    /// How to open the port when firewalld blocks it.
    firewall: Option<String>,
}

/// How long a browser session started from the GUI lasts.
const WEB_LIFETIME: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Starts a browser session sharing `paths` (none: only receiving); a
/// running one is stopped first.
///
/// # Errors
///
/// When a path is not a file or no port can be opened.
#[tauri::command]
pub async fn start_web_share(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<WebShareInfo> {
    let mut files = Vec::with_capacity(paths.len());
    for path in &paths {
        files.push(crate::platform::local_path(&app, path).await?);
    }
    let count = files.len();
    let mut web = state.web.lock().await;
    if let Some(old) = web.take() {
        old.stop();
    }
    let engine = state.engine().await.ok();
    let handler = Arc::new(crate::node::GuiHandler { app: app.clone() });
    let options = ferry::web::WebShareOptions {
        files,
        lifetime: Some(WEB_LIFETIME),
        ..ferry::web::WebShareOptions::default()
    };
    let share = ferry::web::WebShare::start(engine, options, handler)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let port = share.port();
    let firewall = tokio::task::spawn_blocking(move || ferry::web::firewall_hint(port))
        .await
        .ok()
        .flatten();
    let info = WebShareInfo {
        firewall,
        url: share.url().to_string(),
        urls: share.urls().to_vec(),
        qr_svg: qr_svg(share.url())?,
        expires_in: share.expires_in().map(|d| d.as_secs()),
        files: count,
    };
    *web = Some(share);
    Ok(info)
}

/// Ends the browser session; its link stops working at once.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn stop_web_share(state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(share) = state.web.lock().await.take() {
        share.stop();
    }
    Ok(())
}

/// Tells whether a TICKET shares a file or identifies a device to pair with.
///
/// # Errors
///
/// When the text is neither a TICKET nor a Node ID.
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
///
/// # Errors
///
/// When the TICKET is invalid or is a pairing TICKET. Download failures are
/// reported later through `transfer-error`.
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
        let sink = |mut ev: service::TransferEvent| {
            crate::platform::finalize_download(&app, &mut ev);
            app.state::<AppState>().record_download(&ev);
            app.state::<AppState>().track_transfer(&ev);
            let _ = app.emit("transfer", ev);
        };
        if let Err(e) = service::pull_shared(&engine, &ticket, &dest_dir, &sink).await {
            emit_error(&app, e);
        }
    });
    Ok(())
}

/// Answers an `incoming-offer` or `pair-request` event by its ID.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
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
///
/// # Errors
///
/// When the folder cannot be created or the configuration cannot be saved.
#[tauri::command]
pub async fn set_download_dir(path: String, state: State<'_, AppState>) -> CmdResult<()> {
    let dir = PathBuf::from(path);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let mut config = state.config.lock().await;
    config.default_download_dir = Some(dir);
    config.save().map_err(err)
}

/// Address of a device given by Node ID, alias, nearby device name or TICKET;
/// see [`ferry::discovery::resolve_target`].
async fn resolve_with_nearby(state: &AppState, target: &str) -> CmdResult<iroh::EndpointAddr> {
    let config = state.config.lock().await;
    ferry::discovery::resolve_target(&config, state.discovery.get(), target)
        .map_err(|e| format!("{e:#}"))
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
///
/// # Errors
///
/// When local network discovery is unavailable.
#[tauri::command]
pub fn refresh_nearby(state: State<'_, AppState>) -> CmdResult<()> {
    let discovery = state
        .discovery
        .get()
        .ok_or_else(|| t!("tui.discovery_unavailable").into_owned())?;
    discovery.probe();
    Ok(())
}

/// One-button pairing; waits for the other device and returns the saved alias.
///
/// The verification code is reported with the `pairing-code` event once both
/// devices exchanged their nonces.
///
/// # Errors
///
/// When the device cannot be reached, declines, does not answer in time, or the
/// configuration cannot be saved.
#[tauri::command]
pub async fn pair_device(
    node_id: String,
    name: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<String> {
    let addr = resolve_with_nearby(&state, &node_id).await?;
    let engine = state.engine().await?;
    let my_name = state.config.lock().await.device_name();
    let show_code = |code: &str| emit_pairing_code(&app, &node_id, &name, code);
    match request_pairing(engine, addr.clone(), &my_name, show_code)
        .await
        .map_err(|e| format!("{e:#}"))?
    {
        PairOutcome::Accepted { name, addr: remote } => {
            let mut config = state.config.lock().await;
            config
                .add_paired(addr.id, &name, remote.or(Some(addr.clone())), false)
                .map_err(err)?;
            config.save().map_err(err)?;
            state.sync_visibility(&config);
            Ok(config
                .find_peer(&addr.id.to_string())
                .map(|p| p.alias.clone())
                .unwrap_or(name))
        }
        PairOutcome::Rejected => Err(t!("pair.rejected").into_owned()),
    }
}

/// Name of this device.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn get_device_name(state: State<'_, AppState>) -> CmdResult<DeviceName> {
    let config = state.config.lock().await;
    Ok(DeviceName {
        name: config.device_name(),
        is_default: config.device_name.is_none(),
    })
}

/// Changes the name of this device and announces it immediately.
///
/// # Errors
///
/// When the cleaned name is empty or the configuration cannot be saved.
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

/// Network mode of the running node and the saved one.
#[derive(Serialize)]
pub struct NetworkModeInfo {
    /// Mode the running node was started with (`local` or `internet`).
    pub active: &'static str,
    /// Mode saved in the configuration; applies after a restart.
    pub saved: &'static str,
}

/// The network mode of the running node and the one saved for the next start.
///
/// # Errors
///
/// When the P2P node cannot start.
#[tauri::command]
pub async fn get_network_mode(state: State<'_, AppState>) -> CmdResult<NetworkModeInfo> {
    let active = state.engine().await?.mode().code();
    let saved = state.config.lock().await.network_mode.code();
    Ok(NetworkModeInfo { active, saved })
}

/// Saves the network mode (`local` or `internet`); it applies after a restart.
///
/// # Errors
///
/// When the mode is unknown or the configuration cannot be saved.
#[tauri::command]
pub async fn set_network_mode(mode: String, state: State<'_, AppState>) -> CmdResult<()> {
    let mode: ferry::node::NetworkMode = mode.parse().map_err(|e| format!("{e:#}"))?;
    let mut config = state.config.lock().await;
    config.network_mode = mode;
    config.save().map_err(err)
}

/// What limits Ferry in the background on Android; `null` on desktop.
#[tauri::command]
pub fn background_status() -> Option<crate::platform::BackgroundStatus> {
    crate::platform::background_status()
}

/// Opens the system app info screen of Ferry (Android: battery usage).
#[tauri::command]
pub fn open_app_settings() {
    crate::platform::open_app_settings();
}

/// The safety number shared with a trusted device and whether it was verified.
#[derive(Serialize)]
pub struct SafetyNumber {
    /// 60 digits in 12 groups of 5, the same on both devices.
    number: String,
    /// The user compared it with the other device.
    verified: bool,
}

/// The safety number of this device and the trusted device `node_id`
/// ([`ferry::pairing::safety_number`]), to compare with the other device.
///
/// # Errors
///
/// When `node_id` is not a trusted device or the node cannot start.
#[tauri::command]
pub async fn safety_number(node_id: String, state: State<'_, AppState>) -> CmdResult<SafetyNumber> {
    let peer = iroh::EndpointId::from_str(&node_id).map_err(err)?;
    let me = state.engine().await?.node_id();
    let verified = state
        .config
        .lock()
        .await
        .find_peer(&node_id)
        .ok_or_else(|| t!("peer.not_found", target = &node_id).into_owned())?
        .verified;
    Ok(SafetyNumber {
        number: ferry::pairing::safety_number(&me, &peer),
        verified,
    })
}

/// Records whether the user compared the safety number with `node_id`.
///
/// # Errors
///
/// When the device is unknown or the configuration cannot be saved.
#[tauri::command]
pub async fn set_verified(
    node_id: String,
    verified: bool,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    if !config.set_verified(&node_id, verified) {
        return Err(t!("peer.not_found", target = &node_id).into_owned());
    }
    config.save().map_err(err)
}

/// Cancels a running transfer by its ID; `false` when it has already ended.
#[tauri::command]
pub fn cancel_transfer(id: String) -> bool {
    service::cancel_transfer(&id)
}

/// What closing the main window does (desktop): `true` quits Ferry, `false`
/// keeps it running in the system tray.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn get_quit_on_close(state: State<'_, AppState>) -> CmdResult<bool> {
    Ok(state.config.lock().await.quit_on_close)
}

/// Saves what closing the main window does; applies immediately.
///
/// # Errors
///
/// When the configuration cannot be saved.
#[tauri::command]
pub async fn set_quit_on_close(quit: bool, state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    config.quit_on_close = quit;
    config.save().map_err(err)?;
    state
        .quit_on_close
        .store(quit, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// Privacy settings shown in the settings dialog.
#[derive(Serialize, serde::Deserialize)]
pub struct PrivacySettings {
    /// Hidden on the local network from everyone except paired devices.
    hidden: bool,
    /// Texts from Auto-Accept devices are copied without asking.
    auto_accept_texts: bool,
    /// Programs, scripts and documents with macros from Auto-Accept devices are
    /// saved without asking.
    auto_accept_executables: bool,
    /// LocalSend compatibility mode: LocalSend apps can send files here.
    #[serde(default)]
    localsend: bool,
}

/// Current privacy settings.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn get_privacy(state: State<'_, AppState>) -> CmdResult<PrivacySettings> {
    let config = state.config.lock().await;
    Ok(PrivacySettings {
        hidden: config.hidden,
        auto_accept_texts: config.auto_accept_texts,
        auto_accept_executables: config.auto_accept_executables,
        localsend: config.localsend,
    })
}

/// Saves the privacy settings; visibility and the LocalSend compatibility
/// mode apply immediately.
///
/// # Errors
///
/// When the configuration cannot be saved, or the LocalSend mode cannot start
/// (it is then switched off again).
#[tauri::command]
pub async fn set_privacy(
    settings: PrivacySettings,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    {
        let mut config = state.config.lock().await;
        config.hidden = settings.hidden;
        config.auto_accept_texts = settings.auto_accept_texts;
        config.auto_accept_executables = settings.auto_accept_executables;
        config.localsend = settings.localsend;
        config.save().map_err(err)?;
        state.sync_visibility(&config);
    }
    if let Err(e) = crate::node::sync_localsend(&app).await {
        let mut config = state.config.lock().await;
        config.localsend = false;
        config.save().map_err(err)?;
        return Err(e);
    }
    Ok(())
}

/// Restarts the application, e.g. to apply a new network mode (desktop only;
/// on Android the app is restarted by closing it from the recent apps).
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    #[cfg(desktop)]
    app.restart();
    #[cfg(not(desktop))]
    let _ = app;
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
///
/// # Errors
///
/// When the language code is not supported or the configuration cannot be saved.
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
///
/// # Errors
///
/// When no browser can be started (e.g. on Android).
#[tauri::command]
pub fn open_repository() -> CmdResult<()> {
    ferry::system::open_url(env!("CARGO_PKG_REPOSITORY")).map_err(err)
}

/// Paths of files shared from the Android "Share" menu (empty on desktop).
#[tauri::command]
pub fn take_shared_files(app: AppHandle) -> Vec<String> {
    crate::platform::take_shared_files(&app)
}

/// Opens an `http(s)` link from a received text in the default browser
/// (desktop only).
///
/// # Errors
///
/// When `url` is not an `http(s)` link without spaces or no browser can be started.
#[tauri::command]
pub fn open_link(url: String) -> CmdResult<()> {
    let url = url.trim();
    let is_link = (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if !is_link {
        return Err(t!("gui.not_a_link").into_owned());
    }
    ferry::system::open_url(url).map_err(err)
}

/// A text or link shared from the Android "Share" menu (none on desktop).
#[tauri::command]
pub fn take_shared_text(app: AppHandle) -> Option<String> {
    crate::platform::take_shared_text(&app)
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
    state
        .quit_on_close
        .store(config.quit_on_close, std::sync::atomic::Ordering::Relaxed);
    tauri::async_runtime::block_on(async {
        *state.config.lock().await = config;
    });
}
