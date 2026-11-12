//! [`TransferHandler`]: what an interface plugs into [`serve`](super::serve).

use super::events::{OfferDecision, TransferEvent};
use crate::localsend::{LocalSendOffer, LocalSendText, LocalSendTextDecision};
use crate::pairing::PairRequest;
use crate::protocol::messages::{BatchOffer, ShareToken, TextDecision, TransferOffer};
use crate::transfer::sender::PreparedFile;
use crate::web::BrowserOffer;
use iroh::EndpointId;
use rust_i18n::t;
use std::future::Future;

/// Interface-specific behaviour plugged into [`serve`](super::serve).
pub trait TransferHandler: Send + Sync + 'static {
    /// Decides about an offer from `peer`: Auto-Accept, ask the user or reject.
    fn decide(
        &self,
        peer: EndpointId,
        transfer_id: &str,
        offer: &TransferOffer,
    ) -> impl Future<Output = OfferDecision> + Send;

    /// File shared under `token` for download by TICKET, if any.
    fn shared_file(&self, _token: &ShareToken) -> Option<PreparedFile> {
        None
    }

    /// Receives every state change of every transfer handled by [`serve`](super::serve).
    fn on_event(&self, event: TransferEvent);

    /// Whether a request from `peer` may reach the user at all: `pairing` is
    /// `true` for a pairing request. A device that is not admitted gets a
    /// refusal (nothing is asked or shown to the user).
    /// The default admits everyone; interfaces follow
    /// [`Config::paired_only`](crate::config::Config::paired_only).
    fn admits(&self, _peer: EndpointId, _pairing: bool) -> impl Future<Output = bool> + Send {
        async { true }
    }

    /// `peer` removed this device from its trusted devices and asked
    /// ([`Hello::Unpair`](crate::protocol::messages::Hello::Unpair)) to be removed here too. Implementations remove it
    /// from the trusted devices and refresh their lists; the default does
    /// nothing.
    fn peer_unpaired(&self, _peer: EndpointId) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Name of this device sent in pairing responses.
    fn device_name(&self) -> String {
        crate::config::Config::load()
            .map(|c| c.device_name())
            .unwrap_or_else(|_| crate::config::paths::default_device_name())
    }

    /// Handles a pairing request.
    ///
    /// Implementations ask the user (showing [`PairRequest::code`]) and on
    /// acceptance store the device, e.g. with
    /// [`Config::add_paired`](crate::config::Config::add_paired). The default
    /// rejects every request (e.g. a daemon without a terminal).
    fn confirm_pair(&self, _request: PairRequest) -> impl Future<Output = bool> + Send {
        async { false }
    }

    /// Handles a text sent by `peer` (at most [`MAX_TEXT_LEN`](crate::protocol::messages::MAX_TEXT_LEN) bytes).
    ///
    /// Implementations accept it automatically for Auto-Accept devices or ask
    /// the user, and on acceptance copy it to the clipboard. The default
    /// declines every text.
    fn receive_text(
        &self,
        _peer: EndpointId,
        _text: String,
    ) -> impl Future<Output = TextDecision> + Send {
        async {
            TextDecision::Rejected {
                reason: t!("service.text_unsupported").into_owned(),
            }
        }
    }

    /// Decides about several files announced together ([`Hello::Batch`](crate::protocol::messages::Hello::Batch)).
    ///
    /// `Some(OfferDecision::Accept { .. })` accepts all announced files at once:
    /// the following offers on the same connection are received without calling
    /// [`TransferHandler::decide`], up to the announced number of files and
    /// total size. `Some(OfferDecision::Reject { .. })` declines all of them.
    /// `None` (the default) keeps asking about every file separately.
    fn decide_batch(
        &self,
        _peer: EndpointId,
        _batch: &BatchOffer,
    ) -> impl Future<Output = Option<OfferDecision>> + Send {
        async { None }
    }

    /// Decides about files offered by a LocalSend app ([`crate::localsend`]).
    ///
    /// The sender is not authenticated: its name is whatever it announces and
    /// anyone on the local network can send such an offer. Implementations
    /// must therefore always ask the user (never Auto-Accept), show that the
    /// offer comes through the LocalSend compatibility mode and warn about
    /// [`LocalSendOffer::risk`]. The default declines.
    fn decide_localsend(
        &self,
        _offer: &LocalSendOffer,
    ) -> impl Future<Output = OfferDecision> + Send {
        async {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }

    /// Handles a text a LocalSend app sends as a message ([`LocalSendText`]).
    ///
    /// The sender is not authenticated (see [`TransferHandler::decide_localsend`]),
    /// so implementations ask the user. The default keeps treating it as a
    /// file offer ([`LocalSendTextDecision::AsOffer`]), which a daemon declines.
    fn receive_localsend_text(
        &self,
        _text: &LocalSendText,
    ) -> impl Future<Output = LocalSendTextDecision> + Send {
        async { LocalSendTextDecision::AsOffer }
    }

    /// Decides about files a web browser wants to send ([`crate::web`]).
    ///
    /// Only the device that opened the session's link can offer them, but the
    /// link travels over plain HTTP, so implementations always ask the user
    /// (never Auto-Accept) and warn about [`BrowserOffer::risk`]. The default
    /// declines.
    fn decide_browser(&self, _offer: &BrowserOffer) -> impl Future<Output = OfferDecision> + Send {
        async {
            OfferDecision::Reject {
                reason: t!("transfer.user_rejected").into_owned(),
            }
        }
    }
}
