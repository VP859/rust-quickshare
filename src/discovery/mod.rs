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
//!
//! The parts: [`Discovery`] and the device list here, the wire format in
//! `packet`, the running exchange in `exchange` and sockets and networks in
//! `network`.

mod exchange;
mod network;
mod packet;

pub(crate) use network::{bind, blocking_firewall_for, local_networks};
#[cfg(feature = "fuzzing")]
pub use packet::fuzz_announcement;
pub use packet::{Announcement, MAX_BEACON_AGE, sign_announcement, verify_announcement};

use crate::config::Config;
use crate::node::NodeEngine;
use crate::pairing::{DiscoveryKey, clean_device_name};
use anyhow::{Context, Result};
use exchange::{ANNOUNCE_INTERVAL, announce, expire, handle_packet};
use iroh::{EndpointAddr, EndpointId};
use network::{join_multicast, spawn_firewall_check};
use packet::{MAX_BEACON_LEN, SEAL_OVERHEAD};
use rust_i18n::t;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

/// UDP port of discovery packets; must be allowed by firewalls on the LAN.
pub const DISCOVERY_PORT: u16 = 47823;

/// Administratively scoped multicast group used for announcements (not mDNS).
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 71, 83);

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
    /// Secrets shared with paired devices at pairing. A hidden device encrypts
    /// its answers to a device that has one (see `docs/PROTOCOL.md`) and tries them all
    /// on a packet it cannot read.
    pub keys: HashMap<EndpointId, DiscoveryKey>,
    /// Put the operating system in announcements that everyone can read.
    pub announce_platform: bool,
}

impl Visibility {
    /// Visibility stored in the configuration (`hidden` and the paired devices).
    pub fn from_config(config: &Config) -> Self {
        Self {
            hidden: config.hidden,
            trusted: config.trusted_ids().collect(),
            keys: config.discovery_keys(),
            announce_platform: config.announce_platform,
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

/// Background task: announces periodically and on demand, answers new
/// devices and forgets devices that stopped announcing.
async fn run(engine: NodeEngine, socket: UdpSocket, opts: DiscoveryOptions, shared: Arc<Shared>) {
    let mut buf = vec![0u8; MAX_BEACON_LEN + SEAL_OVERHEAD];
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
    fn hidden_devices_answer_only_trusted_ones() {
        let friend = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let visible = Visibility::default();
        assert!(visible.answers(&stranger));
        let hidden = Visibility {
            hidden: true,
            trusted: [friend].into(),
            ..Visibility::default()
        };
        assert!(hidden.answers(&friend));
        assert!(!hidden.answers(&stranger));
    }
}
