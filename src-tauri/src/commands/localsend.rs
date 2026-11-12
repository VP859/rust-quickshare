//! LocalSend apps (iPhone, Mac, …): listing and searching them, and sending
//! files and texts to them.

use super::{CmdResult, emit_error, err};
use crate::node::AppState;
use ferry::transfer::service;
use rust_i18n::t;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// How long a search for LocalSend apps waits for answers.
const LOCALSEND_SCAN: std::time::Duration = std::time::Duration::from_secs(3);

/// LocalSend apps heard recently (announcements, and `register` while the
/// compatibility mode runs), without announcing this device.
#[tauri::command]
pub fn list_localsend() -> Vec<ferry::localsend::client::LocalSendDevice> {
    ferry::localsend::client::nearby()
}

/// Searches for LocalSend apps: announces this device's name to them once and
/// returns the ones that answered (with those heard before).
///
/// # Errors
///
/// When the node cannot start or the multicast socket cannot be opened.
#[tauri::command]
pub async fn scan_localsend(
    state: State<'_, AppState>,
) -> CmdResult<Vec<ferry::localsend::client::LocalSendDevice>> {
    let name = state.config.lock().await.device_name();
    let engine = state.engine().await?.clone();
    ferry::localsend::client::scan(
        &engine,
        &ferry::localsend::client::OwnDevice::named(&name),
        LOCALSEND_SCAN,
    )
    .await
    .map_err(err)
}

/// A LocalSend app asked for a PIN: the frontend asks the user and sends
/// again with it.
#[derive(Clone, Serialize)]
struct LocalSendPin {
    /// The app's certificate fingerprint.
    fingerprint: String,
    /// Its name.
    alias: String,
    /// The files to send, as given.
    paths: Vec<String>,
    /// A PIN was given and refused.
    wrong: bool,
}

/// Sends files to the LocalSend app with this certificate `fingerprint` (from
/// [`list_localsend`] or [`scan_localsend`]), with `pin` for apps that
/// require one.
///
/// Returns immediately; progress arrives as `transfer` events (peer
/// `localsend:<IP address>`), a failure as a `transfer-error` event, and a
/// request for a PIN as a `localsend-pin` event.
///
/// # Errors
///
/// When no files were given, the device is no longer known or an Android
/// `content://` file cannot be copied.
#[tauri::command]
pub async fn send_to_localsend(
    paths: Vec<String>,
    fingerprint: String,
    pin: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    if paths.is_empty() {
        return Err(t!("gui.no_files").into_owned());
    }
    let device = ferry::localsend::client::find_by_fingerprint(&fingerprint)
        .ok_or_else(|| t!("localsend.not_found", name = &fingerprint).into_owned())?;
    let name = state.config.lock().await.device_name();
    let mut local = Vec::with_capacity(paths.len());
    for p in &paths {
        local.push(crate::platform::local_path(&app, p).await?);
    }
    tauri::async_runtime::spawn(async move {
        let sink = |ev: service::TransferEvent| {
            app.state::<AppState>().track_transfer(&ev);
            let _ = app.emit("transfer", ev);
        };
        let me = ferry::localsend::client::OwnDevice::named(&name);
        let pin = pin.filter(|p| !p.trim().is_empty());
        match ferry::localsend::client::send(&device, &local, &me, pin.as_deref(), &sink).await {
            Ok(_) => {}
            Err(e) => match e.downcast_ref::<ferry::localsend::client::PinRequired>() {
                Some(asked) => {
                    let _ = app.emit(
                        "localsend-pin",
                        LocalSendPin {
                            fingerprint: device.fingerprint.clone(),
                            alias: device.label(),
                            paths,
                            wrong: asked.wrong,
                        },
                    );
                }
                None => emit_error(&app, e),
            },
        }
    });
    Ok(())
}

/// How a text sent to a LocalSend app ended.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LocalSendText {
    /// The app shows it.
    Sent,
    /// The user of the app declined it.
    Rejected,
    /// The app needs a PIN (`wrong`: the one given was not accepted).
    Pin {
        /// A PIN was given and refused.
        wrong: bool,
    },
}

/// Sends text or a link to the LocalSend app with this certificate
/// fingerprint, which shows it as a message, and waits for the answer.
///
/// # Errors
///
/// When the text is empty or too long, the device is no longer known, cannot
/// be reached or is busy.
#[tauri::command]
pub async fn send_text_to_localsend(
    text: String,
    fingerprint: String,
    pin: Option<String>,
    state: State<'_, AppState>,
) -> CmdResult<LocalSendText> {
    let device = ferry::localsend::client::find_by_fingerprint(&fingerprint)
        .ok_or_else(|| t!("localsend.not_found", name = &fingerprint).into_owned())?;
    let me = ferry::localsend::client::OwnDevice::named(&state.config.lock().await.device_name());
    let pin = pin.filter(|p| !p.trim().is_empty());
    match ferry::localsend::client::send_text(&device, &text, &me, pin.as_deref()).await {
        Ok(ferry::localsend::client::SendResult::Sent) => Ok(LocalSendText::Sent),
        Ok(ferry::localsend::client::SendResult::Rejected) => Ok(LocalSendText::Rejected),
        Err(e) => match e.downcast_ref::<ferry::localsend::client::PinRequired>() {
            Some(asked) => Ok(LocalSendText::Pin { wrong: asked.wrong }),
            None => Err(format!("{e:#}")),
        },
    }
}
