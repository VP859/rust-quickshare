//! The P2P endpoint: end-to-end encrypted QUIC connections between devices.
//!
//! The [`NetworkMode`] decides whether external servers may be used:
//!
//! - [`NetworkMode::Local`] (default): no external servers. Devices connect
//!   directly to the addresses learned from local network discovery or carried
//!   by a TICKET, so traffic never leaves the network path between them.
//! - [`NetworkMode::Internet`]: additionally the free public relays and the DNS
//!   address lookup of n0 (the iroh developers) are used, so paired devices can
//!   reach each other from different networks. Relays forward only encrypted
//!   packets, but they see IP addresses and Node IDs.

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, Watcher};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, info};

use super::NodeTicket;

/// ALPN of the RQS protocol; must change whenever the encoding of an existing
/// message changes (new trailing enum variants keep it).
pub const ALPN_RQS: &[u8] = b"rqs/0.2";

/// Preferred UDP port of QUIC connections, next to the discovery port, so one
/// firewall rule (`47823-47824/udp`) covers discovery and direct LAN transfers.
/// When it is taken (e.g. a second instance on the same computer) a random
/// port is used.
pub const QUIC_PORT: u16 = 47824;

/// How long to wait for the first local address before building a TICKET.
const ADDR_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for a relay connection before building a TICKET in
/// [`NetworkMode::Internet`], so the TICKET includes the relay URL.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether connections may use servers on the internet.
///
/// Encryption is the same in both modes: QUIC with TLS 1.3, authenticated by
/// the Node ID. Speed is the same whenever a direct connection is possible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// Direct connections only; nothing is sent to external servers. Devices
    /// must be on the same network (or reachable directly, e.g. through a VPN).
    #[default]
    Local,
    /// Direct connections when possible, otherwise through the public relays
    /// of n0; addresses are also published and looked up through n0's DNS
    /// server. Works between different networks without hosting anything.
    Internet,
}

impl NetworkMode {
    /// Both modes, in display order.
    pub const ALL: [NetworkMode; 2] = [NetworkMode::Local, NetworkMode::Internet];

    /// Name used in the configuration file and on the command line.
    pub fn code(self) -> &'static str {
        match self {
            NetworkMode::Local => "local",
            NetworkMode::Internet => "internet",
        }
    }

    /// Whether this is [`NetworkMode::Local`] (used to omit the default from `config.toml`).
    pub fn is_local(&self) -> bool {
        *self == NetworkMode::Local
    }
}

impl fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for NetworkMode {
    type Err = anyhow::Error;

    /// Parses `local` or `internet` (case-insensitive).
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.code().eq_ignore_ascii_case(s.trim()))
            .with_context(|| t!("engine.bad_mode", mode = s))
    }
}

/// Thin wrapper over [`iroh::Endpoint`] (QUIC with TLS 1.3).
///
/// Cheap to clone: all clones share the same endpoint.
#[derive(Clone, Debug)]
pub struct NodeEngine {
    /// The iroh endpoint shared by all clones.
    endpoint: Endpoint,
    /// Whether relays and DNS address lookup are used.
    mode: NetworkMode,
}

impl NodeEngine {
    /// Binds a new endpoint in [`NetworkMode::Local`]: no relay servers and no
    /// DNS/pkarr address lookup, so it never contacts servers on the internet.
    ///
    /// # Errors
    ///
    /// Fails when the endpoint cannot bind its UDP sockets.
    pub async fn new(secret_key: SecretKey) -> Result<Self> {
        Self::with_mode(secret_key, NetworkMode::Local).await
    }

    /// Binds a new endpoint with the given identity and [`NetworkMode`], on
    /// IPv4 port [`QUIC_PORT`] when it is free.
    ///
    /// # Errors
    ///
    /// Fails when the endpoint cannot bind its UDP sockets.
    pub async fn with_mode(secret_key: SecretKey, mode: NetworkMode) -> Result<Self> {
        let builder = |key: SecretKey| {
            let builder = match mode {
                NetworkMode::Local => {
                    Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
                }
                NetworkMode::Internet => Endpoint::builder(presets::N0),
            };
            builder.secret_key(key).alpns(vec![ALPN_RQS.to_vec()])
        };
        let preferred = match builder(secret_key.clone())
            .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, QUIC_PORT)))
        {
            Ok(b) => b.bind().await.ok(),
            Err(_) => None,
        };
        let endpoint = match preferred {
            Some(endpoint) => endpoint,
            None => {
                debug!("UDP {QUIC_PORT} is taken, binding QUIC to a random port");
                builder(secret_key)
                    .bind()
                    .await
                    .with_context(|| t!("engine.bind"))?
            }
        };
        info!(
            "P2P node started in {mode} mode, Node ID {}, sockets {:?}",
            endpoint.id(),
            endpoint.bound_sockets()
        );
        Ok(Self { endpoint, mode })
    }

    /// A node using the persistent identity and the [`NetworkMode`] saved in
    /// the configuration directory.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be loaded or the endpoint cannot bind.
    pub async fn with_persistent_identity() -> Result<Self> {
        let mode = crate::config::Config::load()
            .map(|c| c.network_mode)
            .unwrap_or_default();
        Self::with_mode(super::load_or_generate_default_key()?, mode).await
    }

    /// The network mode this node was started with.
    pub fn mode(&self) -> NetworkMode {
        self.mode
    }

    /// Current address of this node: its Node ID, IP addresses and, in
    /// [`NetworkMode::Internet`], its relay URL.
    ///
    /// Right after startup it waits briefly until the address is usable: at
    /// most 5 s for a relay connection in internet mode, otherwise at most 2 s
    /// for the first IP address.
    ///
    /// # Errors
    ///
    /// Currently infallible; kept fallible for API stability.
    pub async fn node_addr(&self) -> Result<EndpointAddr> {
        if self.mode == NetworkMode::Internet {
            let _ = tokio::time::timeout(ONLINE_TIMEOUT, self.endpoint.online()).await;
            return Ok(self.endpoint.addr());
        }
        let mut watcher = self.endpoint.watch_addr();
        let with_ip = async {
            loop {
                let addr = watcher.get();
                if addr.ip_addrs().next().is_some() || watcher.updated().await.is_err() {
                    return addr;
                }
            }
        };
        Ok(tokio::time::timeout(ADDR_TIMEOUT, with_ip)
            .await
            .unwrap_or_else(|_| self.endpoint.addr()))
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
        let conn = incoming.await.with_context(|| t!("engine.incoming"))?;
        info!("Incoming connection from {}", conn.remote_id());
        Ok(Some(conn))
    }

    /// Connects to `target` using the RQS ALPN.
    ///
    /// # Errors
    ///
    /// Fails when the node cannot be reached at any of its known addresses (and,
    /// in [`NetworkMode::Internet`], not through a relay either).
    pub async fn connect(&self, target: impl Into<EndpointAddr>) -> Result<Connection> {
        let target = target.into();
        debug!("Connecting to {}", target.id);
        let conn = self
            .endpoint
            .connect(target, ALPN_RQS)
            .await
            .with_context(|| t!("engine.connect"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn network_mode_parses_and_defaults_to_local() {
        assert_eq!(
            "internet".parse::<NetworkMode>().unwrap(),
            NetworkMode::Internet
        );
        assert_eq!(
            " LOCAL ".parse::<NetworkMode>().unwrap(),
            NetworkMode::Local
        );
        assert!("relay".parse::<NetworkMode>().is_err());
        assert_eq!(NetworkMode::default(), NetworkMode::Local);
    }

    #[test]
    fn network_mode_in_config_file() {
        let old: Config = toml::from_str("[peers]\n").unwrap();
        assert_eq!(
            old.network_mode,
            NetworkMode::Local,
            "old configs stay local"
        );
        assert!(!toml::to_string(&old).unwrap().contains("network_mode"));

        let config = Config {
            network_mode: NetworkMode::Internet,
            ..Config::default()
        };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("network_mode = \"internet\""), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.network_mode, NetworkMode::Internet);
    }

    #[tokio::test]
    async fn local_node_has_no_relay_address() -> Result<()> {
        let engine = NodeEngine::new(SecretKey::generate()).await?;
        assert_eq!(engine.mode(), NetworkMode::Local);
        let addr = engine.node_addr().await?;
        assert_eq!(addr.relay_urls().count(), 0);
        assert!(addr.ip_addrs().next().is_some(), "no direct address");
        engine.close().await;
        Ok(())
    }
}
