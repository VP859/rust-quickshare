//! [`TransferHandler`] of the background process: what the app decides in
//! its dialogs is asked in notifications with "Accept" and "Decline".
//!
//! The rules are the app's: Auto-Accept devices are accepted without asking
//! (programs and scripts only with `auto_accept_executables`), LocalSend and
//! browser offers are always asked about, accepted texts go to the clipboard.
//! Pairing needs the code compared in the window, so a request is declined
//! with a notification saying to open Ferry.

use super::notify::Notifier;
use ferry::config::Config;
use ferry::localsend::{LocalSendOffer, LocalSendText, LocalSendTextDecision};
use ferry::pairing::PairRequest;
use ferry::protocol::messages::{BatchOffer, MAX_TEXT_LEN, TextDecision, TransferOffer};
use ferry::size::human_size;
use ferry::transfer::risk::{self, Risk};
use ferry::transfer::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use ferry::web::BrowserOffer;
use iroh::EndpointId;
use rust_i18n::t;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// An offer without an answer is declined after this time (as in the app).
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
/// Characters of a text shown in its question.
const TEXT_PREVIEW: usize = 200;

/// Asks in notifications; see the [module](self).
pub struct BackgroundHandler {
    /// Shows the notifications.
    notifier: Arc<Notifier>,
    /// On Linux the clipboard content disappears with this object, so it lives
    /// as long as the process.
    clipboard: std::sync::Mutex<Option<arboard::Clipboard>>,
}

impl BackgroundHandler {
    /// A handler asking through `notifier`.
    pub fn new(notifier: Arc<Notifier>) -> Self {
        Self {
            notifier,
            clipboard: std::sync::Mutex::new(None),
        }
    }

    /// The current configuration: `ferry config`, the app before it handed
    /// over, or a restored backup may have changed it.
    fn config() -> Config {
        Config::load().unwrap_or_else(|e| {
            warn!("Could not load the configuration: {e:#}");
            Config::default()
        })
    }

    /// Name of a paired `peer`, or "unknown device".
    fn sender(config: &Config, peer: &EndpointId) -> String {
        config
            .find_peer(&peer.to_string())
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("daemon.unknown_device").into_owned())
    }

    /// Asks about files: `what` says who sends what, followed by the size and
    /// a warning about files that can run code.
    async fn ask_files(&self, what: &str, size: u64, risk: Option<Risk>) -> bool {
        let mut body = format!("{what} ({})", human_size(size));
        if let Some(risk) = risk {
            body = format!("{body}\n\n{}", risk.warning());
        }
        self.notifier
            .ask(&t!("gui.notify_offer_title"), &body, DECISION_TIMEOUT)
            .await
    }

    /// Accepts into the download folder, or declines.
    fn decision(accepted: bool, config: &Config) -> OfferDecision {
        if accepted {
            OfferDecision::Accept {
                dest_dir: config.download_dir(),
            }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    /// Asks about a text from `sender`, showing its beginning.
    async fn ask_text(&self, sender: &str, text: &str) -> bool {
        let body = format!(
            "{}\n\n{}",
            t!("gui.notify_text", sender = sender),
            service::text_preview(text, TEXT_PREVIEW)
        );
        self.notifier
            .ask(&t!("gui.notify_text_title"), &body, DECISION_TIMEOUT)
            .await
    }

    /// Puts an accepted text into the clipboard and says so; whether it was copied.
    async fn take_text(&self, sender: &str, text: &str) -> bool {
        let copied = self.clipboard.lock().is_ok_and(|mut clipboard| {
            if clipboard.is_none() {
                *clipboard = arboard::Clipboard::new()
                    .inspect_err(|e| warn!("No clipboard: {e}"))
                    .ok();
            }
            clipboard.as_mut().is_some_and(|c| c.set_text(text).is_ok())
        });
        if copied {
            self.notifier
                .show(
                    &t!("gui.notify_text_title"),
                    &t!("gui.notify_text_copied", sender = sender),
                )
                .await;
        }
        copied
    }
}

impl TransferHandler for BackgroundHandler {
    async fn admits(&self, peer: EndpointId, pairing: bool) -> bool {
        Config::admits_now(&peer.to_string(), pairing)
    }

    /// The other device removed this one: removed here too, as in the app.
    async fn peer_unpaired(&self, peer: EndpointId) {
        let removed = Config::load().and_then(|mut config| {
            if config.peers.remove(&peer.to_string()).is_some() {
                config.save()?;
            }
            Ok(())
        });
        if let Err(e) = removed {
            warn!("Could not save after {peer} removed this device: {e:#}");
        }
    }

    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        let config = Self::config();
        let accepted = config.auto_accepts_file(&peer.to_string(), &offer.file_name) || {
            let sender = Self::sender(&config, &peer);
            let what = t!("gui.notify_offer", sender = sender, name = &offer.file_name);
            self.ask_files(&what, offer.file_size, risk::file_risk(&offer.file_name))
                .await
        };
        Self::decision(accepted, &config)
    }

    async fn decide_batch(&self, peer: EndpointId, batch: &BatchOffer) -> Option<OfferDecision> {
        let config = Self::config();
        let accepted = config.auto_accepts_batch(&peer.to_string(), &batch.names) || {
            let sender = Self::sender(&config, &peer);
            let what = t!("gui.notify_batch", sender = sender, count = batch.count);
            self.ask_files(&what, batch.total_size, risk::worst_risk(&batch.names))
                .await
        };
        Some(Self::decision(accepted, &config))
    }

    /// LocalSend senders are not authenticated: always asked, never Auto-Accept.
    async fn decide_localsend(&self, offer: &LocalSendOffer) -> OfferDecision {
        let what = t!(
            "gui.notify_batch",
            sender = offer.sender.label(),
            count = offer.count
        );
        let accepted = self.ask_files(&what, offer.total_size, offer.risk).await;
        Self::decision(accepted, &Self::config())
    }

    /// Files from the browser of a session the app started: always asked.
    async fn decide_browser(&self, offer: &BrowserOffer) -> OfferDecision {
        let what = t!(
            "gui.notify_batch",
            sender = offer.label(),
            count = offer.count
        );
        let accepted = self.ask_files(&what, offer.total_size, offer.risk).await;
        Self::decision(accepted, &Self::config())
    }

    /// The code has to be compared on both screens, which a notification
    /// cannot do: declined, with a notification saying to open Ferry.
    async fn confirm_pair(&self, request: PairRequest) -> bool {
        self.notifier
            .show(
                &t!("gui.notify_pair_title"),
                &t!("gui.background_pair", name = &request.name),
            )
            .await;
        false
    }

    async fn receive_text(&self, peer: EndpointId, text: String) -> TextDecision {
        let config = Self::config();
        let sender = Self::sender(&config, &peer);
        if !config.auto_accepts_texts_from(&peer.to_string())
            && !self.ask_text(&sender, &text).await
        {
            return TextDecision::Rejected {
                reason: t!("transfer.user_rejected").into_owned(),
            };
        }
        self.take_text(&sender, &text).await;
        TextDecision::Accepted
    }

    /// Asked about like any text; a long one (or one the clipboard cannot
    /// take) is saved as a file.
    async fn receive_localsend_text(&self, message: &LocalSendText) -> LocalSendTextDecision {
        let sender = message.sender.label();
        if !self.ask_text(&sender, &message.text).await {
            return LocalSendTextDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            };
        }
        if message.text.len() <= MAX_TEXT_LEN && self.take_text(&sender, &message.text).await {
            LocalSendTextDecision::Taken
        } else {
            LocalSendTextDecision::SaveAsFile {
                dest_dir: Self::config().download_dir(),
            }
        }
    }

    fn on_event(&self, event: TransferEvent) {
        if event.direction != Direction::Download {
            return;
        }
        match event.status {
            TransferStatus::Completed { .. } => {
                info!("Received '{}'", event.file_name);
                let notifier = self.notifier.clone();
                tokio::spawn(async move {
                    notifier
                        .show(&t!("gui.notify_received"), &event.file_name)
                        .await;
                });
            }
            TransferStatus::Failed { error } => {
                warn!("Receiving '{}' failed: {error}", event.file_name);
            }
            _ => {}
        }
    }
}
