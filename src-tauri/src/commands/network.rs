//! The network mode and the relays of the relay modes.

use super::{CmdResult, err};
use crate::node::AppState;
use serde::Serialize;
use tauri::State;

/// Network mode of the running node and the saved one.
#[derive(Serialize)]
pub struct NetworkModeInfo {
    /// Mode the running node was started with (`local`, `internet`, `relay` or `relay-only`).
    pub active: &'static str,
    /// Mode saved in the configuration; applies after a restart.
    pub saved: &'static str,
    /// Saved relays of the `relay` and `relay-only` modes.
    pub relays: Vec<String>,
}

/// The network mode of the running node and the one saved for the next start.
///
/// # Errors
///
/// When the P2P node cannot start.
#[tauri::command]
pub async fn get_network_mode(state: State<'_, AppState>) -> CmdResult<NetworkModeInfo> {
    let active = state.engine().await?.mode().code();
    let config = state.config.lock().await;
    Ok(NetworkModeInfo {
        active,
        saved: config.network_mode.code(),
        relays: config.relay_urls.clone(),
    })
}

/// Saves the network mode (`local`, `internet` or `relay`) and, when given,
/// the relays of the `relay` mode; it applies after a restart.
///
/// # Errors
///
/// When the mode is unknown, a relay URL is invalid, the `relay` mode has no
/// relay, or the configuration cannot be saved. Nothing is saved then.
#[tauri::command]
pub async fn set_network_mode(
    mode: String,
    relays: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let mode: ferry::node::NetworkMode = mode.parse().map_err(|e| format!("{e:#}"))?;
    let mut config = state.config.lock().await;
    let mut next = config.clone();
    next.network_mode = mode;
    if let Some(relays) = relays {
        next.relay_urls = relays
            .iter()
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .map(|u| ferry::node::parse_relay_url(u).map(|r| r.to_string()))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{e:#}"))?;
    }
    ferry::node::Network::from_config(&next).map_err(|e| format!("{e:#}"))?;
    next.save().map_err(err)?;
    *config = next;
    Ok(())
}

/// The result of [`check_relays`] for one relay.
#[derive(Serialize)]
pub struct RelayCheck {
    /// The address as entered.
    url: String,
    /// Round trip of a request to the relay in milliseconds, when it works.
    ms: Option<u64>,
    /// Why it does not work.
    error: Option<String>,
}

/// Tries each relay for real (see [`ferry::node::check_relay`]), all at
/// once; nothing is saved.
#[tauri::command]
pub async fn check_relays(relays: Vec<String>) -> Vec<RelayCheck> {
    let checks = relays
        .into_iter()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .map(|url| {
            tauri::async_runtime::spawn(async move {
                let result = match ferry::node::parse_relay_url(&url) {
                    Ok(relay) => ferry::node::check_relay(&relay).await,
                    Err(e) => Err(e),
                };
                match result {
                    Ok(rtt) => RelayCheck {
                        url,
                        ms: Some(u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX)),
                        error: None,
                    },
                    Err(e) => RelayCheck {
                        url,
                        ms: None,
                        error: Some(format!("{e:#}")),
                    },
                }
            })
        })
        .collect::<Vec<_>>();
    let mut results = Vec::with_capacity(checks.len());
    for check in checks {
        if let Ok(result) = check.await {
            results.push(result);
        }
    }
    results
}
