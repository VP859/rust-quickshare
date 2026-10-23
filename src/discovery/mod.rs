//! Automatyczne wykrywanie urządzeń RQS w sieci lokalnej.
//!
//! Nie korzysta z mDNS ani Bluetooth: każdy węzeł co kilka sekund rozgłasza mały
//! pakiet UDP (`RQSd`, port [`DISCOVERY_PORT`]) na adresy broadcast swoich sieci
//! oraz na własną grupę multicast [`MULTICAST_GROUP`] (TTL 1, także przez loopback –
//! kilka instancji na jednym komputerze widzi się nawet przy VPN blokującym LAN).
//! Na pierwszy pakiet od nowego urządzenia odpowiada bezpośrednio (unicast) –
//! wystarczy więc, że ogłoszenie przejdzie w jedną stronę.
//!
//! Pakiet zawiera nazwę urządzenia i adres iroh. Nie jest to dowód tożsamości:
//! połączenie i tak uwierzytelnia Node ID w TLS, a parowanie wymaga zgody
//! drugiej strony i porównania kodu weryfikacyjnego.
//! Gdy sieć lokalna jest niedostępna (np. VPN z blokadą LAN), działa TICKET/QR.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

use crate::node::NodeEngine;
use crate::pairing::clean_device_name;

/// Port UDP pakietów wykrywania (musi być przepuszczany przez zaporę w sieci lokalnej).
pub const DISCOVERY_PORT: u16 = 47823;

/// Grupa multicast z zakresu administracyjnego (lokalnego) – nie jest to mDNS.
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 71, 83);

const MAGIC: [u8; 4] = *b"RQSd";
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
/// Urządzenie znika z listy po ~3 pominiętych ogłoszeniach.
const DEVICE_TTL: Duration = Duration::from_secs(17);
const MAX_DEVICES: usize = 256;
const MAX_BEACON_LEN: usize = 2048;
const MAX_QUIC_PORTS: usize = 4;

#[derive(Serialize, Deserialize)]
struct Beacon {
    magic: [u8; 4],
    name: String,
    platform: String,
    addr: EndpointAddr,
    /// Porty QUIC (IPv4) – w połączeniu z adresem źródłowym pakietu dają adres w LAN,
    /// nawet zanim iroh wykryje własne adresy.
    quic_ports: Vec<u16>,
    /// Odpowiedź unicast na cudzy pakiet (na odpowiedzi się nie odpowiada).
    reply: bool,
}

/// Urządzenie widoczne w sieci lokalnej.
#[derive(Debug, Clone, Serialize)]
pub struct NearbyDevice {
    pub node_id: String,
    pub name: String,
    pub platform: String,
    #[serde(skip)]
    pub id: EndpointId,
    #[serde(skip)]
    pub addr: EndpointAddr,
}

#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Port nasłuchu i docelowy port ogłoszeń.
    pub port: u16,
    /// Stałe adresy ogłoszeń zamiast adresów broadcast (testy, sieci bez broadcastu).
    pub targets: Option<Vec<SocketAddr>>,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            port: DISCOVERY_PORT,
            targets: None,
        }
    }
}

struct Entry {
    device: NearbyDevice,
    seen: Instant,
}

/// Stan wykrywania – pozwala interfejsom wyjaśnić, dlaczego lista jest pusta.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiscoveryHealth {
    /// Gniazdo nasłuchuje na standardowym porcie (inaczej nie odbieramy ogłoszeń).
    pub listening: bool,
    /// Liczba sieci lokalnych IPv4 (Wi-Fi/LAN) w ostatniej rundzie ogłoszeń.
    pub networks: usize,
    /// W ostatniej rundzie przynajmniej jedno ogłoszenie trafiło do sieci lokalnej.
    pub lan_send_ok: bool,
    /// Ostatni błąd wysyłki do sieci lokalnej (np. EPERM przy VPN lub zaporze).
    pub lan_send_error: Option<String>,
    /// Liczba odebranych ogłoszeń od innych urządzeń.
    pub beacons_received: u64,
}

impl DiscoveryHealth {
    /// Wskazówka dla użytkownika, gdy wykrywanie nie może działać w pełni.
    pub fn hint(&self) -> Option<String> {
        if !self.listening {
            return Some(format!(
                "Port UDP {DISCOVERY_PORT} jest zajęty przez inny program – to urządzenie nie odbiera ogłoszeń."
            ));
        }
        if self.networks == 0 {
            return Some("Brak połączenia z siecią lokalną (Wi-Fi/LAN).".into());
        }
        if !self.lan_send_ok && cfg!(target_os = "android") {
            return Some(format!(
                "Android blokuje dostęp do sieci lokalnej ({}). Nadaj aplikacji uprawnienie \
                 „Urządzenia w pobliżu” (Ustawienia → Aplikacje → RQS Quick Share → Uprawnienia) \
                 i sprawdź, czy VPN na telefonie zezwala na sieć lokalną.",
                self.lan_send_error.as_deref().unwrap_or("nieznany błąd")
            ));
        }
        if !self.lan_send_ok {
            return Some(format!(
                "Wysyłanie do sieci lokalnej jest blokowane ({}) – zwykle przez VPN lub zaporę. \
                 NordVPN: nordvpn set lan-discovery on. Możesz też użyć TICKETU.",
                self.lan_send_error.as_deref().unwrap_or("nieznany błąd")
            ));
        }
        if self.beacons_received == 0 {
            return Some(format!(
                "Nie odebrano jeszcze żadnych ogłoszeń. Sprawdź, czy drugie urządzenie jest w tej samej \
                 sieci, a zapory na obu urządzeniach przepuszczają UDP {DISCOVERY_PORT}."
            ));
        }
        None
    }
}

struct Shared {
    name: RwLock<String>,
    devices: Mutex<HashMap<EndpointId, Entry>>,
    probe: Notify,
    tx: watch::Sender<Vec<NearbyDevice>>,
    health: Mutex<DiscoveryHealth>,
}

impl Shared {
    fn publish(&self, devices: &HashMap<EndpointId, Entry>) {
        let mut list: Vec<_> = devices.values().map(|e| e.device.clone()).collect();
        list.sort_by_key(|d| d.name.to_lowercase());
        self.tx.send_replace(list);
    }
}

/// Usługa wykrywania; zatrzymuje się po upuszczeniu.
pub struct Discovery {
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Discovery {
    /// Uruchamia ogłaszanie i nasłuch (wymaga środowiska tokio).
    pub fn start(engine: NodeEngine, name: &str, opts: DiscoveryOptions) -> Result<Self> {
        let mut listening = true;
        let std_socket = bind(opts.port)
            .or_else(|e| {
                listening = false;
                // Port zajęty bez SO_REUSEADDR – nadal możemy ogłaszać i odbierać odpowiedzi.
                warn!(
                    "Port wykrywania {} niedostępny ({e}) – tryb ograniczony",
                    opts.port
                );
                bind(0)
            })
            .context("Nie udało się otworzyć gniazda wykrywania UDP")?;
        let socket = UdpSocket::from_std(std_socket)?;
        let (tx, _) = watch::channel(Vec::new());
        let shared = Arc::new(Shared {
            name: RwLock::new(clean_device_name(name)),
            devices: Mutex::new(HashMap::new()),
            probe: Notify::new(),
            tx,
            health: Mutex::new(DiscoveryHealth {
                listening,
                ..Default::default()
            }),
        });
        info!(
            "Wykrywanie: nasłuch UDP {} ({})",
            opts.port,
            if listening {
                "OK"
            } else {
                "port zajęty – tryb ograniczony"
            }
        );
        let task = tokio::spawn(run(engine, socket, opts, shared.clone()));
        Ok(Self { shared, task })
    }

    pub fn health(&self) -> DiscoveryHealth {
        self.shared
            .health
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default()
    }

    pub fn devices(&self) -> Vec<NearbyDevice> {
        self.shared.tx.borrow().clone()
    }

    /// Zmiany listy urządzeń (np. do odświeżania interfejsu).
    pub fn subscribe(&self) -> watch::Receiver<Vec<NearbyDevice>> {
        self.shared.tx.subscribe()
    }

    /// Natychmiastowe ogłoszenie (przycisk „Szukaj”).
    pub fn probe(&self) {
        self.shared.probe.notify_one();
    }

    pub fn set_name(&self, name: &str) {
        if let Ok(mut current) = self.shared.name.write() {
            *current = clean_device_name(name);
        }
        self.probe();
    }

    pub fn find(&self, query: &str) -> Option<NearbyDevice> {
        find_in(&self.devices(), query)
    }

    /// Aktualny adres urządzenia, jeśli jest widoczne w sieci lokalnej.
    pub fn addr_of(&self, id: &EndpointId) -> Option<EndpointAddr> {
        self.devices()
            .into_iter()
            .find(|d| &d.id == id)
            .map(|d| d.addr)
    }

    /// Czeka (maks. `timeout`) na urządzenie o podanej nazwie lub Node ID.
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

/// Dopasowanie po Node ID (pełnym lub prefiksie ≥ 8 znaków) albo nazwie (bez wielkości liter).
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

fn bind(port: u16) -> std::io::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // Kilka instancji na jednym komputerze (np. GUI + `rqs discover`) może słuchać jednocześnie.
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    Ok(socket.into())
}

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
                    // Interfejsy mogą pojawić się później (Wi-Fi, zmiana sieci).
                    join_multicast(&engine, &socket, &mut joined);
                }
                announce(&engine, &socket, &opts, &shared).await;
                expire(&shared);
            }
            _ = shared.probe.notified() => announce(&engine, &socket, &opts, &shared).await,
            res = socket.recv_from(&mut buf) => match res {
                Ok((len, from)) => handle_packet(&engine, &socket, &shared, &buf[..len], from).await,
                // Np. ICMP „port unreachable” zgłaszany przez Windows po wysyłce – ignorujemy.
                Err(e) => {
                    debug!("Błąd odbioru pakietu wykrywania: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

fn encode_beacon(engine: &NodeEngine, shared: &Shared, reply: bool) -> Option<Vec<u8>> {
    let endpoint = engine.endpoint();
    let beacon = Beacon {
        magic: MAGIC,
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
    postcard::to_allocvec(&beacon)
        .ok()
        .filter(|b| b.len() <= MAX_BEACON_LEN)
}

async fn announce(
    engine: &NodeEngine,
    socket: &UdpSocket,
    opts: &DiscoveryOptions,
    shared: &Shared,
) {
    let Some(bytes) = encode_beacon(engine, shared, false) else {
        return;
    };
    // Wyniki wysyłek do sieci lokalnej (bez loopbacku i 255.255.255.255, który
    // przy VPN trafia zwykle do tunelu) – na ich podstawie powstaje diagnostyka.
    let mut lan = LanSendStats::default();
    let networks = match &opts.targets {
        Some(targets) => {
            for target in targets {
                lan.record(send(socket, &bytes, *target).await);
            }
            1
        }
        None => {
            let networks = local_networks(engine);
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

            // Multicast przez każdy interfejs, w tym loopback (instancje na tym komputerze).
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
                info!("Wykrywanie: ogłoszenia wysyłane do {networks} sieci lokalnych");
            } else {
                warn!(
                    "Wykrywanie: brak wysyłki do sieci lokalnej (sieci: {networks}, błąd: {})",
                    error.as_deref().unwrap_or("brak interfejsu")
                );
            }
        }
        health.networks = networks;
        health.lan_send_ok = lan.ok;
        health.lan_send_error = error;
    }
}

#[derive(Default)]
struct LanSendStats {
    ok: bool,
    error: Option<String>,
}

impl LanSendStats {
    fn record(&mut self, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.ok = true,
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}

async fn send(socket: &UdpSocket, bytes: &[u8], target: SocketAddr) -> std::io::Result<()> {
    socket
        .send_to(bytes, target)
        .await
        .map(|_| ())
        .inspect_err(|e| {
            debug!("Nie wysłano ogłoszenia do {target}: {e}");
        })
}

/// Dołącza do grupy multicast na loopbacku i każdym interfejsie (pomija już dołączone).
fn join_multicast(engine: &NodeEngine, socket: &UdpSocket, joined: &mut BTreeSet<Ipv4Addr>) {
    let interfaces = [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOCALHOST]
        .into_iter()
        .chain(local_networks(engine).into_iter().map(|(ip, _)| ip));
    for iface in interfaces {
        if !joined.contains(&iface) {
            match socket.join_multicast_v4(MULTICAST_GROUP, iface) {
                // AddrInUse: interfejs domyślny został już dołączony przez 0.0.0.0.
                Ok(()) => {
                    joined.insert(iface);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    joined.insert(iface);
                }
                Err(e) => debug!("Nie dołączono do grupy multicast na {iface}: {e}"),
            }
        }
    }
}

async fn handle_packet(
    engine: &NodeEngine,
    socket: &UdpSocket,
    shared: &Shared,
    data: &[u8],
    from: SocketAddr,
) {
    let Ok(beacon) = postcard::from_bytes::<Beacon>(data) else {
        return;
    };
    if beacon.magic != MAGIC || beacon.addr.id == engine.node_id() {
        return;
    }
    if let Ok(mut health) = shared.health.lock() {
        health.beacons_received += 1;
    }

    let id = beacon.addr.id;
    let mut addr = beacon.addr;
    for port in beacon.quic_ports.iter().take(MAX_QUIC_PORTS) {
        addr.addrs
            .insert(TransportAddr::Ip(SocketAddr::new(from.ip(), *port)));
    }
    let name = match clean_device_name(&beacon.name) {
        n if n.is_empty() => format!("Węzeł {}", id.fmt_short()),
        n => n,
    };
    let platform: String = beacon
        .platform
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    let device = NearbyDevice {
        node_id: id.to_string(),
        name,
        platform,
        id,
        addr,
    };

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
                    "Wykrywanie: nowe urządzenie '{}' ({}) z {from}",
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

    // Nowe urządzenie dowiaduje się o nas od razu, nawet gdy nasz broadcast do niego nie dociera.
    if changed
        && !beacon.reply
        && let Some(bytes) = encode_beacon(engine, shared, true)
        && let Err(e) = socket.send_to(&bytes, from).await
    {
        debug!("Nie wysłano odpowiedzi do {from}: {e}");
    }
}

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

/// Adresy IPv4 tego urządzenia w sieciach lokalnych wraz z adresem broadcast sieci.
fn local_networks(engine: &NodeEngine) -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let mut networks = platform_networks(engine);
    networks.sort();
    networks.dedup();
    networks
}

#[cfg(not(target_os = "android"))]
fn platform_networks(_engine: &NodeEngine) -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
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

#[cfg(target_os = "android")]
fn platform_networks(engine: &NodeEngine) -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
    let networks = android::interfaces();
    if !networks.is_empty() {
        return networks;
    }
    // Gdy getifaddrs nic nie zwróci – przybliżenie /24 z adresów wykrytych przez iroh.
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

/// Lista interfejsów na Androidzie. `getifaddrs` z bioniki działa bez dodatkowych
/// uprawnień (od Androida 11 bez adresów MAC, ale z adresami IP i maskami).
/// Kompilowane także w testach na Linuksie (glibc ma to samo API), aby je sprawdzić.
#[cfg(any(target_os = "android", all(test, target_os = "linux")))]
mod android {
    use std::net::Ipv4Addr;

    pub fn interfaces() -> Vec<(Ipv4Addr, Option<Ipv4Addr>)> {
        let mut networks = Vec::new();
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: przy sukcesie getifaddrs zwraca listę zwalnianą niżej przez freeifaddrs.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return networks;
        }
        let mut current = head;
        while !current.is_null() {
            // SAFETY: `current` wskazuje element listy zwróconej przez getifaddrs.
            let ifa = unsafe { &*current };
            current = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() {
                continue;
            }
            // SAFETY: ifa_addr nie jest pusty; rodzina adresu decyduje o typie struktury.
            if i32::from(unsafe { (*ifa.ifa_addr).sa_family }) != libc::AF_INET {
                continue;
            }
            // SAFETY: dla AF_INET adres i maska są typu sockaddr_in.
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
            // Interfejsy punkt-punkt (VPN: tun0, wg0) nie mają adresu broadcast.
            let has_broadcast = ifa.ifa_flags & libc::IFF_BROADCAST as u32 != 0;
            let broadcast = Ipv4Addr::from(ip | !mask);
            networks.push((
                addr,
                (has_broadcast && broadcast != addr).then_some(broadcast),
            ));
        }
        // SAFETY: zwalniamy dokładnie listę zaalokowaną przez getifaddrs.
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

    /// Wersja androidowa (getifaddrs) zwraca te same sieci co biblioteka desktopowa.
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
        let list = vec![device("Laptop Kasi"), device("Telefon")];
        assert_eq!(find_in(&list, "laptop kasi").unwrap().name, "Laptop Kasi");
        assert_eq!(find_in(&list, &list[1].node_id).unwrap().name, "Telefon");
        assert_eq!(
            find_in(&list, &list[1].node_id[..10]).unwrap().name,
            "Telefon"
        );
        assert!(find_in(&list, &list[1].node_id[..4]).is_none());
        assert!(find_in(&list, "brak").is_none());
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
        let beacon = Beacon {
            magic: MAGIC,
            name: "x".repeat(64),
            platform: "android".into(),
            addr,
            quic_ports: vec![1, 2, 3, 4],
            reply: false,
        };
        assert!(postcard::to_allocvec(&beacon).unwrap().len() < MAX_BEACON_LEN);
    }
}
