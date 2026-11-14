//! The running discovery: announcing this device on every network, answering
//! and recording the devices heard, and forgetting those gone quiet.

use super::network::{LanSendStats, local_networks, send};
use super::packet::{
    Announcement, MAX_QUIC_PORTS, decode_beacon, decode_sealed, encode_beacon, seal, unix_now,
};
use super::{DiscoveryOptions, Entry, MULTICAST_GROUP, NearbyDevice, Shared};
use crate::node::NodeEngine;
use crate::pairing::clean_device_name;
use iroh::TransportAddr;
use rust_i18n::t;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

/// Interval between announcements.
pub(super) const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);

/// A device disappears from the list after about three missed announcements.
const DEVICE_TTL: Duration = Duration::from_secs(17);

/// Upper bound of remembered devices, so a flood of fake announcements cannot exhaust memory.
const MAX_DEVICES: usize = 256;

/// The device described by a valid announcement received from `from`: the
/// name is cleaned (or replaced by a short Node ID), the platform reduced to a
/// short alphanumeric word and the QUIC ports combined with the sender's IP.
/// Everything in it comes from the network, so nothing is trusted as is.
pub(super) fn nearby_device(beacon: Announcement, from: SocketAddr) -> NearbyDevice {
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

/// Sends one announcement round and updates [`DiscoveryHealth`](super::DiscoveryHealth).
///
/// Only sends to local networks count towards `lan_send_ok`; loopback and
/// `255.255.255.255` (usually routed into a VPN tunnel) are ignored.
pub(super) async fn announce(
    engine: &NodeEngine,
    socket: &UdpSocket,
    opts: &DiscoveryOptions,
    shared: &Shared,
) {
    if shared.visibility.read().is_ok_and(|v| v.hidden) {
        return;
    }
    let Some(bytes) = encode_beacon(engine, shared, false, false) else {
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

/// Processes a received announcement: updates the device list and health
/// and answers a new device directly (answers are not answered again).
pub(super) async fn handle_packet(
    engine: &NodeEngine,
    socket: &UdpSocket,
    shared: &Shared,
    data: &[u8],
    from: SocketAddr,
) {
    let now = unix_now();
    let Some(beacon) = decode_beacon(data, now).or_else(|| {
        let visibility = shared.visibility.read().ok()?;
        decode_sealed(&visibility, data, now)
    }) else {
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

    // A visible device answers new devices only (it announces itself anyway).
    // A hidden one never announces, so it answers every announcement of a
    // device it trusts: one lost answer must not hide it until the next start.
    let (hidden, key) = shared
        .visibility
        .read()
        .map(|v| (v.hidden, v.keys.get(&id).copied()))
        .unwrap_or_default();
    if (changed || hidden) && !reply && shared.visibility.read().is_ok_and(|v| v.answers(&id)) {
        // A hidden device answers a paired device that has a secret with a
        // packet nobody else can read; without one (paired before secrets
        // existed) in the clear.
        let sealing = hidden.then_some(key).flatten();
        let bytes = encode_beacon(engine, shared, true, sealing.is_some())
            .and_then(|plain| sealing.map_or(Some(plain.clone()), |k| seal(&k, &plain)));
        if let Some(bytes) = bytes
            && let Err(e) = socket.send_to(&bytes, from).await
        {
            debug!("Could not answer {from}: {e}");
        }
    }
}

/// Removes devices that have not announced themselves for [`DEVICE_TTL`].
pub(super) fn expire(shared: &Shared) {
    let Ok(mut devices) = shared.devices.lock() else {
        return;
    };
    let before = devices.len();
    devices.retain(|_, e| e.seen.elapsed() < DEVICE_TTL);
    if devices.len() != before {
        shared.publish(&devices);
    }
}
