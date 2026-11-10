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
//! - [`NetworkMode::Relay`]: like the internet mode, but only through relays
//!   the user runs or trusts ([`Network::relays`]) and without n0's address
//!   lookup: devices find each other through local discovery and the address
//!   (with the relay URL) saved at pairing or carried by a TICKET.
//! - [`NetworkMode::RelayOnly`]: every connection goes through the user's
//!   relays, even within one network. The endpoint has no UDP socket for
//!   direct paths, so the other device never learns this device's IP address
//!   from Ferry's connections (only the relay sees it) and nothing on the
//!   path sees which two devices exchange data. Slower than a direct path.

use anyhow::{Context, Result};
use iroh::endpoint::{ConnectOptions, Connection, QuicTransportConfig, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, Watcher};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

use super::NodeTicket;
use crate::error::IncompatibleVersion;
use crate::protocol::version::{self, ProtocolVersion};

/// ALPN of the newest Ferry protocol version
/// ([`ProtocolVersion::CURRENT`]). Connections accept every supported version
/// and offer all of them (see [`crate::protocol::version`]); this constant is
/// for code that needs one identifier, such as tests with their own endpoints.
pub const ALPN: &[u8] = ProtocolVersion::CURRENT.alpn();

/// UDP ports of the nodes that share one file by TICKET ([`NodeEngine::one_time`]):
/// the ten after [`QUIC_PORT`]. The first free one is used, several shares at
/// once take several.
pub const SHARE_PORTS: std::ops::RangeInclusive<u16> = QUIC_PORT + 1..=QUIC_PORT + 10;

/// Preferred UDP port of QUIC connections, next to the discovery port, so one
/// firewall rule (`47823-47834/udp`, with [`SHARE_PORTS`]) covers discovery and direct LAN transfers.
/// When it is taken (e.g. a second instance on the same computer) a random
/// port is used.
pub const QUIC_PORT: u16 = 47824;

/// How long to wait for the first local address before building a TICKET.
const ADDR_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for a relay connection before building a TICKET in
/// [`NetworkMode::Internet`] or [`NetworkMode::Relay`], so the TICKET includes
/// the relay URL.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether connections may use servers on the internet.
///
/// Encryption is the same in every mode: QUIC with TLS 1.3, authenticated by
/// the Node ID. Speed is the same whenever a direct connection is possible
/// (never in [`NetworkMode::RelayOnly`]).
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
    /// Direct connections when possible, otherwise through the relays in
    /// [`Network::relays`] (e.g. one the user runs); no n0 server is
    /// contacted. Paired devices are reached at the address saved at
    /// pairing, which contains their relay URL.
    Relay,
    /// Only through the relays in [`Network::relays`], never directly: no IP
    /// transport at all, so peers see only the relay's address. Otherwise
    /// like [`NetworkMode::Relay`].
    #[serde(rename = "relay-only")]
    RelayOnly,
}

impl NetworkMode {
    /// All modes, in display order.
    pub const ALL: [NetworkMode; 4] = [
        NetworkMode::Local,
        NetworkMode::Internet,
        NetworkMode::Relay,
        NetworkMode::RelayOnly,
    ];

    /// Name used in the configuration file and on the command line.
    pub fn code(self) -> &'static str {
        match self {
            NetworkMode::Local => "local",
            NetworkMode::Internet => "internet",
            NetworkMode::Relay => "relay",
            NetworkMode::RelayOnly => "relay-only",
        }
    }

    /// Whether the relays in [`Network::relays`] are used
    /// ([`NetworkMode::Relay`] and [`NetworkMode::RelayOnly`]).
    pub fn own_relays(self) -> bool {
        matches!(self, NetworkMode::Relay | NetworkMode::RelayOnly)
    }

    /// Whether direct connections are possible (all modes but
    /// [`NetworkMode::RelayOnly`]).
    pub fn direct(self) -> bool {
        self != NetworkMode::RelayOnly
    }

    /// Whether relay servers are used (and a TICKET waits for one).
    pub fn uses_relays(self) -> bool {
        self != NetworkMode::Local
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

    /// Parses `local`, `internet`, `relay` or `relay-only` (case-insensitive).
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.code().eq_ignore_ascii_case(s.trim()))
            .with_context(|| t!("engine.bad_mode", mode = s))
    }
}

/// The network mode with the relays of [`NetworkMode::Relay`] and
/// [`NetworkMode::RelayOnly`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Network {
    /// Whether and which servers may be used.
    pub mode: NetworkMode,
    /// Relays of [`NetworkMode::Relay`] and [`NetworkMode::RelayOnly`];
    /// ignored in the other modes.
    pub relays: Vec<RelayUrl>,
    /// Listen on a random UDP port instead of [`QUIC_PORT`]
    /// ([`Config::random_port`](crate::config::Config::random_port)).
    pub random_port: bool,
}

impl From<NetworkMode> for Network {
    fn from(mode: NetworkMode) -> Self {
        Self {
            mode,
            relays: Vec::new(),
            random_port: false,
        }
    }
}

impl Network {
    /// The network saved in `config` (`network_mode`, `relay_urls`).
    ///
    /// # Errors
    ///
    /// Fails in [`NetworkMode::Relay`] or [`NetworkMode::RelayOnly`] when no
    /// relay is set or a URL is invalid.
    pub fn from_config(config: &crate::config::Config) -> Result<Self> {
        let relays = if config.network_mode.own_relays() {
            let relays = config
                .relay_urls
                .iter()
                .map(|u| parse_relay_url(u))
                .collect::<Result<Vec<_>>>()?;
            if relays.is_empty() {
                anyhow::bail!(t!("engine.no_relay"));
            }
            relays
        } else {
            Vec::new()
        };
        Ok(Self {
            mode: config.network_mode,
            relays,
            random_port: config.random_port,
        })
    }
}

/// Checks the URL of a relay: `https://`, or `http://` only to an IP address
/// on this computer or the local network (a relay in development mode), since
/// plain HTTP would show the Node IDs to everyone on the way.
///
/// # Errors
///
/// Fails for anything else, with the reason.
pub fn parse_relay_url(text: &str) -> Result<RelayUrl> {
    let text = text.trim();
    let url: RelayUrl = text
        .parse()
        .map_err(|_| anyhow::anyhow!(t!("engine.bad_relay", url = text)))?;
    let local_ip = |host: &str| {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| match ip {
                IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
                IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
            })
    };
    match (url.scheme(), url.host_str()) {
        ("https", Some(_)) => Ok(url),
        ("http", Some(host)) if local_ip(host) => Ok(url),
        ("http", Some(_)) => anyhow::bail!(t!("engine.relay_http", url = text)),
        _ => anyhow::bail!(t!("engine.bad_relay", url = text)),
    }
}

/// Thin wrapper over [`iroh::Endpoint`] (QUIC with TLS 1.3).
///
/// Cheap to clone: all clones share the same endpoint.
#[derive(Clone, Debug)]
pub struct NodeEngine {
    /// The iroh endpoint shared by all clones.
    endpoint: Endpoint,
    /// Whether relays and DNS address lookup are used, and which relays.
    network: Network,
}

/// Turns a failed connection attempt into the error shown to the user: a peer
/// that speaks none of our protocol versions gets its own, actionable message.
fn connect_error(error: anyhow::Error) -> anyhow::Error {
    if crate::error::is_no_common_protocol(&error) {
        tracing::debug!("No common protocol version: {error:#}");
        return IncompatibleVersion(t!("engine.incompatible").into_owned()).into();
    }
    error.context(t!("engine.connect"))
}

/// QUIC settings of a node in `mode`: iroh's defaults with the congestion
/// control that suits the network.
///
/// Measured with `tests/network_conditions.rs` (3 MiB, 40 ms round trip) and
/// the loopback benchmark (64 MiB, no loss), release build:
///
/// | | 0–0.5 % loss | 1 % loss | 3 % loss | loopback |
/// |---|---|---|---|---|
/// | Cubic | 0.6 s | 5.7 s | 13 s | 347 MiB/s |
/// | BBRv3 | 0.6 s | 0.8 s | 0.7 s | 149 MiB/s |
///
/// Loss-based Cubic halves its rate on every lost packet, so on mobile and
/// long-distance paths – where packets are lost without congestion – it stays
/// far below the link speed; BBRv3 does not. On a local network Wi-Fi and
/// Ethernet retransmit lost frames themselves, losses are rare, and Cubic
/// reaches higher speeds on fast links. So [`NetworkMode::Local`] uses Cubic
/// and the modes with relays BBRv3.
pub fn transport_config(mode: NetworkMode) -> QuicTransportConfig {
    let builder = QuicTransportConfig::builder();
    match mode {
        NetworkMode::Local => builder
            .congestion_controller_factory(Arc::new(noq_proto::congestion::CubicConfig::default())),
        NetworkMode::Internet | NetworkMode::Relay | NetworkMode::RelayOnly => builder
            .congestion_controller_factory(Arc::new(noq_proto::congestion::Bbr3Config::default())),
    }
    .build()
}

impl NodeEngine {
    /// Wraps an endpoint configured by the caller (e.g. tests that control the
    /// network paths). It must accept [`ALPN`]; `mode` is only reported by
    /// [`NodeEngine::mode`].
    pub fn from_endpoint(endpoint: Endpoint, mode: NetworkMode) -> Self {
        Self {
            endpoint,
            network: mode.into(),
        }
    }

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
        Self::with_network(secret_key, mode.into()).await
    }

    /// Binds a new endpoint with the given identity and [`Network`] (with the
    /// relays of [`NetworkMode::Relay`]), on IPv4 port [`QUIC_PORT`] when it
    /// is free, or on a random port with [`Network::random_port`].
    ///
    /// # Errors
    ///
    /// Fails when the endpoint cannot bind its UDP sockets, or in
    /// [`NetworkMode::Relay`] or [`NetworkMode::RelayOnly`] without relays.
    pub async fn with_network(secret_key: SecretKey, network: Network) -> Result<Self> {
        // Port 0 lets the system pick a free port.
        let ports = if network.random_port {
            0..=0
        } else {
            QUIC_PORT..=QUIC_PORT
        };
        Self::bind_in(secret_key, network, ports).await
    }

    /// A node with a **new, random identity** for sharing one file by TICKET.
    ///
    /// Whoever gets the TICKET learns this Node ID instead of the device's
    /// permanent one, so shares cannot be linked to each other or to the
    /// device, and the TICKET cannot be used to contact the real node later. It
    /// listens in [`SHARE_PORTS`], next to the main port, so one firewall rule
    /// covers both.
    ///
    /// # Errors
    ///
    /// Fails when the endpoint cannot bind its UDP sockets.
    pub async fn one_time(network: impl Into<Network>) -> Result<Self> {
        Self::bind_in(SecretKey::generate(), network.into(), SHARE_PORTS).await
    }

    /// Binds on the first free IPv4 port of `ports`, or on a random one when all
    /// are taken.
    async fn bind_in(
        secret_key: SecretKey,
        network: Network,
        ports: std::ops::RangeInclusive<u16>,
    ) -> Result<Self> {
        let mode = network.mode;
        if mode.own_relays() && network.relays.is_empty() {
            anyhow::bail!(t!("engine.no_relay"));
        }
        let builder = |key: SecretKey| {
            let builder = match mode {
                NetworkMode::Local => {
                    Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
                }
                NetworkMode::Internet => Endpoint::builder(presets::N0),
                // No address lookup service: nothing goes to n0.
                NetworkMode::Relay => Endpoint::builder(presets::Minimal)
                    .relay_mode(RelayMode::custom(network.relays.iter().cloned())),
                // No IP transport: no UDP socket, so no direct path and no
                // address of this device in its EndpointAddr.
                NetworkMode::RelayOnly => Endpoint::builder(presets::Minimal)
                    .relay_mode(RelayMode::custom(network.relays.iter().cloned()))
                    .clear_ip_transports(),
            };
            builder
                .secret_key(key)
                .alpns(version::accept_alpns())
                .transport_config(transport_config(mode))
        };
        let mut preferred = None;
        // Without IP transports there is no port to choose.
        for port in ports.clone().filter(|_| mode.direct()) {
            let bound = match builder(secret_key.clone())
                .bind_addr(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
            {
                Ok(b) => b.bind().await.ok(),
                Err(_) => None,
            };
            if bound.is_some() {
                preferred = bound;
                break;
            }
        }
        let endpoint = match preferred {
            Some(endpoint) => endpoint,
            None => {
                if mode.direct() {
                    debug!(
                        "UDP {}-{} is taken, binding QUIC to a random port",
                        ports.start(),
                        ports.end()
                    );
                }
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
        Ok(Self { endpoint, network })
    }

    /// A node using the persistent identity and the [`NetworkMode`] saved in
    /// the configuration directory.
    ///
    /// # Errors
    ///
    /// Fails when the key cannot be loaded or the endpoint cannot bind.
    pub async fn with_persistent_identity() -> Result<Self> {
        let config = crate::config::Config::load().unwrap_or_default();
        let network = Network::from_config(&config)?;
        Self::with_network(super::load_or_generate_default_key()?, network).await
    }

    /// The network mode this node was started with.
    pub fn mode(&self) -> NetworkMode {
        self.network.mode
    }

    /// The network (mode and relays) this node was started with.
    pub fn network(&self) -> &Network {
        &self.network
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
        if self.mode().uses_relays() {
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
        info!(
            "Incoming connection from {} ({})",
            conn.remote_id(),
            version::negotiated(&conn).map_or_else(|| "?".into(), |v| v.to_string())
        );
        Ok(Some(conn))
    }

    /// Connects to `target` using the Ferry [`ALPN`].
    ///
    /// # Errors
    ///
    /// Fails when the node cannot be reached at any of its known addresses (and,
    /// in [`NetworkMode::Internet`], not through a relay either).
    pub async fn connect(&self, target: impl Into<EndpointAddr>) -> Result<Connection> {
        let target = target.into();
        debug!("Connecting to {}", target.id);
        // The newest version is the main one, older ones are offered as well: the
        // other side picks the best it knows, so it may be older than this device.
        let (main, older) = version::offer_alpns();
        let connecting = self
            .endpoint
            .connect_with_opts(
                target,
                &main,
                ConnectOptions::new().with_additional_alpns(older),
            )
            .await
            .map_err(|e| connect_error(anyhow::Error::new(e)))?;
        let conn = connecting
            .await
            .map_err(|e| connect_error(anyhow::Error::new(e)))?;
        info!(
            "Connected to {} ({})",
            conn.remote_id(),
            version::negotiated(&conn).map_or_else(|| "?".into(), |v| v.to_string())
        );
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
        assert_eq!("relay".parse::<NetworkMode>().unwrap(), NetworkMode::Relay);
        assert_eq!(
            "Relay-Only".parse::<NetworkMode>().unwrap(),
            NetworkMode::RelayOnly
        );
        // The code is what the configuration file stores.
        for mode in NetworkMode::ALL {
            assert_eq!(mode.code().parse::<NetworkMode>().unwrap(), mode);
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{}\"", mode.code())
            );
        }
        assert!("n0".parse::<NetworkMode>().is_err());
        assert_eq!(NetworkMode::default(), NetworkMode::Local);
    }

    #[test]
    fn relay_urls_must_be_https_or_local() {
        for good in [
            "https://relay.example.org",
            "https://relay.example.org:8443/",
            "http://127.0.0.1:3340",
            "http://192.168.1.5:3340",
            "http://[::1]:3340",
        ] {
            assert!(parse_relay_url(good).is_ok(), "{good}");
        }
        for bad in [
            "http://relay.example.org",
            "http://8.8.8.8:3340",
            "relay.example.org",
            "ftp://relay.example.org",
            "",
        ] {
            assert!(parse_relay_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_relay_mode_needs_a_relay() {
        let mut config = Config {
            network_mode: NetworkMode::Relay,
            ..Config::default()
        };
        assert!(Network::from_config(&config).is_err());
        config.relay_urls = vec!["https://relay.example.org".into()];
        let network = Network::from_config(&config).unwrap();
        assert_eq!(network.relays.len(), 1);
        config.relay_urls.push("http://relay.example.org".into());
        assert!(
            Network::from_config(&config).is_err(),
            "every URL is checked"
        );
        // Other modes ignore the list.
        config.network_mode = NetworkMode::Internet;
        assert!(Network::from_config(&config).unwrap().relays.is_empty());
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
