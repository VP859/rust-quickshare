//! Questions from the node (offers, pairing, texts) forwarded to the UI loop.

use super::*;

/// What a [`Pending`] decision is about.
pub enum PendingKind {
    /// An incoming file offer.
    File {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// Offered file name.
        file_name: String,
        /// Offered file size in bytes.
        file_size: u64,
    },
    /// Several incoming files announced together.
    Batch {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// Number of files.
        count: u32,
        /// Size of all files in bytes.
        total_size: u64,
        /// Names of the first files.
        names: Vec<String>,
        /// The most serious risk among all files (not only the listed ones).
        risk: Option<ferry::transfer::risk::Risk>,
        /// Offered by a LocalSend app: the sender is not verified.
        localsend: bool,
    },
    /// An incoming text for the clipboard.
    Text {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// One-line preview of the text.
        preview: String,
    },
    /// An incoming pairing request.
    Pair {
        /// Name of the requesting device.
        name: String,
        /// Verification code to compare.
        code: String,
    },
}

/// A decision waiting for `y`/`n`.
pub struct Pending {
    /// Subject of the decision.
    pub kind: PendingKind,
    /// Delivers the answer to the waiting handler.
    pub(super) responder: oneshot::Sender<bool>,
}

impl Pending {
    /// Short description used in status messages.
    pub(super) fn label(&self) -> &str {
        match &self.kind {
            PendingKind::File { file_name, .. } => file_name,
            PendingKind::Batch { peer_label, .. } => peer_label,
            PendingKind::Text { preview, .. } => preview,
            PendingKind::Pair { name, .. } => name,
        }
    }
}

/// Messages from background tasks to the UI loop.
pub enum UiMessage {
    /// A new decision to show.
    Pending(Pending),
    /// Progress of a transfer.
    Transfer(TransferEvent),
    /// The list of nearby devices changed.
    Nearby(Vec<NearbyDevice>),
    /// Text for the status line.
    Status(String),
    /// Trusted devices changed (e.g. after pairing); carries a status text.
    PeersChanged(String),
    /// An accepted text to copy to the clipboard.
    TextReceived {
        /// Alias of the sender or a short Node ID.
        peer_label: String,
        /// The received text.
        text: String,
    },
}

/// [`TransferHandler`] forwarding offers, pairing requests and progress to the UI loop.
pub struct TuiHandler {
    /// Channel to the UI loop.
    pub tx: mpsc::UnboundedSender<UiMessage>,
}

impl TuiHandler {
    /// Queues a decision in the UI and waits for `y`/`n`; no answer within
    /// [`DECISION_TIMEOUT`] means no.
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
    /// Reloads the configuration, so `ferry config` and `ferry pairing` apply
    /// without a restart.
    async fn admits(&self, peer: EndpointId, pairing: bool) -> bool {
        Config::load().map_or(true, |c| c.admits(&peer.to_string(), pairing))
    }

    /// Reloads the configuration per offer, so a changed download folder applies immediately.
    async fn decide(&self, peer: EndpointId, _: &str, offer: &TransferOffer) -> OfferDecision {
        let config = Config::load().unwrap_or_default();
        let dest_dir = config.download_dir();
        let peer_info = config.find_peer(&peer.to_string());
        if config.auto_accepts_file(&peer.to_string(), &offer.file_name) {
            return OfferDecision::Accept { dest_dir };
        }

        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("tui.unknown_device", id = peer.fmt_short()).into_owned());
        let kind = PendingKind::File {
            peer_label,
            file_name: offer.file_name.clone(),
            file_size: offer.file_size,
        };
        if self.ask(kind).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
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
            t!("tui.paired", name = &req.name).into_owned()
        } else {
            t!("tui.pair_save_failed").into_owned()
        }));
        ok
    }

    async fn decide_batch(&self, peer: EndpointId, batch: &BatchOffer) -> Option<OfferDecision> {
        let config = Config::load().unwrap_or_default();
        let dest_dir = config.download_dir();
        let peer_info = config.find_peer(&peer.to_string());
        if config.auto_accepts_batch(&peer.to_string(), &batch.names) {
            return Some(OfferDecision::Accept { dest_dir });
        }
        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("tui.unknown_device", id = peer.fmt_short()).into_owned());
        let kind = PendingKind::Batch {
            peer_label,
            count: batch.count,
            total_size: batch.total_size,
            risk: ferry::transfer::risk::worst_risk(&batch.names),
            names: batch.names.clone(),
            localsend: false,
        };
        Some(if self.ask(kind).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        })
    }

    /// LocalSend senders are not authenticated: always asked, never Auto-Accept.
    async fn decide_localsend(&self, offer: &ferry::localsend::LocalSendOffer) -> OfferDecision {
        let dest_dir = Config::load().unwrap_or_default().download_dir();
        let kind = PendingKind::Batch {
            peer_label: format!("{} ({})", offer.sender.label(), offer.sender.address),
            count: offer.count,
            total_size: offer.total_size,
            names: offer.names.clone(),
            risk: offer.risk,
            localsend: true,
        };
        if self.ask(kind).await {
            OfferDecision::Accept { dest_dir }
        } else {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    async fn receive_text(&self, peer: EndpointId, text: String) -> TextDecision {
        let config = Config::load().unwrap_or_default();
        let peer_info = config.find_peer(&peer.to_string());
        let peer_label = peer_info
            .map(|p| p.alias.clone())
            .unwrap_or_else(|| t!("tui.unknown_device", id = peer.fmt_short()).into_owned());
        if !config.auto_accepts_texts_from(&peer.to_string()) {
            let kind = PendingKind::Text {
                peer_label: peer_label.clone(),
                preview: service::text_preview(&text, 300),
            };
            if !self.ask(kind).await {
                return TextDecision::Rejected {
                    reason: t!("transfer.user_rejected").into_owned(),
                };
            }
        }
        let _ = self.tx.send(UiMessage::TextReceived { peer_label, text });
        TextDecision::Accepted
    }

    fn on_event(&self, event: TransferEvent) {
        let _ = self.tx.send(UiMessage::Transfer(event));
    }
}
