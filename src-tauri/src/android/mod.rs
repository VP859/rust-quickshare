//! ==========================================================================
//!  Natywna integracja z systemem Android (wyłącznie `cfg(target_os = "android")`)
//!
//!  Moduł udostępnia backendowi RQS trzy rzeczy, których nie da się zrobić
//!  z poziomu WebView:
//!
//!   1. **Obsługa URI `content://`** – pliki przekazane przez Share Sheet
//!      (Galeria, Menedżer plików) NIE mają ścieżki dyskowej. Czytamy je
//!      strumieniowo przez `ContentResolver.openInputStream()` i zapisujemy
//!      do katalogu podręcznego aplikacji, skąd trafiają do silnika iroh.
//!
//!   2. **Powiadomienia systemowe** – kanały, powiadomienie przychodzącego
//!      transferu z przyciskami *Odebrać / Odrzucić* oraz pasek postępu.
//!
//!   3. **Foreground Service** – chroni proces węzła `iroh` przed zabiciem
//!      przez Doze/App Standby podczas dużych transferów oraz trzyma
//!      WakeLock (CPU) i MulticastLock (detekcja mDNS w sieci lokalnej).
//!
//!  Wszystkie wywołania idą przez JNI na klasę Kotlin
//!  `com.rqs.quickshare.RqsNotifications` oraz usługę
//!  `com.rqs.quickshare.TransferForegroundService`.
//! ==========================================================================

pub mod content_uri;
pub mod jni;
pub mod network;
pub mod notifications;
pub mod storage;

pub use content_uri::{copy_content_uri_to_cache, is_content_uri, resolve_share_payload};
pub use jni::AndroidBridge;
pub use network::{connectivity_report, on_network_changed};
pub use notifications::{notify_done, notify_incoming_offer, notify_progress};
pub use storage::save_received_file;

use tauri::{AppHandle, Emitter, Manager};

/// Payload intencji `ACTION_SEND`/`ACTION_SEND_MULTIPLE` odebranej z natywnego
/// kodu Kotlin (`MainActivity.handleShareIntent`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SharedIntentPayload {
    /// `android.intent.action.SEND` | `SEND_MULTIPLE` | `VIEW`
    pub action: String,
    /// MIME podany przez aplikację wysyłającą
    pub mime: String,
    /// Lista URI (`content://…`, `file://…` lub surowe ścieżki)
    pub uris: Vec<String>,
    /// Opcjonalny tekst współdzielony (EXTRA_TEXT)
    #[serde(default)]
    pub text: Option<String>,
}

/// Bufor przejętych intencji – WebView startuje później niż Aktywność, więc
/// frontend pobiera je przez `take_pending_share()`.
static PENDING_SHARE: std::sync::Mutex<Option<SharedIntentPayload>> = std::sync::Mutex::new(None);

pub fn stash_shared_payload(payload: SharedIntentPayload) {
    if let Ok(mut guard) = PENDING_SHARE.lock() {
        *guard = Some(payload);
    }
}

pub fn take_shared_payload() -> Option<SharedIntentPayload> {
    PENDING_SHARE.lock().ok().and_then(|mut g| g.take())
}

/// Emituje zdarzenie do frontendu (otwarcie ekranu wyboru odbiorcy).
pub fn emit_to_frontend(app: &AppHandle, event: &str, payload: impl serde::Serialize) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!("Nie udało się wysłać eventu {event}: {e}");
    }
}

/// Uruchamia Foreground Service (jeśli jeszcze nie działa) – wywoływane przed
/// każdym transferem, aby system nie uśpił socketu UDP w trakcie sesji QUIC.
pub fn ensure_foreground_service(app: &AppHandle, title: &str, text: &str, progress: i32) {
    let _ = app; // kontekst Androida bierzemy z ndk-context
    if let Err(e) = AndroidBridge::start_transfer_service(title, text, progress) {
        tracing::warn!("Foreground Service nie wystartował: {e}");
    }
}

pub fn stop_foreground_service(app: &AppHandle) {
    let _ = app;
    let _ = AndroidBridge::stop_transfer_service();
}
