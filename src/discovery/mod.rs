//! Automatic discovery of Ferry devices on the local network.
//!
//! No mDNS or Bluetooth is used. Every node announces itself every few seconds
//! with a small UDP packet (`FRYd`, port [`DISCOVERY_PORT`]) sent to the
//! broadcast address of each of its networks and to the multicast group
//! [`MULTICAST_GROUP`] (TTL 1, also over loopback, so several instances on one
//! computer see each other even behind a VPN that blocks the LAN). The first
//! packet from a new device is answered directly with unicast, so it is enough
//! for announcements to get through in one direction.
//!
//! An announcement carries the device name and its iroh address and is signed
//! with the device's Ed25519 key (the one behind its Node ID) together with a
//! timestamp. Nobody can therefore announce a false address for someone else's
//! Node ID, and recorded announcements are rejected after
//! [`MAX_BEACON_AGE`]. The *name* is still chosen freely by the sender, so it
//! is not a proof of identity: connections authenticate the Node ID with TLS
//! and pairing requires consent and a matching verification code. When the
//! local network is unavailable (e.g. a VPN with LAN blocking), TICKETs and QR
//! codes keep working.
//!
//! A *hidden* device ([`Visibility::hidden`]) does not announce itself; it
//! only answers announcements of its paired devices, which can verify the
//! answer by its signature. Everyone else on the network does not see it.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::node::NodeEngine;
use crate::pairing::clean_device_name;

/// UDP port of discovery packets; must be allowed by firewalls on the LAN.
pub const DISCOVERY_PORT: u16 = 47823;

/// Administratively scoped multicast group used for announcements (not mDNS).
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 71, 83);

/// First bytes of every announcement; other packets on the port are ignored.
const MAGIC: [u8; 4] = *b"FRYd";
/// Interval between announcements.
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
/// A device disappears from the list after about three missed announcements.
const DEVICE_TTL: Duration = Duration::from_secs(17);
/// Upper bound of remembered devices, so a flood of fake announcements cannot exhaust memory.
const MAX_DEVICES: usize = 256;
/// Largest accepted announcement in bytes.
const MAX_BEACON_LEN: usize = 2048;
/// Largest number of QUIC ports taken from one announcement.
const MAX_QUIC_PORTS: usize = 4;
/// Announcements whose timestamp differs from the local clock by more than
/// this are ignored, so recorded packets cannot be replayed later. Generous
/// enough for phones and computers whose clocks are a few minutes off.
pub const MAX_BEACON_AGE: Duration = Duration::from_secs(300);
/// Domain separation of the announcement signature.
const SIGNATURE_CONTEXT: &[u8] = b"ferry discovery beacon v1";

/// Datagram on the discovery port: a signed, serialized [`Announcement`].
#[derive(Serialize, Deserialize)]
struct SignedBeacon {
    /// Always [`MAGIC`].
    magic: [u8; 4],
    /// The postcard-encoded [`Announcement`].
    body: Vec<u8>,
    /// Ed25519 signature of [`SIGNATURE_CONTEXT`] followed by `body`, made
    /// with the key of `Announcement::addr.id`.
    signature: iroh::Signature,
}

/// Content of a discovery announcement, as sent to the local network and
/// answered directly. Signed and decoded with [`sign_announcement`] and
/// [`verify_announcement`]; `docs/PROTOCOL.md` specifies the encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    /// Seconds since the Unix epoch when the announcement was made.
    pub timestamp: u64,
    /// Device name of the sender.
    pub name: String,
    /// Operating system of the sender (`std::env::consts::OS`).
    pub platform: String,
    /// iroh address of the sender (Node ID and direct addresses).
    pub addr: EndpointAddr,
    /// IPv4 QUIC ports; combined with the packet's source address they give a
    /// LAN address even before iroh has discovered its own addresses.
    pub quic_ports: Vec<u16>,
    /// Unicast answer to someone else's announcement (answers are not answered).
    pub reply: bool,
}

/// A device visible on the local network.
#[derive(Debug, Clone, Serialize)]
pub struct NearbyDevice {
    /// Node ID as text.
    pub node_id: String,
    /// Announced device name.
    pub name: String,
    /// Operating system (`linux`, `windows`, `android`, …).
    pub platform: String,
    /// Node ID.
    #[serde(skip)]
    pub id: EndpointId,
    /// Current address including the LAN address the announcement came from.
    #[serde(skip)]
    pub addr: EndpointAddr,
}

/// Who may see this device on the local network.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visibility {
    /// Do not announce this device; answer only devices in `trusted`.
    pub hidden: bool,
    /// Paired devices, which see this device even when it is hidden.
    pub trusted: HashSet<EndpointId>,
}

impl Visibility {
    /// Visibility stored in the configuration (`hidden` and the paired devices).
    pub fn from_config(config: &Config) -> Self {
        Self {
            hidden: config.hidden,
            trusted: config.trusted_ids().collect(),
        }
    }

    /// Whether this device may answer an announcement of `peer`.
    fn answers(&self, peer: &EndpointId) -> bool {
        !self.hidden || self.trusted.contains(peer)
    }
}

/// Configuration of a [`Discovery`] service.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Port to listen on and to send announcements to.
    pub port: u16,
    /// Fixed announcement targets instead of broadcast/multicast (tests,
    /// networks without broadcast).
    pub targets: Option<Vec<SocketAddr>>,
    /// Initial visibility; change it later with [`Discovery::set_visibility`].
    pub visibility: Visibility,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            port: DISCOVERY_PORT,
            targets: None,
            visibility: Visibility::default(),
        }
    }
}

impl DiscoveryOptions {
    /// Default options with the visibility stored in `config`.
    pub fn from_config(config: &Config) -> Self {
        Self {
            visibility: Visibility::from_config(config),
            ..Self::default()
        }
    }
}

/// A visible device and when it was last heard.
struct Entry {
    /// Last announcement of the device.
    device: NearbyDevice,
    /// When it was received.
    seen: Instant,
}

/// State of the discovery service, used to explain an empty device list.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiscoveryHealth {
    /// The socket listens on [`DISCOVERY_PORT`]; otherwise announcements are not received.
    pub listening: bool,
    /// This device does not announce itself ([`Visibility::hidden`]).
    pub hidden: bool,
    /// The first announcement round has completed; before that the state is unknown.
    pub announced: bool,
    /// Number of local IPv4 networks (Wi-Fi/LAN) in the last round.
    pub networks: usize,
    /// At least one announcement reached a local network in the last round.
    pub lan_send_ok: bool,
    /// Last error of sending to a local network (e.g. EPERM caused by a VPN or firewall).
    pub lan_send_error: Option<String>,
    /// Number of announcements received from other devices.
    pub beacons_received: u64,
    /// Announcements from other computers on the network, not counting other
    /// instances on this computer.
    pub lan_beacons_received: u64,
    /// A firewall known to block incoming discovery packets (e.g. `firewalld`).
    pub firewall_blocking: Option<String>,
}

impl DiscoveryHealth {
    /// A hint for the user when discovery cannot fully work, in the active language.
    pub fn hint(&self) -> Option<String> {
        if !self.listening {
            return Some(t!("discovery.port_busy", port = DISCOVERY_PORT).into_owned());
        }
        if self.hidden {
            return Some(t!("discovery.hidden").into_owned());
        }
        if !self.announced {
            return None;
        }
        if self.networks == 0 {
            return Some(t!("discovery.no_network").into_owned());
        }
        let error = || {
            self.lan_send_error
                .clone()
                .unwrap_or_else(|| t!("discovery.unknown_error").into_owned())
        };
        if !self.lan_send_ok && cfg!(target_os = "android") {
            return Some(t!("discovery.android_blocked", error = error()).into_owned());
        }
        if !self.lan_send_ok {
            return Some(t!("discovery.lan_blocked", error = error()).into_owned());
        }
        if self.lan_beacons_received == 0 {
            if self.firewall_blocking.as_deref() == Some("firewalld") {
                return Some(
                    t!(
                        "discovery.firewalld",
                        ports = format!("{DISCOVERY_PORT}-{}", crate::node::SHARE_PORTS.end())
                    )
                    .into_owned(),
                );
            }
            return Some(t!("discovery.nothing_received", port = DISCOVERY_PORT).into_owned());
        }
        None
    }
}

/// State shared by the [`Discovery`] handle and its background task.
struct Shared {
    /// Announced device name.
    name: RwLock<String>,
    /// Visible devices keyed by Node ID.
    devices: Mutex<HashMap<EndpointId, Entry>>,
    /// Wakes the task for an immediate announcement.
    probe: Notify,
    /// Publishes the sorted device list to subscribers.
    tx: watch::Sender<Vec<NearbyDevice>>,
    /// Current state, see [`DiscoveryHealth`].
    health: Mutex<DiscoveryHealth>,
    /// IPv4 addresses of this computer from the last announcement round, used
    /// to tell announcements of other local instances from the network.
    own_addrs: Mutex<Vec<Ipv4Addr>>,
    /// Who may see this device.
    visibility: RwLock<Visibility>,
}

impl Shared {
    /// Sends the current device list, sorted by name, to subscribers.
    fn publish(&self, devices: &HashMap<EndpointId, Entry>) {
        let mut list: Vec<_> = devices.values().map(|e| e.device.clone()).collect();
        list.sort_by_key(|d| d.name.to_lowercase());
        self.tx.send_replace(list);
    }
}

/// The discovery service; stops when dropped.
pub struct Discovery {
    /// State shared with the background task.
    shared: Arc<Shared>,
    /// The background task; aborted on drop.
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Discovery {
    /// Starts announcing and listening. Must be called within a Tokio runtime.
    ///
    /// When the port is taken by a program that does not share it, discovery
    /// runs in a limited mode: it still announces and receives direct answers.
    ///
    /// # Errors
    ///
    /// Fails when no UDP socket can be opened at all.
    pub fn start(engine: NodeEngine, name: &str, opts: DiscoveryOptions) -> Result<Self> {
        let mut listening = true;
        let std_socket = bind(opts.port)
            .or_else(|e| {
                listening = false;
                warn!(
                    "Discovery port {} unavailable ({e}), running in limited mode",
                    opts.port
                );
                bind(0)
            })
            .with_context(|| t!("discovery.socket"))?;
        let socket = UdpSocket::from_std(std_socket)?;
        let (tx, _) = watch::channel(Vec::new());
        let shared = Arc::new(Shared {
            name: RwLock::new(clean_device_name(name)),
            devices: Mutex::new(HashMap::new()),
            probe: Notify::new(),
            tx,
            health: Mutex::new(DiscoveryHealth {
                listening,
                hidden: opts.visibility.hidden,
                ..Default::default()
            }),
            own_addrs: Mutex::new(Vec::new()),
            visibility: RwLock::new(opts.visibility.clone()),
        });
        info!(
            "Discovery listening on UDP {} ({})",
            opts.port,
            if listening { "ok" } else { "limited mode" }
        );
        if opts.targets.is_none() {
            spawn_firewall_check(shared.clone(), opts.port);
        }
        let task = tokio::spawn(run(engine, socket, opts, shared.clone()));
        Ok(Self { shared, task })
    }

    /// Current state of the service.
    pub fn health(&self) -> DiscoveryHealth {
        self.shared
            .health
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default()
    }

    /// Devices currently visible, sorted by name.
    pub fn devices(&self) -> Vec<NearbyDevice> {
        self.shared.tx.borrow().clone()
    }

    /// Receiver notified whenever the device list changes.
    pub fn subscribe(&self) -> watch::Receiver<Vec<NearbyDevice>> {
        self.shared.tx.subscribe()
    }

    /// Announces this device immediately (e.g. the "Search" button) and checks
    /// the firewall again.
    pub fn probe(&self) {
        self.shared.probe.notify_one();
        spawn_firewall_check(self.shared.clone(), DISCOVERY_PORT);
    }

    /// Changes the announced device name and announces it immediately.
    pub fn set_name(&self, name: &str) {
        if let Ok(mut current) = self.shared.name.write() {
            *current = clean_device_name(name);
        }
        self.probe();
    }

    /// Changes who may see this device. Becoming visible announces it at once;
    /// devices that saw it before keep it in their lists until it expires there.
    pub fn set_visibility(&self, visibility: Visibility) {
        let hidden = visibility.hidden;
        if let Ok(mut current) = self.shared.visibility.write() {
            *current = visibility;
        }
        if let Ok(mut health) = self.shared.health.lock() {
            health.hidden = hidden;
        }
        if !hidden {
            self.shared.probe.notify_one();
        }
    }

    /// Finds a visible device; see [`find_in`].
    pub fn find(&self, query: &str) -> Option<NearbyDevice> {
        find_in(&self.devices(), query)
    }

    /// Current address of a device if it is visible on the local network.
    pub fn addr_of(&self, id: &EndpointId) -> Option<EndpointAddr> {
        self.devices()
            .into_iter()
            .find(|d| &d.id == id)
            .map(|d| d.addr)
    }

    /// Waits at most `timeout` for a device matching `query` (see [`find_in`]).
    pub async fn wait_for(&self, query: &str, timeout: Duration) -> Option<NearbyDevice> {
        let mut rx = self.subscribe();
        self.probe();
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(d) = find_in(&rx.borrow_and_update(), query) {
                    return d;
                }
                if rx.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        })
        .await
        .ok()
    }
}

/// Address of `target`: an alias, nearby device name, Node ID or TICKET.
///
/// A fresh LAN address announced on the local network is preferred; otherwise
/// the address stored in `config` or carried by the TICKET is used. A bare
/// Node ID of a device that is not visible on the network cannot be reached.
///
/// # Errors
///
/// Fails when `target` is neither a trusted or nearby device nor a valid
/// TICKET or Node ID.
pub fn resolve_target(
    config: &Config,
    discovery: Option<&Discovery>,
    target: &str,
) -> Result<EndpointAddr> {
    let resolved = config.resolve_target(target);
    let nearby = discovery.and_then(|d| match &resolved {
        Ok(addr) => d.addr_of(&addr.id),
        Err(_) => d.find(target).map(|n| n.addr),
    });
    match (nearby, resolved) {
        (Some(addr), _) | (None, Ok(addr)) => Ok(addr),
        (None, Err(e)) => Err(e),
    }
}

/// Finds a device by full Node ID, a Node ID prefix of at least 8 characters,
/// or its name (case-insensitive).
pub fn find_in(devices: &[NearbyDevice], query: &str) -> Option<NearbyDevice> {
    let q = query.trim();
    devices
        .iter()
        .find(|d| {
            d.node_id == q
                || (q.len() >= 8 && d.node_id.starts_with(q))
                || d.name.eq_ignore_ascii_case(q)
        })
        .cloned()
}

/// Checks the firewall on a separate thread (it runs external programs) and
/// stores the result in [`DiscoveryHealth::firewall_blocking`].
fn spawn_firewall_check(shared: Arc<Shared>, port: u16) {
    std::thread::spawn(move || {
        let blocking = blocking_firewall(port);
        if let Some(name) = &blocking {
            warn!("Discovery: {name} blocks incoming UDP {port}");
        }
        if let Ok(mut health) = shared.health.lock() {
            health.firewall_blocking = blocking;
        }
    });
}

/// Name of an active firewall that does not allow incoming UDP on `port`.
fn blocking_firewall(port: u16) -> Option<String> {
    blocking_firewall_for(port, "udp")
}

/// Name of an active firewall that does not allow incoming `protocol`
/// (`udp`, `tcp`) on `port`. Runs external programs: call it off the async
/// threads.
///
/// Only firewalld (default on Fedora and openSUSE) can be queried without root:
/// `firewall-cmd --query-port` also takes port ranges into account.
#[cfg(target_os = "linux")]
pub(crate) fn blocking_firewall_for(port: u16, protocol: &str) -> Option<String> {
    use std::process::{Command, Stdio};
    let running = Command::new("firewall-cmd")
        .arg("--state")
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout.starts_with(b"running"));
    if !running {
        return None;
    }
    let open = Command::new("firewall-cmd")
        .arg(format!("--query-port={port}/{protocol}"))
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    (!open).then(|| "firewalld".to_string())
}

/// Other platforms: no firewall that can be queried without administrator rights.
#[cfg(not(target_os = "linux"))]
pub(crate) fn blocking_firewall_for(_port: u16, _protocol: &str) -> Option<String> {
    None
}

/// Opens the discovery socket: shared port (several apps on one computer),
/// broadcast allowed, non-blocking. Port `0` picks a random port.
pub(crate) fn bind(port: u16) -> std::io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    Ok(socket.into())
}

/// Background task: announces periodically and on demand, answers new
/// devices and forgets devices that stopped announcing.
async fn run(engine: NodeEngine, socket: UdpSocket, opts: DiscoveryOptions, shared: Arc<Shared>) {
    let mut buf = vec![0u8; MAX_BEACON_LEN];
    let mut tick = tokio::time::interval(ANNOUNCE_INTERVAL);
    let mut joined = BTreeSet::new();
    if opts.targets.is_none() {
        let _ = socket.set_multicast_loop_v4(true);
        let _ = socket.set_multicast_ttl_v4(1);
    }
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if opts.targets.is_none() {
                    join_multicast(&engine, &socket, &mut joined);
                }
                announce(&engine, &socket, &opts, &shared).await;
                expire(&shared);
            }
            _ = shared.probe.notified() => announce(&engine, &socket, &opts, &shared).await,
            res = socket.recv_from(&mut buf) => match res {
                Ok((len, from)) => handle_packet(&engine, &socket, &shared, &buf[..len], from).await,
                Err(e) => {
                    debug!("Discovery receive error: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// Current time in seconds since the Unix epoch (0 if the clock is before it).
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Serializes and signs the current announcement; `None` if it would not fit
/// in one datagram.
fn encode_beacon(engine: &NodeEngine, shared: &Shared, reply: bool) -> Option<Vec<u8>> {
    let endpoint = engine.endpoint();
    let beacon = Announcement {
        timestamp: unix_now(),
        name: shared.name.read().map(|n| n.clone()).unwrap_or_default(),
        platform: std::env::consts::OS.to_string(),
        addr: endpoint.addr(),
        quic_ports: endpoint
            .bound_sockets()
            .iter()
            .filter(|a| a.is_ipv4())
            .map(|a| a.port())
            .take(MAX_QUIC_PORTS)
            .collect(),
        reply,
    };
    sign_announcement(endpoint.secret_key(), &beacon)
}

/// Serializes `announcement` and signs it with `key`, which must be the key of
/// `announcement.addr.id`: the datagram that goes on the wire. `None` if it would
/// not fit in one datagram.
pub fn sign_announcement(key: &iroh::SecretKey, announcement: &Announcement) -> Option<Vec<u8>> {
    let body = postcard::to_allocvec(announcement).ok()?;
    let signature = key.sign(&[SIGNATURE_CONTEXT, &body].concat());
    postcard::to_allocvec(&SignedBeacon {
        magic: MAGIC,
        body,
        signature,
    })
    .ok()
    .filter(|b| b.len() <= MAX_BEACON_LEN)
}

/// Decodes a datagram and checks its signature and age; `None` for anything
/// that is not a valid announcement made within [`MAX_BEACON_AGE`] of `now`
/// (seconds since the Unix epoch). The signature must be by the key of the
/// announced Node ID.
pub fn verify_announcement(data: &[u8], now: u64) -> Option<Announcement> {
    decode_beacon(data, now)
}

/// The decoding and checking behind [`verify_announcement`].
fn decode_beacon(data: &[u8], now: u64) -> Option<Announcement> {
    let signed = postcard::from_bytes::<SignedBeacon>(data).ok()?;
    if signed.magic != MAGIC {
        return None;
    }
    let beacon = postcard::from_bytes::<Announcement>(&signed.body).ok()?;
    beacon
        .addr
        .id
        .verify(
            &[SIGNATURE_CONTEXT, &signed.body].concat(),
            &signed.signature,
        )
        .ok()?;
    (beacon.timestamp.abs_diff(now) <= MAX_BEACON_AGE.as_secs()).then_some(beacon)
}

/// The device described by a valid announcement received from `from`: the
/// name is cleaned (or replaced by a short Node ID), the platform reduced to a
/// short alphanumeric word and the QUIC ports combined with the sender's IP.
/// Everything in it comes from the network, so nothing is trusted as is.
fn nearby_device(beacon: Announcement, from: SocketAddr) -> NearbyDevice {
    let id = beacon.addr.id;
    let mut addr = beacon.addr;
    for port in beacon.quic_ports.iter().take(MAX_QUIC_PORTS) {
        addr.addrs
            .insert(TransportAddr::Ip(SocketAddr::new(from.ip(), *port)));
    }
    let name = match clean_device_name(&beacon.name) {
        n if n.is_empty() => t!("config.fallback_alias", id = id.fmt_short()).into_owned(),
        n => n,
    };
    let platform: String = beacon
        .platform
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    NearbyDevice {
        node_id: id.to_string(),
        name,
        platform,
        id,
        addr,
    }
}

/// Entry point for the `announcement` fuzz target (feature `fuzzing`): decodes
/// `data` as a datagram, and as the body of an announcement an attacker signed
/// with their own key, and checks what would reach the device list.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_announcement(data: &[u8]) {
    let _ = decode_beacon(data, unix_now());
    if let Ok(beacon) = postcard::from_bytes::<Announcement>(data) {
        let listed = beacon.addr.addrs.len();
        let device = nearby_device(beacon, SocketAddr::from(([192, 168, 1, 2], DISCOVERY_PORT)));
        assert!(!device.name.is_empty());
        assert!(device.name.chars().all(|c| !c.is_control()));
        assert!(device.platform.len() <= 16);
        assert!(device.addr.addrs.len() <= listed + MAX_QUIC_PORTS);
    }
}

/// Sends one announcement round and updates [`DiscoveryHealth`].
///
/// Only sends to local networks count towards `lan_send_ok`; loopback and
/// `255.255.255.255` (usually routed into a VPN tunnel) are ignored.
async fn announce(
    engine: &NodeEngine,
    socket: &UdpSocket,
    opts: &DiscoveryOptions,
    shared: &Shared,
) {
    if shared.visibility.read().is_ok_and(|v| v.hidden) {
        return;
    }
    let Some(bytes) = encode_beacon(engine, shared, false) else {
        return;
    };
    let mut lan = LanSendStats::default();
    let networks = match &opts.targets {
        Some(targets) => {
            for target in targets {
                lan.record(send(socket, &bytes, *target).await);
            }
            1
        }
        None => {
            let networks = local_networks(Some(engine));
            if let Ok(mut own) = shared.own_addrs.lock() {
                *own = networks.iter().map(|(ip, _)| *ip).collect();
            }
            send(
                socket,
                &bytes,
                SocketAddrV4::new(Ipv4Addr::BROADCAST, opts.port).into(),
            )
            .await
            .ok();
            for ip in networks
                .iter()
                .filter_map(|(_, b)| *b)
                .collect::<BTreeSet<_>>()
            {
                lan.record(send(socket, &bytes, SocketAddrV4::new(ip, opts.port).into()).await);
            }

            let group: SocketAddr = SocketAddrV4::new(MULTICAST_GROUP, opts.port).into();
            if socket2::SockRef::from(socket)
                .set_multicast_if_v4(&Ipv4Addr::LOCALHOST)
                .is_ok()
            {
                send(socket, &bytes, group).await.ok();
            }
            for (iface, _) in &networks {
                if socket2::SockRef::from(socket)
                    .set_multicast_if_v4(iface)
                    .is_ok()
                {
                    lan.record(send(socket, &bytes, group).await);
                }
            }
            networks.len()
        }
    };

    if let Ok(mut health) = shared.health.lock() {
        let error = if lan.ok { None } else { lan.error };
        if health.networks != networks
            || health.lan_send_ok != lan.ok
            || health.lan_send_error != error
        {
            if lan.ok {
                info!("Discovery: announcing on {networks} local network(s)");
            } else {
                warn!(
                    "Discovery: cannot send to the local network (networks: {networks}, error: {})",
                    error.as_deref().unwrap_or("no interface")
                );
            }
        }
        health.announced = true;
        health.networks = networks;
        health.lan_send_ok = lan.ok;
        health.lan_send_error = error;
    }
}

/// Outcome of sending one announcement round to local networks.
#[derive(Default)]
struct LanSendStats {
    /// At least one send succeeded.
    ok: bool,
    /// Last send error.
    error: Option<String>,
}

impl LanSendStats {
    /// Records the result of one send.
    fn record(&mut self, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.ok = true,
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}

/// Sends one datagram, logging failures at debug level.
async fn send(socket: &UdpSocket, bytes: &[u8], target: SocketAddr) -> std::io::Result<()> {
    socket
        .send_to(bytes, target)
        .await
        .map(|_| ())
        .inspect_err(|e| {
            debug!("Could not send an announcement to {target}: {e}");
        })
}

/// Joins the multicast group on loopback and every local interface not joined yet.
///
/// `AddrInUse` means the default interface was already joined through `0.0.0.0`.
fn join_multicast(engine: &NodeEngine, socket: &UdpSocket, joined: &mut BTreeSet<Ipv4Addr>) {
    let interfaces = [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOCALHOST]
        .into_iter()
        .chain(local_networks(Some(engine)).into_iter().map(|(ip, _)| ip));
    for iface in interfaces {
        if !joined.contains(&iface) {
            match socket.join_multicast_v4(MULTICAST_GROUP, iface) {
                Ok(()) => {
                    joined.insert(iface);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    joined.insert(iface);
                }
                Err(e) => debug!("Could not join the multicast group on {iface}: {e}"),
            }
        }
    }
}

/// Processes a received announcement: updates the device list and health
/// and answers a new device directly (answers are not answered again).
async fn handle_packet(
    engine: &NodeEngine,
    socket: &UdpSocket,
    shared: &Shared,
    data: &[u8],
    from: SocketAddr,
) {
    let Some(beacon) = decode_beacon(data, unix_now()) else {
        return;
    };
    if beacon.addr.id == engine.node_id() {
        return;
    }
    let from_this_computer = match from.ip() {
        std::net::IpAddr::V4(ip) => {
            ip.is_loopback() || shared.own_addrs.lock().is_ok_and(|own| own.contains(&ip))
        }
        std::net::IpAddr::V6(ip) => ip.is_loopback(),
    };
    if let Ok(mut health) = shared.health.lock() {
        health.beacons_received += 1;
        if !from_this_computer {
            health.lan_beacons_received += 1;
        }
    }

    let id = beacon.addr.id;
    let reply = beacon.reply;
    let device = nearby_device(beacon, from);

    let changed = {
        let Ok(mut devices) = shared.devices.lock() else {
            return;
        };
        let changed = match devices.get(&id) {
            Some(e) => {
                e.device.name != device.name
                    || e.device.platform != device.platform
                    || e.device.addr != device.addr
            }
            None if devices.len() >= MAX_DEVICES => return,
            None => {
                info!(
                    "Discovery: new device '{}' ({}) from {from}",
                    device.name, device.platform
                );
                true
            }
        };
        devices.insert(
            id,
            Entry {
                device,
                seen: Instant::now(),
            },
        );
        if changed {
            shared.publish(&devices);
        }
        changed
    };

    if changed
        && !reply
        && shared.visibility.read().is_ok_and(|v| v.answers(&id))
        && let Some(bytes) = encode_beacon(engine, shared, true)
        && let Err(e) = socket.send_to(&bytes, from).await
    {
        debug!("Could not answer {from}: {e}");
    }
}

/// Removes devices that have not announced themselves for [`DEVICE_TTL`].
fn expire(shared: &Shared) {
    let Ok(mut devices) = shared.devices.lock() else {
        return;
    };
    let before = devices.len();
    devices.retain(|_, e| e.seen.elapsed() < DEVICE_TTL);
    if devices.len() != before {
        shared.publish(&devices);
    }
}

/// IPv4 addresses of this device on local networks, with each network's broadcast address.
///
/// `engine` is only used on Android, as a fallback when the system lists no
/// interface.
pub(crate) fn local_networks(engine: Option<&NodeEngine>) -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let mut networks = platform_networks(engine);
    networks.sort();
    networks.dedup();
    networks
}

/// Desktop variant using the `network-interface` crate; the broadcast
/// address is computed from the netmask when the system does not report it.
#[cfg(not(target_os = "android"))]
fn platform_networks(_engine: Option<&NodeEngine>) -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};
    let mut networks = Vec::new();
    for iface in NetworkInterface::show().unwrap_or_default() {
        for addr in iface.addr {
            let Addr::V4(v4) = addr else { continue };
            if v4.ip.is_loopback() {
                continue;
            }
            let broadcast = v4.broadcast.or_else(|| {
                v4.netmask
                    .map(|m| Ipv4Addr::from(u32::from(v4.ip) | !u32::from(m)))
            });
            networks.push((v4.ip, broadcast.filter(|b| *b != v4.ip)));
        }
    }
    networks
}

/// Android variant; falls back to a `/24` guess from iroh's addresses when
/// `getifaddrs` returns nothing.
#[cfg(target_os = "android")]
fn platform_networks(engine: Option<&NodeEngine>) -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let networks = android::interfaces();
    if !networks.is_empty() {
        return networks;
    }
    let Some(engine) = engine else {
        return networks;
    };
    engine
        .endpoint()
        .addr()
        .ip_addrs()
        .filter_map(|addr| match addr {
            SocketAddr::V4(v4) if v4.ip().is_private() => {
                let o = v4.ip().octets();
                Some((*v4.ip(), Some(Ipv4Addr::new(o[0], o[1], o[2], 255))))
            }
            _ => None,
        })
        .collect()
}

/// Interface listing for Android through `getifaddrs`, which bionic allows
/// without extra permissions (without MAC addresses since Android 11).
///
/// Also compiled in Linux tests, where glibc provides the same API.
/// One of the two places that need `unsafe` (C FFI); every block is justified.
#[cfg(any(target_os = "android", all(test, target_os = "linux")))]
#[allow(unsafe_code)]
mod android {
    use std::net::Ipv4Addr;

    /// IPv4 address of every non-loopback interface and its broadcast address;
    /// point-to-point interfaces (VPN tunnels) have no broadcast address.
    pub fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
        let mut networks = Vec::new();
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: on success getifaddrs returns a list that is freed below with freeifaddrs.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return networks;
        }
        let mut current = head;
        while !current.is_null() {
            // SAFETY: `current` points to an element of the list returned by getifaddrs.
            let ifa = unsafe { &*current };
            current = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() {
                continue;
            }
            // SAFETY: ifa_addr is non-null; its family determines the concrete type.
            if i32::from(unsafe { (*ifa.ifa_addr).sa_family }) != libc::AF_INET {
                continue;
            }
            // SAFETY: for AF_INET both the address and the netmask are sockaddr_in.
            let (ip, mask) = unsafe {
                let ip = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let mask = &*(ifa.ifa_netmask as *const libc::sockaddr_in);
                (
                    u32::from_be(ip.sin_addr.s_addr),
                    u32::from_be(mask.sin_addr.s_addr),
                )
            };
            let addr = Ipv4Addr::from(ip);
            if addr.is_loopback() || addr.is_unspecified() {
                continue;
            }
            let has_broadcast = ifa.ifa_flags & libc::IFF_BROADCAST as u32 != 0;
            let broadcast = Ipv4Addr::from(ip | !mask);
            networks.push((
                addr,
                (has_broadcast && broadcast != addr).then_some(broadcast),
            ));
        }
        // SAFETY: frees exactly the list allocated by getifaddrs above.
        unsafe { libc::freeifaddrs(head) };
        networks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> NearbyDevice {
        let id = iroh::SecretKey::generate().public();
        NearbyDevice {
            node_id: id.to_string(),
            name: name.into(),
            platform: "linux".into(),
            id,
            addr: id.into(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn getifaddrs_matches_network_interface() {
        use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};
        let expected: BTreeSet<(Ipv4Addr, Option<Ipv4Addr>)> = NetworkInterface::show()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|i| i.addr)
            .filter_map(|a| match a {
                Addr::V4(v4) if !v4.ip.is_loopback() => {
                    let b = v4.broadcast.or_else(|| {
                        v4.netmask
                            .map(|m| Ipv4Addr::from(u32::from(v4.ip) | !u32::from(m)))
                    });
                    Some((v4.ip, b.filter(|b| *b != v4.ip)))
                }
                _ => None,
            })
            .collect();
        let got: BTreeSet<_> = android::interfaces().into_iter().collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn finds_by_name_id_and_prefix() {
        let list = vec![device("Kate's Laptop"), device("Phone")];
        assert_eq!(
            find_in(&list, "kate's laptop").unwrap().name,
            "Kate's Laptop"
        );
        assert_eq!(find_in(&list, &list[1].node_id).unwrap().name, "Phone");
        assert_eq!(
            find_in(&list, &list[1].node_id[..10]).unwrap().name,
            "Phone"
        );
        assert!(find_in(&list, &list[1].node_id[..4]).is_none());
        assert!(find_in(&list, "missing").is_none());
    }

    #[test]
    fn beacon_fits_in_one_datagram() {
        let id = iroh::SecretKey::generate().public();
        let mut addr = EndpointAddr::from(id);
        for i in 0..8u16 {
            addr.addrs.insert(TransportAddr::Ip(SocketAddr::from((
                [192, 168, 1, i as u8],
                40000 + i,
            ))));
        }
        let beacon = Announcement {
            timestamp: unix_now(),
            name: "x".repeat(64),
            platform: "android".into(),
            addr,
            quic_ports: vec![1, 2, 3, 4],
            reply: false,
        };
        let body = postcard::to_allocvec(&beacon).unwrap();
        let signed = SignedBeacon {
            magic: MAGIC,
            signature: iroh::SecretKey::generate().sign(&body),
            body,
        };
        assert!(postcard::to_allocvec(&signed).unwrap().len() < MAX_BEACON_LEN);
    }

    /// Signs `beacon` with `key` the way [`encode_beacon`] does.
    fn sign(key: &iroh::SecretKey, beacon: &Announcement) -> Vec<u8> {
        let body = postcard::to_allocvec(beacon).unwrap();
        let signature = key.sign(&[SIGNATURE_CONTEXT, &body].concat());
        postcard::to_allocvec(&SignedBeacon {
            magic: MAGIC,
            body,
            signature,
        })
        .unwrap()
    }

    #[test]
    fn beacons_are_signed_and_fresh() {
        let key = iroh::SecretKey::generate();
        let now = 1_800_000_000;
        let beacon = |id: EndpointId, timestamp| Announcement {
            timestamp,
            name: "Laptop".into(),
            platform: "linux".into(),
            addr: id.into(),
            quic_ports: vec![],
            reply: false,
        };
        let valid = sign(&key, &beacon(key.public(), now));
        assert!(decode_beacon(&valid, now).is_some());
        assert!(decode_beacon(&valid, now + MAX_BEACON_AGE.as_secs()).is_some());
        assert!(
            decode_beacon(&valid, now + MAX_BEACON_AGE.as_secs() + 1).is_none(),
            "replayed too late"
        );

        let victim = iroh::SecretKey::generate().public();
        let forged = sign(&key, &beacon(victim, now));
        assert!(
            decode_beacon(&forged, now).is_none(),
            "someone else's Node ID"
        );

        let mut tampered = valid.clone();
        let last = tampered.len() - 70;
        tampered[last] ^= 1;
        assert!(decode_beacon(&tampered, now).is_none(), "modified body");
    }

    #[test]
    fn hidden_devices_answer_only_trusted_ones() {
        let friend = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let visible = Visibility::default();
        assert!(visible.answers(&stranger));
        let hidden = Visibility {
            hidden: true,
            trusted: [friend].into(),
        };
        assert!(hidden.answers(&friend));
        assert!(!hidden.answers(&stranger));
    }
}
