//! LocalSend discovery: announcements on the multicast group 224.0.0.167:53317.
//!
//! On start this device announces itself (`"announce": true`) three times
//! (after 0.1, 0.5 and 2 s, like the official app). LocalSend apps that hear
//! it register with the HTTPS server (`POST /api/localsend/v2/register`) and
//! list this device. When a LocalSend app announces itself (it does so when
//! opened or refreshed), this device answers with its own message
//! (`"announce": false`), to which the app reacts the same way. Answers are
//! never answered, so two Ferry devices cannot keep each other talking.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::MULTICAST_GROUP;
use super::dto::DeviceInfo;
use super::server::OwnInfo;
use crate::node::NodeEngine;
use crate::transfer::service::TransferHandler;

/// Pauses of the announcement burst.
const ANNOUNCE_DELAYS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(400),
    Duration::from_millis(1500),
];
/// How often new network interfaces are looked for.
const REJOIN_INTERVAL: Duration = Duration::from_secs(15);
/// The same device is answered at most this often.
const ANSWER_INTERVAL: Duration = Duration::from_secs(2);
/// Largest datagram read.
const MAX_DATAGRAM: usize = 16 * 1024;

/// Builds this device's multicast messages.
pub struct Announcer {
    /// This device's description.
    info: OwnInfo,
    /// Current device name.
    alias: Box<dyn Fn() -> String + Send + Sync>,
}

impl Announcer {
    /// Messages for `info` with the device name of `handler`.
    pub(super) fn new<H: TransferHandler>(info: OwnInfo, handler: Arc<H>) -> Self {
        Self {
            info,
            alias: Box::new(move || handler.device_name()),
        }
    }

    /// The JSON datagram; `announce` asks others to answer.
    pub fn message(&self, announce: bool) -> Vec<u8> {
        let info = DeviceInfo {
            port: Some(self.info.port),
            protocol: Some("https".to_string()),
            announce: Some(announce),
            ..self.info.describe((self.alias)())
        };
        serde_json::to_vec(&info).unwrap_or_default()
    }

    /// Whether a datagram is an announcement of another device that should
    /// be answered.
    pub fn should_answer(&self, datagram: &[u8]) -> bool {
        serde_json::from_slice::<DeviceInfo>(datagram).is_ok_and(|other| {
            other.fingerprint != self.info.fingerprint && other.announce == Some(true)
        })
    }
}

/// Opens the multicast socket on `port`, shared with other apps (a LocalSend
/// app on the same computer).
///
/// # Errors
///
/// Fails when the socket cannot be created.
pub fn bind(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::from_std(crate::discovery::bind(port)?)
}

/// Sends `bytes` to the group through loopback and every local interface.
pub(super) async fn send_to_group(
    socket: &UdpSocket,
    bytes: &[u8],
    interfaces: &BTreeSet<Ipv4Addr>,
) {
    let group = SocketAddr::from(SocketAddrV4::new(MULTICAST_GROUP, super::PORT));
    for iface in std::iter::once(&Ipv4Addr::LOCALHOST).chain(interfaces) {
        if socket2::SockRef::from(socket)
            .set_multicast_if_v4(iface)
            .is_ok()
            && let Err(e) = socket.send_to(bytes, group).await
        {
            debug!("LocalSend announcement via {iface} failed: {e}");
        }
    }
}

/// Joins the group on loopback and every local interface not joined yet;
/// returns whether a new one was joined.
pub(super) fn join(
    engine: &NodeEngine,
    socket: &UdpSocket,
    joined: &mut BTreeSet<Ipv4Addr>,
) -> bool {
    let mut new = false;
    let current: Vec<Ipv4Addr> = crate::discovery::local_networks(Some(engine))
        .into_iter()
        .map(|(ip, _)| ip)
        .collect();
    for iface in [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOCALHOST]
        .into_iter()
        .chain(current.iter().copied())
    {
        if joined.contains(&iface) {
            continue;
        }
        match socket.join_multicast_v4(MULTICAST_GROUP, iface) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(e) => {
                debug!("LocalSend: cannot join the group on {iface}: {e}");
                continue;
            }
        }
        joined.insert(iface);
        new |= !iface.is_unspecified() && !iface.is_loopback();
    }
    joined.retain(|ip| ip.is_unspecified() || ip.is_loopback() || current.contains(ip));
    new
}

/// Announces this device and answers announcements until `stop`.
pub(super) async fn run(
    engine: NodeEngine,
    socket: UdpSocket,
    announcer: Announcer,
    stop: CancellationToken,
) {
    let _ = socket.set_multicast_loop_v4(true);
    let _ = socket.set_multicast_ttl_v4(1);
    let mut joined = BTreeSet::new();
    join(&engine, &socket, &mut joined);
    let interfaces = |joined: &BTreeSet<Ipv4Addr>| -> BTreeSet<Ipv4Addr> {
        joined
            .iter()
            .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
            .copied()
            .collect()
    };

    let mut burst = ANNOUNCE_DELAYS.iter();
    let mut next_announce = tokio::time::Instant::now() + ANNOUNCE_DELAYS[0];
    let mut rejoin = tokio::time::interval(REJOIN_INTERVAL);
    let mut answered: HashMap<IpAddr, Instant> = HashMap::new();
    let mut buf = vec![0u8; MAX_DATAGRAM];
    burst.next();
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep_until(next_announce) => {
                send_to_group(&socket, &announcer.message(true), &interfaces(&joined)).await;
                next_announce = match burst.next() {
                    Some(delay) => tokio::time::Instant::now() + *delay,
                    // Until the next network change.
                    None => tokio::time::Instant::now() + Duration::from_secs(365 * 24 * 3600),
                };
            }
            _ = rejoin.tick() => {
                if join(&engine, &socket, &mut joined) {
                    // A new network: announce there too.
                    burst = ANNOUNCE_DELAYS.iter();
                    burst.next();
                    next_announce = tokio::time::Instant::now() + ANNOUNCE_DELAYS[0];
                }
            }
            received = socket.recv_from(&mut buf) => {
                let Ok((len, from)) = received else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                // Every LocalSend app heard is a possible recipient.
                if let Ok(info) = serde_json::from_slice::<DeviceInfo>(&buf[..len]) {
                    super::client::remember_info(&info, from.ip(), &announcer.info.fingerprint);
                }
                if !announcer.should_answer(&buf[..len]) {
                    continue;
                }
                let now = Instant::now();
                answered.retain(|_, at| now.duration_since(*at) < ANSWER_INTERVAL);
                if answered.len() < 256 && !answered.contains_key(&from.ip()) {
                    answered.insert(from.ip(), now);
                    send_to_group(&socket, &announcer.message(false), &interfaces(&joined)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named;
    impl TransferHandler for Named {
        async fn decide(
            &self,
            _: iroh::EndpointId,
            _: &str,
            _: &crate::protocol::messages::TransferOffer,
        ) -> crate::transfer::service::OfferDecision {
            unreachable!()
        }
        fn on_event(&self, _: crate::transfer::service::TransferEvent) {}
        fn device_name(&self) -> String {
            "Laptop".into()
        }
    }

    fn announcer() -> Announcer {
        let info = OwnInfo {
            fingerprint: "ABC".into(),
            device_type: "desktop",
            port: 53317,
        };
        Announcer::new(info, Arc::new(Named))
    }

    #[test]
    fn messages_carry_what_localsend_expects() {
        let message: serde_json::Value =
            serde_json::from_slice(&announcer().message(true)).unwrap();
        assert_eq!(message["alias"], "Laptop");
        assert_eq!(message["fingerprint"], "ABC");
        assert_eq!(message["port"], 53317);
        assert_eq!(message["protocol"], "https");
        assert_eq!(message["announce"], true);
        assert_eq!(message["deviceType"], "desktop");
        assert_eq!(message["download"], false);
    }

    #[test]
    fn only_announcements_of_others_are_answered() {
        let a = announcer();
        let other = |fingerprint: &str, announce: Option<bool>| {
            serde_json::to_vec(&DeviceInfo {
                alias: "Phone".into(),
                version: "2.2".into(),
                fingerprint: fingerprint.into(),
                announce,
                ..DeviceInfo::default()
            })
            .unwrap()
        };
        assert!(a.should_answer(&other("XYZ", Some(true))));
        assert!(
            !a.should_answer(&other("XYZ", Some(false))),
            "answers are not answered"
        );
        assert!(!a.should_answer(&other("XYZ", None)));
        assert!(!a.should_answer(&other("ABC", Some(true))), "own message");
        assert!(!a.should_answer(b"not json"));
        assert!(!a.should_answer(&a.message(true)));
    }
}
