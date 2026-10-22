//! Węzeł P2P działający przez cały czas życia aplikacji GUI, wykrywanie urządzeń
//! w sieci lokalnej oraz handler, który przekazuje oferty, prośby o sparowanie
//! i postęp transferów do frontendu.

use iroh::EndpointId;
use rqs_p2p::config::Config;
use rqs_p2p::discovery::{Discovery, DiscoveryOptions};
use rqs_p2p::node::NodeEngine;
use rqs_p2p::pairing::PairRequest;
use rqs_p2p::protocol::messages::{ShareToken, TransferOffer};
use rqs_p2p::transfer::sender::PreparedFile;
use rqs_p2p::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{Mutex, OnceCell, oneshot};
use tracing::warn;

/// Po tym czasie oferta bez odpowiedzi jest odrzucana.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// Czas na potwierdzenie parowania w interfejsie (rdzeń czeka 60 s).
const PAIR_UI_TIMEOUT: Duration = Duration::from_secs(55);

pub struct AppState {
    engine: OnceCell<NodeEngine>,
    pub config: Mutex<Config>,
    pub pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    pub shares: std::sync::Mutex<HashMap<ShareToken, PreparedFile>>,
    /// Katalog odbioru ustalony przy starcie (na Androidzie zależny od uprawnień).
    pub platform_download_dir: OnceLock<PathBuf>,
    /// Wykrywanie w sieci lokalnej (brak, gdy nie udało się otworzyć gniazda UDP).
    pub discovery: OnceLock<Discovery>,
    /// Odebrane pliki – tylko je wolno otworzyć z poziomu interfejsu.
    pub received: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            engine: OnceCell::new(),
            config: Mutex::new(Config::default()),
            pending: Mutex::new(HashMap::new()),
            shares: std::sync::Mutex::new(HashMap::new()),
            platform_download_dir: OnceLock::new(),
            discovery: OnceLock::new(),
            received: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Zapamiętuje ścieżkę ukończonego pobrania (zezwolenie na „Otwórz”).
    pub fn record_download(&self, event: &TransferEvent) {
        if event.direction == Direction::Download
            && let TransferStatus::Completed { path: Some(p) } = &event.status
            && let Ok(mut received) = self.received.lock()
        {
            received.insert(PathBuf::from(p));
        }
    }

    /// Węzeł ze stałą tożsamością; uruchamiany leniwie przy pierwszym użyciu.
    pub async fn engine(&self) -> Result<&NodeEngine, String> {
        self.engine
            .get_or_try_init(NodeEngine::with_persistent_identity)
            .await
            .map_err(|e| format!("Nie udało się uruchomić węzła P2P: {e:#}"))
    }

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

#[derive(Clone, Serialize)]
pub struct IncomingPair {
    pub id: String,
    pub peer: String,
    pub name: String,
    pub code: String,
}

#[derive(Clone, Serialize)]
pub struct IncomingOffer {
    pub id: String,
    pub peer: String,
    pub alias: Option<String>,
    pub file_name: String,
    pub size: u64,
}

pub struct GuiHandler {
    pub app: AppHandle,
}

impl GuiHandler {
    fn state(&self) -> tauri::State<'_, AppState> {
        self.app.state::<AppState>()
    }

    /// Powiadomienie systemowe, gdy okno nie jest na pierwszym planie.
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
            warn!("Nie udało się wyświetlić powiadomienia: {e}");
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
        self.notify(
            "Przychodzący plik",
            &format!(
                "{} chce wysłać „{}”",
                alias.as_deref().unwrap_or("Nieznane urządzenie"),
                offer.file_name
            ),
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
                reason: "Użytkownik odrzucił transfer".into(),
            }
        }
    }

    fn shared_file(&self, token: &ShareToken) -> Option<PreparedFile> {
        self.state().shares.lock().ok()?.get(token).cloned()
    }

    fn device_name(&self) -> String {
        // Wywoływane w kontekście async – nie blokujemy, gdy konfiguracja jest zajęta.
        self.state()
            .config
            .try_lock()
            .map(|c| c.device_name())
            .unwrap_or_else(|_| rqs_p2p::config::paths::default_device_name())
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        let state = self.state();
        let id = uuid_like(&req);
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
            "Prośba o sparowanie",
            &format!("{} – kod {}", req.name, req.code),
        );

        // Nieco krócej niż limit rdzenia, aby posprzątać i zamknąć okno dialogowe.
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
                warn!("Nie udało się zapisać sparowanego urządzenia: {e:#}");
                false
            }
        }
    }

    fn on_event(&self, event: TransferEvent) {
        self.state().record_download(&event);
        if event.direction == Direction::Download
            && let TransferStatus::Completed { .. } = event.status
        {
            self.notify("Plik odebrany", &event.file_name);
        }
        let _ = self.app.emit("transfer", event);
    }
}

/// Identyfikator prośby o sparowanie (unikalny dla urządzenia i chwili).
fn uuid_like(req: &PairRequest) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("pair-{}-{nanos}", req.peer.fmt_short())
}

/// Uruchamia wykrywanie w sieci lokalnej i przekazuje zmiany listy do frontendu (`nearby`).
fn start_discovery(app: &AppHandle, engine: &NodeEngine, name: &str) {
    let discovery = match Discovery::start(engine.clone(), name, DiscoveryOptions::default()) {
        Ok(d) => d,
        Err(e) => {
            warn!("Wykrywanie w sieci lokalnej niedostępne: {e:#}");
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

/// Uruchamia węzeł i pętlę nasłuchu w tle (wywoływane w `setup`).
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
