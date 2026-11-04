//! Sharing one file by TICKET with a one-time identity.
//!
//! A [`Share`] starts its own node with a **random, throw-away key**
//! ([`NodeEngine::one_time`]) instead of using the device's permanent one. The
//! TICKET it prints (or shows as a QR code) therefore names this node only:
//!
//! * whoever receives it, and whoever it is passed on to, learns nothing that
//!   identifies the device – two shares from one device cannot be linked;
//! * the Node ID in an old TICKET cannot be used later to contact the device or
//!   to send it offers, because the node is gone when the share ends;
//! * the share serves exactly one file, offers sent to it are declined, and it
//!   ends after [`ShareOptions::lifetime`] or, with [`ShareOptions::single_use`],
//!   after the first complete download.
//!
//! The recipient's IP address is visible to the sender as in any transfer, and
//! the device's IP address to the recipient; the identity is what is kept apart.

use anyhow::Result;
use iroh::EndpointId;
use rust_i18n::t;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::sender::PreparedFile;
use super::service::{
    self, Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use crate::node::{Network, NodeEngine, NodeTicket};
use crate::protocol::messages::{ShareToken, TransferOffer};

/// How long and how often a [`Share`] can be downloaded.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShareOptions {
    /// The share ends after this time; `None` keeps it until stopped.
    pub lifetime: Option<Duration>,
    /// The share ends after the first complete download. An interrupted
    /// download can be resumed; a second download is not possible.
    pub single_use: bool,
}

/// Receiver of the transfer events of a share.
pub type ShareSink = Arc<dyn Fn(TransferEvent) + Send + Sync>;

/// A file shared by a TICKET, served by a node with a one-time identity.
pub struct Share {
    /// The TICKET to give to the recipient.
    ticket: NodeTicket,
    /// Name of the shared file.
    file_name: String,
    /// Node ID of the one-time node.
    node_id: EndpointId,
    /// Ends the share.
    stop: CancellationToken,
    /// The task serving the file; finishes when the share ends.
    task: JoinHandle<()>,
}

impl Share {
    /// Starts a node with a new identity and serves `file` from it.
    ///
    /// `sink` receives the transfer events (the upload to the recipient).
    ///
    /// # Errors
    ///
    /// Fails when the node cannot start or its address cannot be determined.
    pub async fn start(
        file: PreparedFile,
        network: impl Into<Network>,
        options: ShareOptions,
        sink: ShareSink,
    ) -> Result<Self> {
        let engine = NodeEngine::one_time(network).await?;
        let token = service::new_share_token();
        let ticket = NodeTicket::with_share(engine.node_addr().await?, token);
        let stop = CancellationToken::new();
        let file_name = file.name.clone();
        let handler = Arc::new(ShareHandler {
            token,
            file,
            sink,
            single_use: options.single_use,
            stop: stop.clone(),
        });
        let node_id = engine.node_id();
        let ends = stop.clone();
        let task = tokio::spawn(async move {
            let expired = async {
                match options.lifetime {
                    Some(lifetime) => tokio::time::sleep(lifetime).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = service::serve(engine.clone(), handler) => {}
                () = ends.cancelled() => {}
                () = expired => {}
            }
            engine.close().await;
        });
        Ok(Self {
            ticket,
            file_name,
            node_id,
            stop,
            task,
        })
    }

    /// The TICKET (`ferry:…`) to give to the recipient.
    ///
    /// # Errors
    ///
    /// Fails only if the TICKET cannot be serialized.
    pub fn ticket(&self) -> Result<String> {
        self.ticket.serialize_to_string()
    }

    /// Name of the shared file.
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Node ID of the one-time node (not the device's own).
    pub fn node_id(&self) -> EndpointId {
        self.node_id
    }

    /// Ends the share now.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Whether the share has ended (expired, downloaded or stopped).
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Waits until the share ends.
    pub async fn finished(&mut self) {
        let _ = (&mut self.task).await;
    }
}

impl Drop for Share {
    /// Dropping a share ends it: the node and its identity are discarded.
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Serves one file under a token and declines everything else.
struct ShareHandler {
    /// Token from the TICKET.
    token: ShareToken,
    /// The shared file.
    file: PreparedFile,
    /// Receives the transfer events.
    sink: ShareSink,
    /// End the share after the first complete download.
    single_use: bool,
    /// Ends the share.
    stop: CancellationToken,
}

impl TransferHandler for ShareHandler {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        OfferDecision::Reject {
            reason: t!("share.only").into_owned(),
        }
    }

    fn shared_file(&self, token: &ShareToken) -> Option<PreparedFile> {
        (token == &self.token).then(|| self.file.clone())
    }

    fn on_event(&self, event: TransferEvent) {
        let delivered = event.direction == Direction::Upload
            && matches!(event.status, TransferStatus::Completed { .. });
        (self.sink)(event);
        if delivered && self.single_use {
            self.stop.cancel();
        }
    }
}
