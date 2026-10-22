use anyhow::Result;
#[cfg(not(target_os = "android"))]
use arboard::Clipboard;
use iroh::EndpointId;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use crate::config::Config;
use crate::discovery::{Discovery, DiscoveryOptions, NearbyDevice};
use crate::node::NodeEngine;
use crate::pairing::{PairOutcome, PairRequest, pairing_code, request_pairing};
use crate::protocol::messages::TransferOffer;
use crate::transfer::service::{OfferDecision, TransferEvent, TransferHandler};

/// Czas na decyzję w TUI, po którym oferta jest odrzucana.
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

pub enum PendingKind {
    File {
        peer_label: String,
        file_name: String,
        file_size: u64,
    },
    Pair {
        name: String,
        code: String,
    },
}

/// Decyzja oczekująca na [y]/[n] – oferta pliku lub prośba o sparowanie.
pub struct Pending {
    pub kind: PendingKind,
    responder: oneshot::Sender<bool>,
}

impl Pending {
    fn label(&self) -> &str {
        match &self.kind {
            PendingKind::File { file_name, .. } => file_name,
            PendingKind::Pair { name, .. } => name,
        }
    }
}

pub enum UiMessage {
    Pending(Pending),
    Transfer(TransferEvent),
    Nearby(Vec<NearbyDevice>),
    /// Zmieniła się lista zaufanych urządzeń (np. po parowaniu).
    PeersChanged(String),
}

/// Handler węzła przekazujący oferty, parowanie i postęp do pętli TUI.
pub struct TuiHandler {
    pub tx: mpsc::UnboundedSender<UiMessage>,
    pub dest_dir: PathBuf,
}

impl TuiHandler {
    async fn ask(&self, kind: PendingKind) -> bool {
        let (responder, answer) = oneshot::channel();
        let _ = self
            .tx
            .send(UiMessage::Pending(Pending { kind, responder }));
        matches!(
            tokio::time::timeout(DECISION_TIMEOUT, answer).await,
            Ok(Ok(true))
        )
    }
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

        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| peer.fmt_short().to_string());
        let kind = PendingKind::File {
            peer_label,
            file_name: offer.file_name.clone(),
            file_size: offer.file_size,
        };
        if self.ask(kind).await {
            OfferDecision::Accept {
                dest_dir: self.dest_dir.clone(),
            }
        } else {
            OfferDecision::Reject {
                reason: "Użytkownik odrzucił transfer".into(),
            }
        }
    }

    async fn confirm_pair(&self, req: PairRequest) -> bool {
        let kind = PendingKind::Pair {
            name: req.name.clone(),
            code: req.code.clone(),
        };
        if !self.ask(kind).await {
            return false;
        }
        let saved = Config::load().and_then(|mut c| {
            c.add_paired(req.peer, &req.name, req.addr, false)?;
            c.save()
        });
        let ok = saved.is_ok();
        let _ = self.tx.send(UiMessage::PeersChanged(if ok {
            format!("Sparowano z '{}'", req.name)
        } else {
            "Błąd zapisu sparowanego urządzenia".to_string()
        }));
        ok
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.tx.send(UiMessage::Transfer(event));
    }
}

pub struct App {
    pub engine: NodeEngine,
    pub discovery: Option<Discovery>,
    pub node_id: String,
    pub ticket_str: String,
    pub status_msg: String,
    pub config: Config,
    /// Transfery w kolejności pojawienia się (najnowsze na końcu).
    pub transfers: Vec<TransferEvent>,
    pub pending: VecDeque<Pending>,
    pub nearby: Vec<NearbyDevice>,
    pub selected: usize,
    tx: mpsc::UnboundedSender<UiMessage>,
}

impl App {
    pub async fn new(tx: mpsc::UnboundedSender<UiMessage>) -> Result<Self> {
        let config = Config::load().unwrap_or_default();
        let engine = NodeEngine::with_persistent_identity().await?;
        let node_id = engine.node_id().to_string();
        let ticket_str = engine.ticket().await?.serialize_to_string()?;
        let discovery = Discovery::start(
            engine.clone(),
            &config.device_name(),
            DiscoveryOptions::default(),
        )
        .ok();

        Ok(Self {
            engine,
            discovery,
            node_id,
            ticket_str,
            status_msg:
                "Węzeł gotowy. [↑↓] wybór, [p] sparuj, [r] szukaj, [c] kopiuj TICKET, [q] wyjście."
                    .to_string(),
            config,
            transfers: Vec::new(),
            pending: VecDeque::new(),
            nearby: Vec::new(),
            selected: 0,
            tx,
        })
    }

    pub fn handle_message(&mut self, msg: UiMessage) {
        match msg {
            UiMessage::Pending(p) => {
                self.status_msg = match &p.kind {
                    PendingKind::File { file_name, .. } => format!("Nowa oferta: {file_name}"),
                    PendingKind::Pair { name, .. } => format!("Prośba o sparowanie: {name}"),
                };
                self.pending.push_back(p);
            }
            UiMessage::Transfer(ev) => match self.transfers.iter_mut().find(|t| t.id == ev.id) {
                Some(existing) => *existing = ev,
                None => self.transfers.push(ev),
            },
            UiMessage::Nearby(devices) => {
                self.nearby = devices;
                self.selected = self.selected.min(self.nearby.len().saturating_sub(1));
            }
            UiMessage::PeersChanged(status) => {
                self.config = Config::load().unwrap_or_default();
                self.status_msg = status;
            }
        }
    }

    pub fn select(&mut self, delta: isize) {
        if self.nearby.is_empty() {
            return;
        }
        let len = self.nearby.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    pub fn refresh_nearby(&mut self) {
        if let Some(d) = &self.discovery {
            d.probe();
            self.status_msg = "Szukanie urządzeń w sieci lokalnej...".into();
        } else {
            self.status_msg = "Wykrywanie w sieci lokalnej jest niedostępne.".into();
        }
    }

    /// Paruje z zaznaczonym urządzeniem „W pobliżu” (w tle – TUI pozostaje responsywne).
    pub fn pair_selected(&mut self) {
        let Some(device) = self.nearby.get(self.selected).cloned() else {
            self.status_msg = "Brak urządzeń w pobliżu do sparowania.".into();
            return;
        };
        let code = pairing_code(&self.engine.node_id(), &device.id);
        self.status_msg = format!(
            "Parowanie z '{}' – kod {code}. Potwierdź na drugim urządzeniu...",
            device.name
        );
        let engine = self.engine.clone();
        let my_name = self.config.device_name();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let status = match request_pairing(&engine, device.addr.clone(), &my_name).await {
                Ok(PairOutcome::Accepted { name, addr }) => {
                    let saved = Config::load().and_then(|mut c| {
                        c.add_paired(device.id, &name, addr.or(Some(device.addr)), false)?;
                        c.save()
                    });
                    match saved {
                        Ok(()) => format!("Sparowano z '{name}'"),
                        Err(e) => format!("Błąd zapisu: {e:#}"),
                    }
                }
                Ok(PairOutcome::Rejected) => format!("'{}' odrzuciło parowanie", device.name),
                Err(e) => format!("Parowanie nieudane: {e:#}"),
            };
            let _ = tx.send(UiMessage::PeersChanged(status));
        });
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

    pub fn answer_pending(&mut self, accept: bool) {
        if let Some(p) = self.pending.pop_front() {
            let label = p.label().to_string();
            let _ = p.responder.send(accept);
            self.status_msg = if accept {
                format!("Zaakceptowano: {label}")
            } else {
                format!("Odrzucono: {label}")
            };
        }
    }
}
