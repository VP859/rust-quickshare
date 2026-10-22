use anyhow::Result;
#[cfg(not(target_os = "android"))]
use arboard::Clipboard;
use iroh::EndpointId;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use crate::config::Config;
use crate::node::NodeEngine;
use crate::protocol::messages::TransferOffer;
use crate::transfer::service::{OfferDecision, TransferEvent, TransferHandler};

/// Czas na decyzję w TUI, po którym oferta jest odrzucana.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

pub struct PendingOffer {
    pub peer_label: String,
    pub file_name: String,
    pub file_size: u64,
    responder: oneshot::Sender<bool>,
}

pub enum UiMessage {
    Offer(PendingOffer),
    Transfer(TransferEvent),
}

/// Handler węzła przekazujący oferty i postęp do pętli TUI.
pub struct TuiHandler {
    pub tx: mpsc::UnboundedSender<UiMessage>,
    pub dest_dir: PathBuf,
}

impl TransferHandler for TuiHandler {
    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        let config = Config::load().unwrap_or_default();
        let peer_info = config.find_peer(&peer.to_string());
        if peer_info.is_some_and(|p| p.auto_accept) {
            return OfferDecision::Accept {
                dest_dir: self.dest_dir.clone(),
            };
        }

        let (responder, answer) = oneshot::channel();
        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| peer.fmt_short().to_string());
        let _ = self.tx.send(UiMessage::Offer(PendingOffer {
            peer_label,
            file_name: offer.file_name.clone(),
            file_size: offer.file_size,
            responder,
        }));
        match tokio::time::timeout(DECISION_TIMEOUT, answer).await {
            Ok(Ok(true)) => OfferDecision::Accept {
                dest_dir: self.dest_dir.clone(),
            },
            _ => OfferDecision::Reject {
                reason: "Użytkownik odrzucił transfer".into(),
            },
        }
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.tx.send(UiMessage::Transfer(event));
    }
}

pub struct App {
    pub engine: NodeEngine,
    pub node_id: String,
    pub ticket_str: String,
    pub status_msg: String,
    pub config: Config,
    /// Transfery w kolejności pojawienia się (najnowsze na końcu).
    pub transfers: Vec<TransferEvent>,
    pub pending: VecDeque<PendingOffer>,
}

impl App {
    pub async fn new() -> Result<Self> {
        let config = Config::load().unwrap_or_default();
        let engine = NodeEngine::with_persistent_identity().await?;
        let node_id = engine.node_id().to_string();
        let ticket_str = engine.ticket().await?.serialize_to_string()?;

        Ok(Self {
            engine,
            node_id,
            ticket_str,
            status_msg: "Węzeł gotowy. [c] kopiuj TICKET, [q] wyjście.".to_string(),
            config,
            transfers: Vec::new(),
            pending: VecDeque::new(),
        })
    }

    pub fn handle_message(&mut self, msg: UiMessage) {
        match msg {
            UiMessage::Offer(offer) => {
                self.status_msg = format!("Nowa oferta: {}", offer.file_name);
                self.pending.push_back(offer);
            }
            UiMessage::Transfer(ev) => match self.transfers.iter_mut().find(|t| t.id == ev.id) {
                Some(existing) => *existing = ev,
                None => self.transfers.push(ev),
            },
        }
    }

    pub fn copy_ticket_to_clipboard(&mut self) {
        #[cfg(not(target_os = "android"))]
        {
            if let Ok(mut clipboard) = Clipboard::new()
                && clipboard.set_text(&self.ticket_str).is_ok()
            {
                self.status_msg = "Skopiowano TICKET do schowka!".to_string();
                return;
            }
            self.status_msg = "Błąd kopiowania do schowka.".to_string();
        }

        #[cfg(target_os = "android")]
        {
            self.status_msg = "Schowek niedostępny w trybie tekstowym na Androidzie.".to_string();
        }
    }

    pub fn answer_pending_offer(&mut self, accept: bool) {
        if let Some(offer) = self.pending.pop_front() {
            let _ = offer.responder.send(accept);
            self.status_msg = if accept {
                format!("Zaakceptowano: {}", offer.file_name)
            } else {
                format!("Odrzucono: {}", offer.file_name)
            };
        }
    }
}
