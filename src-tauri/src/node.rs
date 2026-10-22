//! Węzeł P2P działający przez cały czas życia aplikacji GUI oraz handler,
//! który przekazuje oferty i postęp transferów do frontendu.

use iroh::EndpointId;
use rqs_p2p::config::Config;
use rqs_p2p::node::NodeEngine;
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

pub struct AppState {
    engine: OnceCell<NodeEngine>,
    pub config: Mutex<Config>,
    pub pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    pub shares: std::sync::Mutex<HashMap<ShareToken, PreparedFile>>,
    /// Katalog odbioru ustalony przy starcie (na Androidzie zależny od uprawnień).
    pub platform_download_dir: OnceLock<PathBuf>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            engine: OnceCell::new(),
            config: Mutex::new(Config::default()),
            pending: Mutex::new(HashMap::new()),
            shares: std::sync::Mutex::new(HashMap::new()),
            platform_download_dir: OnceLock::new(),
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

    fn on_event(&self, event: TransferEvent) {
        if event.direction == Direction::Download
            && let TransferStatus::Completed { .. } = event.status
        {
            self.notify("Plik odebrany", &event.file_name);
        }
        let _ = self.app.emit("transfer", event);
    }
}

/// Uruchamia węzeł i pętlę nasłuchu w tle (wywoływane w `setup`).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        match state.engine().await {
            Ok(engine) => {
                let _ = app.emit("node-ready", engine.node_id().to_string());
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
