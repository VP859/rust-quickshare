//! Tauri commands invoked by the frontend with `invoke("<name>", { … })`.
//!
//! Argument names are camelCase on the JavaScript side (`nodeId` for `node_id`).
//! Errors are returned as localized strings. Long operations (sending,
//! downloading, pairing after adding a device) run in the background and report
//! through events: `transfer`, `transfer-error`, `pairing-code` and `pair-result`.
//!
//! The commands are grouped by topic in the submodules and re-exported here,
//! so `generate_handler!` in `lib.rs` names them directly.

mod android;
mod app;
mod identity;
mod localsend;
mod network;
mod peers;
mod settings;
mod shared;
mod transfers;
mod web;

pub use android::*;
pub use app::*;
pub use identity::*;
pub use localsend::*;
pub use network::*;
pub use peers::*;
pub use settings::*;
pub use shared::*;
pub use transfers::*;
pub use web::*;

use crate::node::AppState;
use qrcode::QrCode;
use qrcode::render::svg;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Result of a command; the error is a message shown to the user.
type CmdResult<T> = Result<T, String>;

/// Converts any error into the message returned to the frontend.
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Payload of the `transfer-error` event.
#[derive(Clone, Serialize)]
struct TransferError {
    /// Error description including its causes.
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

/// Address of a device given by Node ID, alias, nearby device name or TICKET;
/// see [`ferry::discovery::resolve_target`].
async fn resolve_with_nearby(state: &AppState, target: &str) -> CmdResult<iroh::EndpointAddr> {
    let config = state.config.lock().await;
    ferry::discovery::resolve_target(&config, state.discovery.get(), target)
        .map_err(|e| format!("{e:#}"))
}
