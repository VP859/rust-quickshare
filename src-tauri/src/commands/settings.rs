//! Settings: privacy, the speed limit and what closing the window does.

use super::{CmdResult, err};
use crate::node::AppState;
use serde::Serialize;
use tauri::{AppHandle, State};

/// The speed limit in bytes per second; `null` without a limit.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn get_speed_limit(state: State<'_, AppState>) -> CmdResult<Option<u64>> {
    Ok(state.config.lock().await.speed_limit)
}

/// Sets the speed limit (`2M`, `500K`, `off`), saves it and applies it to
/// transfers in progress; returns it in bytes per second.
///
/// # Errors
///
/// When the value is not a speed or the configuration cannot be saved.
#[tauri::command]
pub async fn set_speed_limit(value: String, state: State<'_, AppState>) -> CmdResult<Option<u64>> {
    let limit = ferry::transfer::control::parse_speed(&value).map_err(|e| format!("{e:#}"))?;
    let mut config = state.config.lock().await;
    config.speed_limit = limit;
    config.save().map_err(err)?;
    ferry::transfer::control::set_speed_limit(limit);
    Ok(limit)
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
    /// Finished transfers are kept in the history.
    #[serde(default = "default_true")]
    keep_history: bool,
    /// Offers, texts and pairing requests only from paired devices.
    #[serde(default)]
    paired_only: bool,
    /// Pairing is open at the moment (shown only; see [`set_pairing_open`](super::set_pairing_open)).
    #[serde(default)]
    pairing_open: bool,
    /// The operating system is announced to everyone on the local network.
    #[serde(default)]
    announce_platform: bool,
    /// Photos are sent without their location and other metadata.
    #[serde(default)]
    strip_metadata: bool,
    /// Random port for direct connections instead of the fixed one (applies
    /// at the next start).
    #[serde(default)]
    random_port: bool,
}

/// `true`, for settings that are on when the frontend does not send them.
fn default_true() -> bool {
    true
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
        keep_history: !config.disable_history,
        paired_only: config.paired_only,
        pairing_open: config.pairing_open(),
        announce_platform: config.announce_platform,
        random_port: config.random_port,
        strip_metadata: config.strip_metadata,
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
        config.disable_history = !settings.keep_history;
        config.paired_only = settings.paired_only;
        config.announce_platform = settings.announce_platform;
        config.strip_metadata = settings.strip_metadata;
        ferry::transfer::metadata::set_strip_metadata(settings.strip_metadata);
        config.random_port = settings.random_port;
        config.save().map_err(err)?;
        ferry::history::enable_from_config(&config);
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
