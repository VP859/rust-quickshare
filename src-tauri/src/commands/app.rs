//! The device and the app: name, language, version, welcome card, restart, and
//! loading the configuration at startup.

use super::{CmdResult, err, qr_svg};
use crate::node::AppState;
use ferry::config::Config;
use ferry::i18n::{self, Language};
use ferry::pairing::clean_device_name;
use rust_i18n::t;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

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

/// Whether the welcome card for new users may be shown (it was never closed).
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn get_welcome(state: State<'_, AppState>) -> CmdResult<bool> {
    Ok(!state.config.lock().await.welcome_dismissed)
}

/// Hides the welcome card for good: closed by the user, or no longer needed
/// after the first pairing or transfer.
///
/// # Errors
///
/// When the configuration cannot be saved.
#[tauri::command]
pub async fn dismiss_welcome(state: State<'_, AppState>) -> CmdResult<()> {
    let mut config = state.config.lock().await;
    if config.welcome_dismissed {
        return Ok(());
    }
    config.welcome_dismissed = true;
    config.save().map_err(err)
}

/// Restarts the application, e.g. to apply a new network mode or a restored
/// device key (on Android in a new process, see `Restarter.kt`).
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        crate::platform::restart_app();
    }
    #[cfg(not(target_os = "android"))]
    app.restart();
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
