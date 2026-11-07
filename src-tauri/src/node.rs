//! The P2P node living for the whole lifetime of the GUI, local network
//! discovery and the handler forwarding offers, pairing requests and transfer
//! progress to the frontend.
//!
//! Events emitted to the frontend: `node-ready`, `node-error`, `nearby`,
//! `incoming-offer`, `incoming-text`, `pair-request` (questions answered with
//! `respond_to_offer`), `request-cancelled` (a question expired), `text-received`,
//! `peers-changed` and `transfer`.

use ferry::config::Config;
use ferry::discovery::{Discovery, DiscoveryOptions, Visibility};
use ferry::localsend::{LocalSend, LocalSendOffer, LocalSendOptions};
use ferry::node::NodeEngine;
use ferry::pairing::PairRequest;
use ferry::protocol::messages::{BatchOffer, TextDecision, TransferOffer};
use ferry::transfer::risk;
use ferry::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use ferry::transfer::share::Share;
use iroh::EndpointId;
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
    /// The P2P node, started on first use; see [`AppState::engine`].
    engine: OnceCell<NodeEngine>,
    /// Current configuration; saved after every change.
    pub config: Mutex<Config>,
    /// Pending offer and pairing decisions keyed by their ID.
    pub pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// Files shared by TICKET, each served by a node with its own one-time
    /// identity; ended shares are dropped when a new one starts.
    pub shares: std::sync::Mutex<Vec<Share>>,
    /// Download folder chosen at startup (on Android it depends on permissions).
    pub platform_download_dir: OnceLock<PathBuf>,
    /// Gallery folder for received photos and videos (Android only).
    pub platform_media_dir: OnceLock<PathBuf>,
    /// Local network discovery; unset when the UDP socket could not be opened.
    pub discovery: OnceLock<Discovery>,
    /// Received files; only these may be opened from the interface.
    pub received: std::sync::Mutex<HashSet<PathBuf>>,
    /// Copy of [`Config::quit_on_close`], read synchronously when the window
    /// is closed.
    pub quit_on_close: std::sync::atomic::AtomicBool,
    /// The user was already told this session that Ferry keeps running in the tray.
    pub tray_hint_shown: std::sync::atomic::AtomicBool,
    /// IDs of transfers that have not ended; while there are any, Android keeps
    /// the phone awake.
    active_transfers: std::sync::Mutex<HashSet<String>>,
    /// The LocalSend compatibility mode while it is on; see [`sync_localsend`].
    localsend: Mutex<Option<LocalSend>>,
    /// The browser session (`start_web_share`), at most one at a time.
    pub web: Mutex<Option<ferry::web::WebShare>>,
}

impl AppState {
    /// Empty state; the node starts in [`start`].
    pub fn new() -> Self {
        Self {
            engine: OnceCell::new(),
            config: Mutex::new(Config::default()),
            pending: Mutex::new(HashMap::new()),
            shares: std::sync::Mutex::new(Vec::new()),
            platform_download_dir: OnceLock::new(),
            platform_media_dir: OnceLock::new(),
            discovery: OnceLock::new(),
            received: std::sync::Mutex::new(HashSet::new()),
            quit_on_close: std::sync::atomic::AtomicBool::new(false),
            tray_hint_shown: std::sync::atomic::AtomicBool::new(false),
            active_transfers: std::sync::Mutex::new(HashSet::new()),
            localsend: Mutex::new(None),
            web: Mutex::new(None),
        }
    }

    /// Applies the saved visibility and the current list of paired devices to
    /// discovery; called after every change of either.
    pub fn sync_visibility(&self, config: &Config) {
        if let Some(discovery) = self.discovery.get() {
            discovery.set_visibility(Visibility::from_config(config));
        }
    }

    /// Follows every transfer event: when the first transfer starts or the last
    /// one ends, tells the platform (Android holds a wake lock in between).
    pub fn track_transfer(&self, event: &TransferEvent) {
        let Ok(mut active) = self.active_transfers.lock() else {
            return;
        };
        let was_idle = active.is_empty();
        if event.status.is_final() {
            active.remove(&event.id);
        } else {
            active.insert(event.id.clone());
        }
        if was_idle != active.is_empty() {
            crate::platform::set_transfers_active(!active.is_empty());
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
    /// Offered file name; for a batch the name of its first file.
    pub file_name: String,
    /// Offered size in bytes; for a batch the size of all files.
    pub size: u64,
    /// Number of files: 1 for a single file, more for a batch.
    pub count: u32,
    /// Names of the files of a batch (at most the first 20); empty for a single file.
    pub names: Vec<String>,
    /// Why the file(s) can be dangerous (`program`, `script`, `macro` or
    /// `disguised`, see [`ferry::transfer::risk::Risk::code`]), if they can.
    pub risk: Option<&'static str>,
    /// Address of a LocalSend app that sent the offer (compatibility mode,
    /// the sender is not verified); `None` for Ferry devices.
    pub localsend: Option<String>,
    /// The web browser that sent the offer (`Safari, iPhone` at an address)
    /// through a browser session; `None` otherwise.
    pub browser: Option<String>,
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
    /// Shared application state.
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

    /// Alias of a trusted sender and whether it has Auto-Accept enabled.
    async fn sender_info(&self, peer: &EndpointId) -> (Option<String>, bool) {
        let state = self.state();
        let config = state.config.lock().await;
        config
            .find_peer(&peer.to_string())
            .map_or((None, false), |p| (Some(p.alias.clone()), p.auto_accept))
    }

    /// Asks the user through the frontend and waits for the answer.
    ///
    /// Emits `event` with `payload` (whose `id` the frontend passes to
    /// `respond_to_offer`), shows a notification and waits at most `timeout`.
    /// A request left unanswered is withdrawn with a `request-cancelled` event,
    /// so the frontend closes its dialog. Returns `true` only on acceptance.
    async fn ask<P: Serialize + Clone>(
        &self,
        id: &str,
        event: &str,
        payload: P,
        timeout: Duration,
        notification: (&str, &str),
    ) -> bool {
        let state = self.state();
        let (tx, rx) = oneshot::channel();
        state.pending.lock().await.insert(id.to_string(), tx);
        let _ = self.app.emit(event, payload);
        self.notify(notification.0, notification.1);
        let accepted = matches!(tokio::time::timeout(timeout, rx).await, Ok(Ok(true)));
        if state.pending.lock().await.remove(id).is_some() {
            let _ = self.app.emit("request-cancelled", id);
        }
        accepted
    }
}

impl TransferHandler for GuiHandler {
    /// Looks at the configuration the settings change (`paired_only`, pairing window).
    async fn admits(&self, peer: EndpointId, pairing: bool) -> bool {
        self.state()
            .config
            .lock()
            .await
            .admits(&peer.to_string(), pairing)
    }

    async fn decide(
        &self,
        peer: EndpointId,
        transfer_id: &str,
        offer: &TransferOffer,
    ) -> OfferDecision {
        let dest_dir = self.state().download_dir().await;
        let (alias, _) = self.sender_info(&peer).await;
        // Auto-Accept does not cover programs and scripts: those always ask.
        let auto_accept = self
            .state()
            .config
            .lock()
            .await
            .auto_accepts_file(&peer.to_string(), &offer.file_name);
        let sender = alias
            .clone()
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());
        let accepted = auto_accept
            || self
                .ask(
                    transfer_id,
                    "incoming-offer",
                    IncomingOffer {
                        id: transfer_id.to_string(),
                        peer: peer.to_string(),
                        alias,
                        file_name: offer.file_name.clone(),
                        size: offer.file_size,
                        count: 1,
                        names: Vec::new(),
                        risk: risk::file_risk(&offer.file_name).map(risk::Risk::code),
                        localsend: None,
                        browser: None,
                    },
                    DECISION_TIMEOUT,
                    (
                        &t!("gui.notify_offer_title"),
                        &t!("gui.notify_offer", sender = sender, name = &offer.file_name),
                    ),
                )
                .await;
        if accepted {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    async fn decide_batch(&self, peer: EndpointId, batch: &BatchOffer) -> Option<OfferDecision> {
        let dest_dir = self.state().download_dir().await;
        let (alias, _) = self.sender_info(&peer).await;
        let auto_accept = self
            .state()
            .config
            .lock()
            .await
            .auto_accepts_batch(&peer.to_string(), &batch.names);
        let sender = alias
            .clone()
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());
        let id = request_id("batch", &peer);
        let request = IncomingOffer {
            id: id.clone(),
            peer: peer.to_string(),
            alias,
            file_name: batch.names.first().cloned().unwrap_or_default(),
            size: batch.total_size,
            count: batch.count,
            names: batch.names.clone(),
            risk: risk::worst_risk(&batch.names).map(risk::Risk::code),
            localsend: None,
            browser: None,
        };
        let notification = (
            &*t!("gui.notify_offer_title"),
            &*t!("gui.notify_batch", sender = &sender, count = batch.count),
        );
        let accepted = auto_accept
            || self
                .ask(
                    &id,
                    "incoming-offer",
                    request,
                    DECISION_TIMEOUT,
                    notification,
                )
                .await;
        Some(if accepted {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        })
    }

    /// LocalSend senders are not authenticated: always asked, never Auto-Accept.
    async fn decide_localsend(&self, offer: &LocalSendOffer) -> OfferDecision {
        let dest_dir = self.state().download_dir().await;
        let id = format!("localsend-{}", offer.id);
        let request = IncomingOffer {
            id: id.clone(),
            peer: offer.sender.peer_id(),
            alias: Some(offer.sender.label()),
            file_name: offer.names.first().cloned().unwrap_or_default(),
            size: offer.total_size,
            count: offer.count,
            names: if offer.count > 1 {
                offer.names.clone()
            } else {
                Vec::new()
            },
            risk: offer.risk.map(risk::Risk::code),
            localsend: Some(offer.sender.address.to_string()),
            browser: None,
        };
        let notification = (
            &*t!("gui.notify_offer_title"),
            &*t!(
                "gui.notify_batch",
                sender = offer.sender.label(),
                count = offer.count
            ),
        );
        if self
            .ask(
                &id,
                "incoming-offer",
                request,
                DECISION_TIMEOUT,
                notification,
            )
            .await
        {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    /// Files from the browser of a session the user started: always asked.
    async fn decide_browser(&self, offer: &ferry::web::BrowserOffer) -> OfferDecision {
        let dest_dir = self.state().download_dir().await;
        let id = format!("browser-{}", offer.id);
        let request = IncomingOffer {
            id: id.clone(),
            peer: format!("{}{}", ferry::web::PEER_PREFIX, offer.address),
            alias: Some(offer.label()),
            file_name: offer.names.first().cloned().unwrap_or_default(),
            size: offer.total_size,
            count: offer.count,
            names: if offer.count > 1 {
                offer.names.clone()
            } else {
                Vec::new()
            },
            risk: offer.risk.map(risk::Risk::code),
            localsend: None,
            browser: Some(offer.label()),
        };
        let notification = (
            &*t!("gui.notify_offer_title"),
            &*t!(
                "gui.notify_batch",
                sender = offer.label(),
                count = offer.count
            ),
        );
        if self
            .ask(
                &id,
                "incoming-offer",
                request,
                DECISION_TIMEOUT,
                notification,
            )
            .await
        {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    /// Called from async code, so it never blocks on a busy configuration lock.
    fn device_name(&self) -> String {
        self.state()
            .config
            .try_lock()
            .map(|c| c.device_name())
            .unwrap_or_else(|_| ferry::config::paths::default_device_name())
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        let id = request_id("pair", &req.peer);
        let request = IncomingPair {
            id: id.clone(),
            peer: req.peer.to_string(),
            name: req.name.clone(),
            code: req.code.clone(),
        };
        let notification = (
            &*t!("gui.notify_pair_title"),
            &*t!("gui.notify_pair", name = &req.name, code = &req.code),
        );
        if !self
            .ask(&id, "pair-request", request, PAIR_UI_TIMEOUT, notification)
            .await
        {
            return false;
        }
        let state = self.state();
        let mut config = state.config.lock().await;
        let saved = config
            .add_paired(req.peer, &req.name, req.addr, false)
            .and_then(|()| config.save());
        state.sync_visibility(&config);
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
        let (alias, _) = self.sender_info(&peer).await;
        let auto_accept = self
            .state()
            .config
            .lock()
            .await
            .auto_accepts_texts_from(&peer.to_string());
        let sender = alias
            .clone()
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned());
        let id = request_id("text", &peer);
        let request = IncomingText {
            id: id.clone(),
            peer: peer.to_string(),
            alias: alias.clone(),
            text: text.clone(),
        };
        let notification = (
            &*t!("gui.notify_text_title"),
            &*t!("gui.notify_text", sender = &sender),
        );
        if !auto_accept
            && !self
                .ask(
                    &id,
                    "incoming-text",
                    request,
                    DECISION_TIMEOUT,
                    notification,
                )
                .await
        {
            return TextDecision::Rejected {
                reason: t!("transfer.user_rejected").into_owned(),
            };
        }

        let copied = self
            .app
            .clipboard()
            .write_text(text.clone())
            .inspect_err(|e| warn!("Could not copy the received text: {e}"))
            .is_ok();
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
        self.state().track_transfer(&event);
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
fn start_discovery(app: &AppHandle, engine: &NodeEngine, config: &Config) {
    let opts = DiscoveryOptions::from_config(config);
    let discovery = match Discovery::start(engine.clone(), &config.device_name(), opts) {
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

/// Starts or stops the LocalSend compatibility mode to match the
/// configuration; restarts it so a changed hidden mode applies.
///
/// # Errors
///
/// The localized reason when the mode should run but cannot start (e.g. the
/// port is taken by the LocalSend app).
pub async fn sync_localsend(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut running = state.localsend.lock().await;
    if let Some(old) = running.take() {
        old.stopped().await;
    }
    let config = state.config.lock().await.clone();
    if !config.localsend {
        return Ok(());
    }
    let engine = state.engine().await?;
    let handler = Arc::new(GuiHandler { app: app.clone() });
    let localsend = LocalSend::start(engine, LocalSendOptions::from_config(&config), handler)
        .await
        .map_err(|e| format!("{e:#}"))?;
    *running = Some(localsend);
    Ok(())
}

/// Starts the node, discovery and the accept loop in the background (called from `setup`).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        match state.engine().await {
            Ok(engine) => {
                let _ = app.emit("node-ready", engine.node_id().to_string());
                let config = state.config.lock().await.clone();
                start_discovery(&app, engine, &config);
                ferry::history::enable_from_config(&config);
                ferry::transfer::control::set_speed_limit(config.speed_limit);
                if let Err(e) = sync_localsend(&app).await {
                    warn!("{e}");
                }
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
