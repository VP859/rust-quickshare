//! Sockets and networks: binding the discovery port, multicast groups,
//! sending to every local network, the networks of this device (also on
//! Android) and the firewall check.

use super::{MULTICAST_GROUP, Shared};
use crate::node::NodeEngine;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// Checks the firewall on a separate thread (it runs external programs) and
/// stores the result in [`DiscoveryHealth::firewall_blocking`](super::DiscoveryHealth::firewall_blocking).
pub(super) fn spawn_firewall_check(shared: Arc<Shared>, port: u16) {
    std::thread::spawn(move || {
        let blocking = blocking_firewall_for(port, "udp");
        if let Some(name) = &blocking {
            warn!("Discovery: {name} blocks incoming UDP {port}");
        }
        if let Ok(mut health) = shared.health.lock() {
            health.firewall_blocking = blocking;
        }
    });
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

/// Outcome of sending one announcement round to local networks.
#[derive(Default)]
pub(super) struct LanSendStats {
    /// At least one send succeeded.
    pub(super) ok: bool,
    /// Last send error.
    pub(super) error: Option<String>,
}

impl LanSendStats {
    /// Records the result of one send.
    pub(super) fn record(&mut self, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.ok = true,
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}

/// Sends one datagram, logging failures at debug level.
pub(super) async fn send(
    socket: &UdpSocket,
    bytes: &[u8],
    target: SocketAddr,
) -> std::io::Result<()> {
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
pub(super) fn join_multicast(
    engine: &NodeEngine,
    socket: &UdpSocket,
    joined: &mut BTreeSet<Ipv4Addr>,
) {
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

// The only test compares with the netlink listing of Linux.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

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
}
