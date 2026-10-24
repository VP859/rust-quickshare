//! The P2P endpoint: QUIC connections with hole punching and relay fallback.

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use rust_i18n::t;
use std::time::Duration;
use tracing::{debug, info};

use super::NodeTicket;

/// ALPN of the RQS protocol; must change whenever the wire format changes.
pub const ALPN_RQS: &[u8] = b"rqs/0.2";

/// How long to wait for a relay connection before building a TICKET.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Thin wrapper over [`iroh::Endpoint`] (QUIC, hole punching, encrypted relay).
///
/// Cheap to clone: all clones share the same endpoint.
#[derive(Clone, Debug)]
pub struct NodeEngine {
    endpoint: Endpoint,
}

impl NodeEngine {
    /// Binds a new endpoint with the given identity and the default n0 relays
    /// and address lookup.
    ///
    /// # Errors
    ///
    /// Fails when the endpoint cannot bind its UDP sockets.
    pub async fn new(secret_key: SecretKey) -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            .alpns(vec![ALPN_RQS.to_vec()])
            .bind()
            .await
            .context(t!("engine.bind"))?;
        info!("P2P node started, Node ID {}", endpoint.id());
        Ok(Self { endpoint })
    }

    /// A node using the persistent identity from the configuration directory.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be loaded or the endpoint cannot bind.
    pub async fn with_persistent_identity() -> Result<Self> {
        Self::new(super::load_or_generate_default_key()?).await
    }

    /// Current address of this node.
    ///
    /// Waits briefly for a relay connection so that the address (and any TICKET
    /// built from it) includes the relay URL.
    ///
    /// # Errors
    ///
    /// Currently infallible; kept fallible for API stability.
    pub async fn node_addr(&self) -> Result<EndpointAddr> {
        let _ = tokio::time::timeout(ONLINE_TIMEOUT, self.endpoint.online()).await;
        Ok(self.endpoint.addr())
    }

    /// A pairing TICKET for this node.
    ///
    /// # Errors
    ///
    /// See [`NodeEngine::node_addr`].
    pub async fn ticket(&self) -> Result<NodeTicket> {
        Ok(NodeTicket::new(self.node_addr().await?))
    }

    /// Waits for the next incoming connection; `Ok(None)` once the endpoint is closed.
    ///
    /// # Errors
    ///
    /// Fails when the handshake of the incoming connection fails.
    pub async fn accept(&self) -> Result<Option<Connection>> {
        debug!("Waiting for an incoming QUIC connection");
        let Some(incoming) = self.endpoint.accept().await else {
            return Ok(None);
        };
        let conn = incoming.await.context(t!("engine.incoming"))?;
        info!("Incoming connection from {}", conn.remote_id());
        Ok(Some(conn))
    }

    /// Connects to `target` using the RQS ALPN.
    ///
    /// # Errors
    ///
    /// Fails when the node cannot be reached directly or through a relay.
    pub async fn connect(&self, target: impl Into<EndpointAddr>) -> Result<Connection> {
        let target = target.into();
        debug!("Connecting to {}", target.id);
        let conn = self
            .endpoint
            .connect(target, ALPN_RQS)
            .await
            .context(t!("engine.connect"))?;
        info!("Connected to {}", conn.remote_id());
        Ok(conn)
    }

    /// Checks whether `target` is reachable by opening and immediately closing a connection.
    pub async fn ping(&self, target: impl Into<EndpointAddr>, timeout: Duration) -> bool {
        match tokio::time::timeout(timeout, self.connect(target)).await {
            Ok(Ok(conn)) => {
                conn.close(0u32.into(), b"ping");
                true
            }
            _ => false,
        }
    }

    /// Node ID (public key) of this node.
    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// The underlying iroh endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Gracefully closes all connections and the endpoint.
    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}
