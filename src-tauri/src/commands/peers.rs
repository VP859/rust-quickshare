//! Trusted devices and pairing: adding, renaming and removing devices, the
//! pairing window, safety numbers, and the devices nearby.

use super::{CmdResult, err, resolve_with_nearby};
use crate::node::AppState;
use ferry::discovery::NearbyDevice;
use ferry::pairing::{PairOutcome, request_pairing};
use rust_i18n::t;
use serde::Serialize;
use std::str::FromStr;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// How long to wait when checking whether a device responds.
const PING_TIMEOUT: Duration = Duration::from_secs(6);

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
    // The frontend reads the list after every change (pairing, renaming,
    // removing), so the Android share menu follows it from here.
    crate::platform::set_share_targets(
        &peers
            .iter()
            .map(|p| (p.node_id.clone(), p.alias.clone()))
            .collect::<Vec<_>>(),
    );
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
            Ok((
                addr,
                PairOutcome::Accepted {
                    addr: remote,
                    discovery_key,
                    ..
                },
            )) => {
                let mut config = state.config.lock().await;
                let saved = config
                    .add_paired(addr.id, &alias, remote, auto_accept, Some(discovery_key))
                    .and_then(|()| config.save());
                state.sync_visibility(&config);
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
pub async fn remove_peer(
    node_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let target = {
        let mut config = state.config.lock().await;
        let target = config.resolve_target(&node_id).ok();
        if config.peers.remove(&node_id).is_some() {
            config.queue_unpair(&node_id);
        }
        config.save().map_err(err)?;
        state.sync_visibility(&config);
        target
    };
    // Tell the other device, so that it does not keep listing this one as
    // paired. Best effort and in the background: it may be offline.
    if let (Some(target), Ok(engine)) = (target, state.engine().await) {
        let engine = engine.clone();
        let app = app.clone();
        tokio::spawn(async move {
            match ferry::transfer::service::send_unpair(&engine, target).await {
                Ok(()) => crate::node::forget_unpair(&app, &node_id).await,
                // It is told when it appears on the local network.
                Err(e) => tracing::debug!("Could not tell the removed device yet: {e:#}"),
            }
        });
    }
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
        PairOutcome::Accepted {
            name,
            addr: remote,
            discovery_key,
        } => {
            let mut config = state.config.lock().await;
            config
                .add_paired(
                    addr.id,
                    &name,
                    remote.or(Some(addr.clone())),
                    false,
                    Some(discovery_key),
                )
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

/// Minutes pairing stays open after the user opens it.
const PAIRING_MINUTES: i64 = 5;

/// Opens pairing for a few minutes (`open`) or closes it. Only unknown devices
/// need it, and only with `paired_only` on.
///
/// # Errors
///
/// When the configuration cannot be saved.
#[tauri::command]
pub async fn set_pairing_open(open: bool, state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    if open {
        config.open_pairing(PAIRING_MINUTES);
    } else {
        config.close_pairing();
    }
    config.save().map_err(err)
}
