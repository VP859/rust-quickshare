//! The packets on the wire: signed announcements ([`Announcement`]), the
//! encrypted answers of hidden devices to their paired devices, and their
//! checks (freshness, signature, size).

use super::{Shared, Visibility};
use crate::node::NodeEngine;
use crate::pairing::DiscoveryKey;
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// First bytes of every announcement; other packets on the port are ignored.
const MAGIC: [u8; 4] = *b"FRYd";

/// Largest accepted announcement in bytes.
pub(super) const MAX_BEACON_LEN: usize = 2048;

/// Bytes [`seal`] adds to an announcement: nonce and authentication tag.
pub(super) const SEAL_OVERHEAD: usize = SEAL_NONCE_LEN + SEAL_TAG_LEN;

/// Length of the random nonce at the start of a sealed packet.
const SEAL_NONCE_LEN: usize = 16;

/// Length of the authentication tag at the end of a sealed packet.
const SEAL_TAG_LEN: usize = 32;

/// Largest number of QUIC ports taken from one announcement.
pub(super) const MAX_QUIC_PORTS: usize = 4;

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

/// Current time in seconds since the Unix epoch (0 if the clock is before it).
pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Encryption and authentication keys of a [`DiscoveryKey`] (separate keys for
/// separate purposes).
fn seal_keys(key: &DiscoveryKey) -> ([u8; 32], [u8; 32]) {
    (
        blake3::derive_key("ferry discovery encryption v1", key),
        blake3::derive_key("ferry discovery authentication v1", key),
    )
}

/// The authentication tag of a sealed packet: a keyed BLAKE3 hash of the nonce
/// and the ciphertext.
fn seal_tag(mac_key: &[u8; 32], nonce_and_ciphertext: &[u8]) -> blake3::Hash {
    blake3::Hasher::new_keyed(mac_key)
        .update(nonce_and_ciphertext)
        .finalize()
}

/// Encrypts `plain` (a signed announcement) for the device that shares `key`
/// with this one: `nonce ‖ ciphertext ‖ tag`, with nothing recognizable in it,
/// so a hidden device's answer looks like random bytes to everyone else.
///
/// The ciphertext is `plain` XOR the BLAKE3 output (extendable, keyed with the
/// encryption key) for the random nonce, the tag a keyed BLAKE3 hash of nonce
/// and ciphertext (encrypt-then-MAC); BLAKE3 in keyed mode is a PRF and a MAC.
/// `None` only when the system has no random numbers.
pub(super) fn seal(key: &DiscoveryKey, plain: &[u8]) -> Option<Vec<u8>> {
    let (enc_key, mac_key) = seal_keys(key);
    let mut out = vec![0u8; SEAL_NONCE_LEN];
    getrandom::fill(&mut out).ok()?;
    let mut stream = vec![0u8; plain.len()];
    blake3::Hasher::new_keyed(&enc_key)
        .update(&out)
        .finalize_xof()
        .fill(&mut stream);
    out.extend(plain.iter().zip(&stream).map(|(p, s)| p ^ s));
    let tag = seal_tag(&mac_key, &out);
    out.extend_from_slice(tag.as_bytes());
    Some(out)
}

/// Decrypts a packet made by [`seal`] with `key`; `None` when it was not made
/// with this key or was changed on the way.
fn open_sealed(key: &DiscoveryKey, packet: &[u8]) -> Option<Vec<u8>> {
    if packet.len() <= SEAL_OVERHEAD {
        return None;
    }
    let (enc_key, mac_key) = seal_keys(key);
    let (signed, tag) = packet.split_at(packet.len() - SEAL_TAG_LEN);
    // `blake3::Hash` compares in constant time.
    if seal_tag(&mac_key, signed) != blake3::Hash::from_bytes(tag.try_into().ok()?) {
        return None;
    }
    let (nonce, cipher) = signed.split_at(SEAL_NONCE_LEN);
    let mut stream = vec![0u8; cipher.len()];
    blake3::Hasher::new_keyed(&enc_key)
        .update(nonce)
        .finalize_xof()
        .fill(&mut stream);
    Some(cipher.iter().zip(&stream).map(|(c, s)| c ^ s).collect())
}

/// Reads a packet that is not a plain announcement: tries the secrets of the
/// paired devices and returns the announcement inside, if it is a valid
/// answer from the device whose secret opened it.
pub(super) fn decode_sealed(
    visibility: &Visibility,
    data: &[u8],
    now: u64,
) -> Option<Announcement> {
    visibility.keys.iter().find_map(|(owner, key)| {
        let beacon = decode_beacon(&open_sealed(key, data)?, now)?;
        // Only answers are sent sealed, and only by the owner of the secret.
        (beacon.reply && beacon.addr.id == *owner).then_some(beacon)
    })
}

/// Serializes and signs the current announcement; `None` if it would not fit
/// in one datagram. `private` is for a packet only a paired device can read.
pub(super) fn encode_beacon(
    engine: &NodeEngine,
    shared: &Shared,
    reply: bool,
    private: bool,
) -> Option<Vec<u8>> {
    let endpoint = engine.endpoint();
    let platform = if private || shared.visibility.read().is_ok_and(|v| v.announce_platform) {
        std::env::consts::OS.to_string()
    } else {
        String::new()
    };
    let beacon = Announcement {
        timestamp: unix_now(),
        name: shared.name.read().map(|n| n.clone()).unwrap_or_default(),
        platform,
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
pub(super) fn decode_beacon(data: &[u8], now: u64) -> Option<Announcement> {
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

/// Entry point for the `announcement` fuzz target (feature `fuzzing`): decodes
/// `data` as a datagram, and as the body of an announcement an attacker signed
/// with their own key, and checks what would reach the device list.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_announcement(data: &[u8]) {
    use super::DISCOVERY_PORT;
    use super::exchange::nearby_device;
    use std::net::SocketAddr;

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

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{EndpointId, TransportAddr};
    use std::net::SocketAddr;

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
    fn sealed_packets_are_unreadable_and_tamper_proof() {
        let key = [7u8; 32];
        let plain = b"FRYd some signed announcement with a name and a Node ID".to_vec();
        let packet = seal(&key, &plain).unwrap();
        assert_eq!(packet.len(), plain.len() + SEAL_OVERHEAD);
        assert!(
            !packet.windows(4).any(|w| w == MAGIC),
            "no recognizable marker in the packet"
        );
        assert_eq!(open_sealed(&key, &packet), Some(plain.clone()));
        assert_ne!(
            packet,
            seal(&key, &plain).unwrap(),
            "a new nonce every time"
        );
        assert_eq!(open_sealed(&[8; 32], &packet), None, "another secret");
        for i in 0..packet.len() {
            let mut bad = packet.clone();
            bad[i] ^= 1;
            assert_eq!(open_sealed(&key, &bad), None, "byte {i} changed");
        }
        assert_eq!(open_sealed(&key, &packet[..packet.len() - 1]), None);
        assert_eq!(open_sealed(&key, &[]), None);
        assert_eq!(open_sealed(&key, &[0; SEAL_OVERHEAD]), None);
    }

    #[test]
    fn only_answers_of_the_owner_of_the_secret_are_accepted() {
        let (owner, other) = (iroh::SecretKey::generate(), iroh::SecretKey::generate());
        let key = [3u8; 32];
        let visibility = Visibility {
            keys: [(owner.public(), key)].into(),
            ..Visibility::default()
        };
        let now = unix_now();
        let announcement = |key: &iroh::SecretKey, reply| Announcement {
            timestamp: now,
            name: "Hidden".into(),
            platform: String::new(),
            addr: key.public().into(),
            quic_ports: vec![1],
            reply,
        };
        let sealed = |signer: &iroh::SecretKey, reply| {
            let plain = sign_announcement(signer, &announcement(signer, reply)).unwrap();
            seal(&key, &plain).unwrap()
        };
        assert!(decode_sealed(&visibility, &sealed(&owner, true), now).is_some());
        assert!(
            decode_sealed(&visibility, &sealed(&owner, false), now).is_none(),
            "only answers are sent sealed"
        );
        assert!(
            decode_sealed(&visibility, &sealed(&other, true), now).is_none(),
            "a device with the secret cannot speak for another one"
        );
        assert!(decode_sealed(&visibility, &sealed(&owner, true), now + 10_000).is_none());
        assert!(decode_sealed(&Visibility::default(), &sealed(&owner, true), now).is_none());
    }
}
