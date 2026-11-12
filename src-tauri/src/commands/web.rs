//! Browser sessions: a link and QR code for a device without Ferry.

use super::{CmdResult, qr_svg};
use crate::node::AppState;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};

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

/// Who uses the browser session, for the GUI.
#[derive(Serialize)]
pub struct WebShareStatus {
    /// The session is running.
    running: bool,
    /// The device that opened the link and owns the session.
    owner: Option<String>,
    /// Other devices that tried the link after it was taken.
    refused: Vec<String>,
}

/// Who uses the running browser session (polled while its dialog is open).
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn web_share_status(state: State<'_, AppState>) -> CmdResult<WebShareStatus> {
    let web = state.web.lock().await;
    Ok(match web.as_ref().filter(|share| !share.is_finished()) {
        Some(share) => WebShareStatus {
            running: true,
            owner: share.owner().map(|ip| ip.to_string()),
            refused: share.refused().iter().map(ToString::to_string).collect(),
        },
        None => WebShareStatus {
            running: false,
            owner: None,
            refused: Vec::new(),
        },
    })
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
