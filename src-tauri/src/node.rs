//! The P2P node living for the whole lifetime of the GUI, local network
//! discovery and the handler forwarding offers, pairing requests and transfer
//! progress to the frontend.
//!
//! Events emitted to the frontend: `node-ready`, `node-error`, `nearby`,
//! `incoming-offer`, `incoming-text`, `text-received`, `request-cancelled`,
//! `pair-request`, `pair-cancelled`, `peers-changed` and `transfer`.

use iroh::EndpointId;
use rqs_p2p::config::Config;
use rqs_p2p::discovery::{Discovery, DiscoveryOptions};
use rqs_p2p::node::NodeEngine;
use rqs_p2p::pairing::PairRequest;
use rqs_p2p::protocol::messages::{ShareToken, TextDecision, TransferOffer};
use rqs_p2p::transfer::sender::PreparedFile;
use rqs_p2p::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use rust_i18n::t;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{Mutex, OnceCell, oneshot};
use tracing::warn;

/// An offer without an answer is rejected after this time.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// Time to confirm a pairing request in the GUI; shorter than the core's 60 s,
/// so the dialog can be closed and the pending entry cleaned up.
const PAIR_UI_TIMEOUT: Duration = Duration::from_secs(55);

/// State shared by all Tauri commands.
pub struct AppState {
    engine: OnceCell<NodeEngine>,
    /// Current configuration; saved after every change.
    pub config: Mutex<Config>,
    /// Pending offer and pairing decisions keyed by their ID.
    pub pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// Files shared by TICKET, keyed by share token.
    pub shares: std::sync::Mutex<HashMap<ShareToken, PreparedFile>>,
    /// Download folder chosen at startup (on Android it depends on permissions).
    pub platform_download_dir: OnceLock<PathBuf>,
    /// Gallery folder for received photos and videos (Android only).
    pub platform_media_dir: OnceLock<PathBuf>,
    /// Local network discovery; unset when the UDP socket could not be opened.
    pub discovery: OnceLock<Discovery>,
    /// Received files; only these may be opened from the interface.
    pub received: std::sync::Mutex<HashSet<PathBuf>>,
}

impl AppState {
    /// Empty state; the node starts in [`start`].
    pub fn new() -> Self {
        Self {
            engine: OnceCell::new(),
            config: Mutex::new(Config::default()),
            pending: Mutex::new(HashMap::new()),
            shares: std::sync::Mutex::new(HashMap::new()),
            platform_download_dir: OnceLock::new(),
            platform_media_dir: OnceLock::new(),
            discovery: OnceLock::new(),
            received: std::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Remembers the path of a completed download, allowing it to be opened.
    pub fn record_download(&self, event: &TransferEvent) {
        if event.direction == Direction::Download
            && let TransferStatus::Completed { path: Some(p) } = &event.status
            && let Ok(mut received) = self.received.lock()
        {
            received.insert(PathBuf::from(p));
        }
    }

    /// The node with the persistent identity, started lazily on first use.
    ///
    /// # Errors
    ///
    /// A localized message when the node cannot start.
    pub async fn engine(&self) -> Result<&NodeEngine, String> {
        self.engine
            .get_or_try_init(NodeEngine::with_persistent_identity)
            .await
            .map_err(|e| t!("gui.node_failed", error = format!("{e:#}")).into_owned())
    }

    /// Folder for received files: the user's setting, the platform default or
    /// the configuration default, in this order.
    pub async fn download_dir(&self) -> PathBuf {
        let config = self.config.lock().await;
        match (
            &config.default_download_dir,
            self.platform_download_dir.get(),
        ) {
            (Some(dir), _) => dir.clone(),
            (None, Some(dir)) => dir.clone(),
            (None, None) => config.download_dir(),
        }
    }
}

/// Payload of the `pair-request` event.
#[derive(Clone, Serialize)]
pub struct IncomingPair {
    /// ID to answer with `respond_to_offer`.
    pub id: String,
    /// Node ID of the requesting device.
    pub peer: String,
    /// Its device name.
    pub name: String,
    /// Verification code.
    pub code: String,
}

/// Payload of the `incoming-offer` event.
#[derive(Clone, Serialize)]
pub struct IncomingOffer {
    /// Transfer ID to answer with `respond_to_offer`.
    pub id: String,
    /// Node ID of the sender.
    pub peer: String,
    /// Alias of the sender if it is trusted.
    pub alias: Option<String>,
    /// Offered file name.
    pub file_name: String,
    /// Offered file size in bytes.
    pub size: u64,
}

/// Payload of the `incoming-text` event.
#[derive(Clone, Serialize)]
pub struct IncomingText {
    /// ID to answer with `respond_to_offer`.
    pub id: String,
    /// Node ID of the sender.
    pub peer: String,
    /// Alias of the sender if it is trusted.
    pub alias: Option<String>,
    /// The received text.
    pub text: String,
}

/// Payload of the `text-received` event, emitted after a text was accepted.
#[derive(Clone, Serialize)]
pub struct ReceivedText {
    /// Node ID of the sender.
    pub peer: String,
    /// Alias of the sender if it is trusted.
    pub alias: Option<String>,
    /// The received text.
    pub text: String,
    /// Whether the text was copied to the clipboard.
    pub copied: bool,
}

/// [`TransferHandler`] of the GUI.
pub struct GuiHandler {
    /// Handle used to emit events and access [`AppState`].
    pub app: AppHandle,
}

impl GuiHandler {
    fn state(&self) -> tauri::State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// Shows a system notification when the window is not focused.
    fn notify(&self, title: &str, body: &str) {
        let focused = self
            .app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
        if !focused
            && let Err(e) = self
                .app
                .notification()
                .builder()
                .title(title)
                .body(body)
                .show()
        {
            warn!("Could not show a notification: {e}");
        }
    }
}

impl TransferHandler for GuiHandler {
    async fn decide(
        &self,
        peer: EndpointId,
        transfer_id: &str,
        offer: &TransferOffer,
    ) -> OfferDecision {
        let state = self.state();
        let dest_dir = state.download_dir().await;
        let peer_info = state
            .config
            .lock()
            .await
            .find_peer(&peer.to_string())
            .cloned();
        if peer_info.as_ref().is_some_and(|p| p.auto_accept) {
            return OfferDecision::Accept { dest_dir };
        }

        let (tx, rx) = oneshot::channel();
        state
            .pending
            .lock()
            .await
            .insert(transfer_id.to_string(), tx);
        let alias = peer_info.map(|p| p.alias);
        let _ = self.app.emit(
            "incoming-offer",
            IncomingOffer {
                id: transfer_id.to_string(),
                peer: peer.to_string(),
                alias: alias.clone(),
                file_name: offer.file_name.clone(),
                size: offer.file_size,
            },
        );
        let sender = alias.unwrap_or_else(|| t!("daemon.unknown_device").into_owned());
        self.notify(
            &t!("gui.notify_offer_title"),
            &t!("gui.notify_offer", sender = sender, name = &offer.file_name),
        );

        let accepted = matches!(
            tokio::time::timeout(DECISION_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        state.pending.lock().await.remove(transfer_id);
        if accepted {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    fn shared_file(&self, token: &ShareToken) -> Option<PreparedFile> {
        self.state().shares.lock().ok()?.get(token).cloned()
    }

    /// Called from async code, so it never blocks on a busy configuration lock.
    fn device_name(&self) -> String {
        self.state()
            .config
            .try_lock()
            .map(|c| c.device_name())
            .unwrap_or_else(|_| rqs_p2p::config::paths::default_device_name())
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        let state = self.state();
        let id = request_id("pair", &req.peer);
        let (tx, rx) = oneshot::channel();
        state.pending.lock().await.insert(id.clone(), tx);
        let _ = self.app.emit(
            "pair-request",
            IncomingPair {
                id: id.clone(),
                peer: req.peer.to_string(),
                name: req.name.clone(),
                code: req.code.clone(),
            },
        );
        self.notify(
            &t!("gui.notify_pair_title"),
            &t!("gui.notify_pair", name = &req.name, code = &req.code),
        );

        let accepted = matches!(
            tokio::time::timeout(PAIR_UI_TIMEOUT, rx).await,
            Ok(Ok(true))
        );
        if state.pending.lock().await.remove(&id).is_some() {
            let _ = self.app.emit("pair-cancelled", id);
        }
        if !accepted {
            return false;
        }
        let mut config = state.config.lock().await;
        let saved = config
            .add_paired(req.peer, &req.name, req.addr, false)
            .and_then(|()| config.save());
        drop(config);
        match saved {
            Ok(()) => {
                let _ = self.app.emit("peers-changed", ());
                true
            }
            Err(e) => {
                warn!("Could not save the paired device: {e:#}");
                false
            }
        }
    }

    async fn receive_text(&self, peer: EndpointId, text: String) -> TextDecision {
        let state = self.state();
        let peer_info = state
            .config
            .lock()
            .await
            .find_peer(&peer.to_string())
            .cloned();
        let alias = peer_info.as_ref().map(|p| p.alias.clone());
        let sender = alias
            .clone()
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());

        if !peer_info.is_some_and(|p| p.auto_accept) {
            let id = request_id("text", &peer);
            let (tx, rx) = oneshot::channel();
            state.pending.lock().await.insert(id.clone(), tx);
            let _ = self.app.emit(
                "incoming-text",
                IncomingText {
                    id: id.clone(),
                    peer: peer.to_string(),
                    alias: alias.clone(),
                    text: text.clone(),
                },
            );
            self.notify(
                &t!("gui.notify_text_title"),
                &t!("gui.notify_text", sender = &sender),
            );
            let accepted = matches!(
                tokio::time::timeout(DECISION_TIMEOUT, rx).await,
                Ok(Ok(true))
            );
            if state.pending.lock().await.remove(&id).is_some() {
                let _ = self.app.emit("request-cancelled", id);
            }
            if !accepted {
                return TextDecision::Rejected {
                    reason: t!("transfer.user_rejected").into_owned(),
                };
            }
        }

        let copied = match self.app.clipboard().write_text(text.clone()) {
            Ok(()) => true,
            Err(e) => {
                warn!("Could not copy the received text: {e}");
                false
            }
        };
        let _ = self.app.emit(
            "text-received",
            ReceivedText {
                peer: peer.to_string(),
                alias,
                text,
                copied,
            },
        );
        self.notify(
            &t!("gui.notify_text_title"),
            &t!("gui.notify_text_copied", sender = &sender),
        );
        TextDecision::Accepted
    }

    fn on_event(&self, mut event: TransferEvent) {
        crate::platform::finalize_download(&self.app, &mut event);
        self.state().record_download(&event);
        if event.direction == Direction::Download
            && let TransferStatus::Completed { .. } = event.status
        {
            self.notify(&t!("gui.notify_received"), &event.file_name);
        }
        let _ = self.app.emit("transfer", event);
    }
}

/// Unique ID of a pairing request or an incoming text (kind, device and moment).
fn request_id(kind: &str, peer: &EndpointId) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{kind}-{}-{nanos}", peer.fmt_short())
}

/// Starts local network discovery and forwards list changes as `nearby` events.
fn start_discovery(app: &AppHandle, engine: &NodeEngine, name: &str) {
    let discovery = match Discovery::start(engine.clone(), name, DiscoveryOptions::default()) {
        Ok(d) => d,
        Err(e) => {
            warn!("Local network discovery unavailable: {e:#}");
            return;
        }
    };
    let mut watch = discovery.subscribe();
    let state = app.state::<AppState>();
    if state.discovery.set(discovery).is_err() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while watch.changed().await.is_ok() {
            let devices = watch.borrow_and_update().clone();
            let _ = app.emit("nearby", devices);
        }
    });
}

/// Starts the node, discovery and the accept loop in the background (called from `setup`).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        match state.engine().await {
            Ok(engine) => {
                let _ = app.emit("node-ready", engine.node_id().to_string());
                let name = state.config.lock().await.device_name();
                start_discovery(&app, engine, &name);
                let handler = Arc::new(GuiHandler { app: app.clone() });
                service::serve(engine.clone(), handler).await;
            }
            Err(e) => {
                warn!("{e}");
                let _ = app.emit("node-error", e);
            }
        }
    });
}
