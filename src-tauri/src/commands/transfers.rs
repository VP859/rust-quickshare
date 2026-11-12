//! Files and texts between Ferry devices: sending, answering offers, pausing
//! and cancelling, sharing and receiving by TICKET, the history, and opening
//! what arrived.

use super::{CmdResult, emit_error, err, qr_svg, resolve_with_nearby};
use crate::node::AppState;
use ferry::node::NodeTicket;
use ferry::protocol::messages::TextDecision;
use ferry::transfer::sender::prepare_file;
use ferry::transfer::service;
use ferry::transfer::share::{Share, ShareOptions, ShareSink};
use rust_i18n::t;
use serde::Serialize;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

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
    if reveal {
        ferry::system::reveal_path(&path).map_err(err)
    } else {
        crate::platform::open_file(&path)
    }
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
    let mode = ferry::node::Network::from_config(&*state.config.lock().await)
        .map_err(|e| format!("{e:#}"))?;
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

/// Answers an `incoming-offer`, `incoming-text` or `pair-request` event by
/// its ID; `copy` copies a long text from a LocalSend app instead of saving it
/// as a file.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn respond_to_offer(
    transfer_id: String,
    accept: bool,
    copy: Option<bool>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    if let Some(tx) = state.pending.lock().await.remove(&transfer_id) {
        let _ = tx.send(crate::node::Answer {
            accept,
            copy: copy.unwrap_or(false),
        });
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

/// Cancels a running transfer by its ID; with `discard` a partly received
/// file is deleted instead of kept for resuming. `false` when it has already
/// ended.
#[tauri::command]
pub fn cancel_transfer(id: String, discard: Option<bool>) -> bool {
    if discard == Some(true) {
        service::cancel_and_discard(&id)
    } else {
        service::cancel_transfer(&id)
    }
}

/// Pauses (`paused`) or resumes a running transfer; `false` when it has ended.
#[tauri::command]
pub fn pause_transfer(id: String, paused: bool) -> bool {
    service::pause_transfer(&id, paused)
}

/// A history entry with what can be done with it again.
#[derive(Serialize)]
pub struct HistoryRow {
    /// The entry as recorded.
    #[serde(flatten)]
    entry: ferry::history::HistoryEntry,
    /// The file on this device (sent or received) when it still exists: it
    /// can be sent again.
    local_file: Option<String>,
    /// The other device is a Ferry device (not LocalSend or a browser), so
    /// the file can be sent to it again directly.
    ferry_peer: bool,
}

/// Finished transfers, newest first, filtered by `query` and `kind`
/// (`file` or `text`; anything else: both); at most 500.
///
/// # Errors
///
/// When the history file cannot be read.
#[tauri::command]
pub fn get_history(query: Option<String>, kind: Option<String>) -> CmdResult<Vec<HistoryRow>> {
    use ferry::history::HistoryKind;
    let path = ferry::history::default_path().map_err(err)?;
    let entries = ferry::history::load(&path).map_err(err)?;
    let mut entries = ferry::history::search(entries, query.as_deref().unwrap_or_default());
    match kind.as_deref() {
        Some("file") => entries.retain(|e| e.kind == HistoryKind::File),
        Some("text") => entries.retain(|e| e.kind == HistoryKind::Text),
        _ => {}
    }
    entries.truncate(500);
    Ok(entries
        .into_iter()
        .map(|entry| HistoryRow {
            local_file: entry.local_file().map(|p| p.display().to_string()),
            ferry_peer: entry.ferry_peer().is_some(),
            entry,
        })
        .collect())
}

/// Deletes the history of transfers.
///
/// # Errors
///
/// When the file cannot be deleted.
#[tauri::command]
pub fn clear_history() -> CmdResult<()> {
    let path = ferry::history::default_path().map_err(err)?;
    ferry::history::clear(&path).map_err(err)
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
