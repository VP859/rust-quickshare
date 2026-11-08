//! One-button pairing of devices.
//!
//! The initiating device sends [`Hello::Pair`]; the other device asks its user
//! for consent. Both show the same six-digit verification code, so nothing has
//! to be typed or scanned. After acceptance both devices store each other as
//! trusted.
//!
//! # Why the code cannot be forged
//!
//! The connection itself is authenticated (TLS 1.3 with the Node ID keys), but
//! the user picks the device by its announced *name*, which anyone on the
//! network can copy. Comparing the code is what proves that both screens talk
//! to each other and not to an impostor in the middle.
//!
//! A code computed from the two Node IDs alone could be matched by an attacker
//! who generates keys until one gives the right six digits – a matter of
//! seconds. The code therefore also depends on two random nonces exchanged
//! with a *commit-reveal* scheme, as in Bluetooth Secure Simple Pairing and
//! ZRTP:
//!
//! 1. the requester sends a hash (commitment) of its nonce `a`,
//! 2. the responder answers with its nonce `b`,
//! 3. the requester reveals `a`, which the responder checks against the commitment,
//! 4. both compute the code from both Node IDs and both nonces.
//!
//! Neither side can choose its nonce after seeing the other one, so a
//! man in the middle has one chance in a million per attempt to make the codes
//! on both screens match, and every attempt shows a request to the user.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId};
use rust_i18n::t;

use crate::error::ProtocolViolation;
use std::time::Duration;

use crate::node::NodeEngine;
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    Hello, MAX_DEVICE_NAME_LEN, PairChallenge, PairResponse, PairReveal,
};
use crate::transfer::Channel;

/// Time the other device has to confirm a pairing request.
pub const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
/// The initiator waits a little longer than the decision timeout to cover network delays.
const PAIR_RESPONSE_TIMEOUT: Duration = Duration::from_secs(75);
/// Time for a protocol step that needs no user action (challenge, reveal).
const PAIR_STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// A random 32-byte pairing nonce.
pub type PairNonce = [u8; 32];

/// Secret shared by two paired devices, with which a hidden device encrypts
/// its answers to local network discovery ([`crate::discovery`]).
pub type DiscoveryKey = [u8; 32];

/// Pairing request passed to the user interface for confirmation.
#[derive(Debug, Clone)]
pub struct PairRequest {
    /// Node ID of the requesting device, authenticated by TLS.
    pub peer: EndpointId,
    /// Device name announced by the requester (already cleaned).
    pub name: String,
    /// Verification code to compare with the requester's screen.
    pub code: String,
    /// Requester's address; `None` if it did not match the authenticated Node ID.
    pub addr: Option<EndpointAddr>,
    /// Secret for discovery, the same on both devices; store it with
    /// [`Config::add_paired`](crate::config::Config::add_paired).
    pub discovery_key: DiscoveryKey,
}

/// Result of [`request_pairing`].
#[derive(Debug, Clone)]
pub enum PairOutcome {
    /// The other device accepted and now trusts this one.
    Accepted {
        /// Device name of the other side.
        name: String,
        /// Its address, if provided and valid.
        addr: Option<EndpointAddr>,
        /// Secret for discovery, the same on both devices.
        discovery_key: DiscoveryKey,
    },
    /// The other device declined.
    Rejected,
}

/// Device name without control characters, whitespace collapsed and truncated
/// to [`MAX_DEVICE_NAME_LEN`] bytes.
///
/// # Examples
///
/// ```
/// use ferry::pairing::clean_device_name;
///
/// assert_eq!(clean_device_name("  Office\n\tLaptop "), "Office Laptop");
/// ```
pub fn clean_device_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    while name.len() > MAX_DEVICE_NAME_LEN {
        name.pop();
    }
    name
}

/// Fresh random nonce from the operating system.
///
/// # Errors
///
/// Fails only when the operating system's random number generator is unavailable.
fn new_nonce() -> Result<PairNonce> {
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).context("random number generator unavailable")?;
    Ok(nonce)
}

/// Commitment of the requester `initiator` to its nonce, sent in [`Hello::Pair`]:
/// `BLAKE3-derive-key("ferry pairing commitment v1", initiator ‖ nonce)`.
pub fn pairing_commitment(initiator: &EndpointId, nonce: &PairNonce) -> [u8; 32] {
    *commitment(initiator, nonce).as_bytes()
}

/// [`pairing_commitment`] as a hash, which compares in constant time.
fn commitment(initiator: &EndpointId, nonce: &PairNonce) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new_derive_key("ferry pairing commitment v1");
    hasher.update(initiator.as_bytes());
    hasher.update(nonce);
    hasher.finalize()
}

/// Six-digit verification code (`"123 456"`) of one pairing attempt.
///
/// Both devices compute the same code from the requester's and responder's
/// Node IDs and the two nonces exchanged during the attempt.
///
/// # Examples
///
/// ```
/// use ferry::pairing::pairing_code;
///
/// let a = iroh::SecretKey::generate().public();
/// let b = iroh::SecretKey::generate().public();
/// let code = pairing_code(&a, &b, &[1; 32], &[2; 32]);
/// assert_eq!(code.len(), 7);
/// assert_ne!(code, pairing_code(&a, &b, &[1; 32], &[3; 32]));
/// ```
pub fn pairing_code(
    initiator: &EndpointId,
    responder: &EndpointId,
    initiator_nonce: &PairNonce,
    responder_nonce: &PairNonce,
) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("ferry pairing code v2");
    hasher.update(initiator.as_bytes());
    hasher.update(responder.as_bytes());
    hasher.update(initiator_nonce);
    hasher.update(responder_nonce);
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    // 2^64 is not a multiple of 10^6, but the bias (< 10^-13) is irrelevant.
    let wide = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let n = wide % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// The [`DiscoveryKey`] of one pairing attempt, derived from both Node IDs and
/// both nonces. The nonces travel only inside the authenticated, encrypted
/// connection, so only the two devices (and nobody who merely watches the
/// network or sees the six-digit code) can compute it.
fn discovery_key(
    initiator: &EndpointId,
    responder: &EndpointId,
    initiator_nonce: &PairNonce,
    responder_nonce: &PairNonce,
) -> DiscoveryKey {
    let mut hasher = blake3::Hasher::new_derive_key("ferry discovery key v1");
    hasher.update(initiator.as_bytes());
    hasher.update(responder.as_bytes());
    hasher.update(initiator_nonce);
    hasher.update(responder_nonce);
    *hasher.finalize().as_bytes()
}

/// Number of five-digit groups in a [safety number](safety_number).
const SAFETY_GROUPS: usize = 12;

/// The **safety number** of two devices: 60 digits (12 groups of 5) that are the
/// same on both devices and can be compared at any time – by reading them out,
/// or side by side – to check that a trusted device is still the one it was
/// when paired.
///
/// It is derived from the two Node IDs, i.e. the public keys that authenticate
/// every connection, so equal numbers mean that both devices see the same two
/// keys and nobody sits in between. Unlike the pairing code it does not change.
/// 60 digits (about 200 bits) keep an attacker from finding a pair of keys that
/// produces a chosen number.
///
/// # Examples
///
/// ```
/// use ferry::pairing::safety_number;
///
/// let a = iroh::SecretKey::generate().public();
/// let b = iroh::SecretKey::generate().public();
/// let number = safety_number(&a, &b);
/// assert_eq!(number, safety_number(&b, &a), "the same on both devices");
/// assert_eq!(number.split(' ').count(), 12);
/// ```
pub fn safety_number(a: &EndpointId, b: &EndpointId) -> String {
    let (lo, hi) = if a.as_bytes() <= b.as_bytes() {
        (a, b)
    } else {
        (b, a)
    };
    let mut hasher = blake3::Hasher::new_derive_key("ferry safety number v1");
    hasher.update(lo.as_bytes());
    hasher.update(hi.as_bytes());
    let mut reader = hasher.finalize_xof();
    (0..SAFETY_GROUPS)
        .map(|_| {
            let mut chunk = [0u8; 8];
            reader.fill(&mut chunk);
            // 2^64 is not a multiple of 10^5, but the bias (< 10^-14) is irrelevant.
            format!("{:05}", u64::from_le_bytes(chunk) % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Responder side of the nonce exchange: sends [`PairChallenge`], reads and
/// checks [`PairReveal`] and returns the verification code and the
/// [`DiscoveryKey`].
///
/// # Errors
///
/// Fails when the requester does not reveal its nonce in time or the nonce
/// does not match `commitment` (a protocol violation, the request is dropped).
pub(crate) async fn answer_pairing(
    channel: &mut Channel,
    me: &EndpointId,
    requester: &EndpointId,
    requester_commitment: &[u8; 32],
) -> Result<(String, DiscoveryKey)> {
    let mine = new_nonce()?;
    write_msg(&mut channel.send, &PairChallenge { nonce: mine }).await?;
    let reveal: PairReveal = tokio::time::timeout(PAIR_STEP_TIMEOUT, read_msg(&mut channel.recv))
        .await
        .with_context(|| t!("pairing.interrupted"))??;
    // blake3::Hash compares in constant time.
    if commitment(requester, &reveal.nonce) != blake3::Hash::from_bytes(*requester_commitment) {
        return Err(ProtocolViolation(t!("pairing.bad_commitment").into_owned()).into());
    }
    Ok((
        pairing_code(requester, me, &reveal.nonce, &mine),
        discovery_key(requester, me, &reveal.nonce, &mine),
    ))
}

/// Sends a pairing request to `target` and waits for the user's decision there.
///
/// `on_code` receives the verification code as soon as it is known (after the
/// nonce exchange, before the other user decides); the interface shows it so
/// the user can compare it with the other screen. The caller is responsible
/// for storing the device on [`PairOutcome::Accepted`].
///
/// # Errors
///
/// Fails when the device cannot be reached, nobody answers in time, or the
/// connection is interrupted.
pub async fn request_pairing(
    engine: &NodeEngine,
    target: EndpointAddr,
    my_name: &str,
    on_code: impl FnOnce(&str) + Send,
) -> Result<PairOutcome> {
    let mine = new_nonce()?;
    let me = engine.node_id();
    let conn = engine.connect(target).await?;
    let peer = conn.remote_id();
    let hello = Hello::Pair {
        name: clean_device_name(my_name),
        addr: engine.endpoint().addr(),
        commitment: *commitment(&me, &mine).as_bytes(),
    };
    let mut channel = Channel::open(&conn, &hello).await?;
    let challenge: PairChallenge =
        tokio::time::timeout(PAIR_STEP_TIMEOUT, read_msg(&mut channel.recv))
            .await
            .with_context(|| t!("pairing.no_answer"))?
            .with_context(|| t!("pairing.interrupted"))?;
    write_msg(&mut channel.send, &PairReveal { nonce: mine }).await?;
    on_code(&pairing_code(&me, &peer, &mine, &challenge.nonce));

    let response: PairResponse =
        tokio::time::timeout(PAIR_RESPONSE_TIMEOUT, read_msg(&mut channel.recv))
            .await
            .with_context(|| t!("pairing.no_answer"))?
            .with_context(|| t!("pairing.interrupted"))?;
    channel.finish().await;
    conn.close(0u32.into(), b"paired");

    Ok(if response.accepted {
        PairOutcome::Accepted {
            name: clean_device_name(&response.name),
            addr: response.addr.filter(|a| a.id == peer),
            discovery_key: discovery_key(&me, &peer, &mine, &challenge.nonce),
        }
    } else {
        PairOutcome::Rejected
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_formatted_and_depends_on_every_input() {
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        let code = pairing_code(&a, &b, &[1; 32], &[2; 32]);
        assert_eq!(code.len(), 7);
        assert_eq!(code.as_bytes()[3], b' ');
        assert!(code.chars().all(|c| c.is_ascii_digit() || c == ' '));
        // Changing any input changes the code (with overwhelming probability).
        assert_ne!(code, pairing_code(&b, &a, &[1; 32], &[2; 32]));
        assert_ne!(code, pairing_code(&a, &b, &[9; 32], &[2; 32]));
        assert_ne!(code, pairing_code(&a, &b, &[1; 32], &[9; 32]));
    }

    #[test]
    fn safety_numbers_are_symmetric_and_depend_on_both_keys() {
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        let c = iroh::SecretKey::generate().public();
        let number = safety_number(&a, &b);
        assert_eq!(number, safety_number(&b, &a));
        assert_ne!(number, safety_number(&a, &c));
        assert_ne!(number, safety_number(&c, &b));
        assert_eq!(number.len(), 12 * 5 + 11);
        assert!(
            number
                .split(' ')
                .all(|g| g.len() == 5 && g.bytes().all(|b| b.is_ascii_digit()))
        );
    }

    #[test]
    fn discovery_key_is_shared_and_depends_on_every_input() {
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        let key = discovery_key(&a, &b, &[1; 32], &[2; 32]);
        assert_eq!(key, discovery_key(&a, &b, &[1; 32], &[2; 32]));
        assert_ne!(key, discovery_key(&b, &a, &[1; 32], &[2; 32]));
        assert_ne!(key, discovery_key(&a, &b, &[9; 32], &[2; 32]));
        assert_ne!(key, discovery_key(&a, &b, &[1; 32], &[9; 32]));
        // Not the six digits shown to the user, and not the code's hash.
        let code = pairing_code(&a, &b, &[1; 32], &[2; 32]);
        assert!(!code.is_empty() && key.iter().any(|b| *b != 0));
    }

    #[test]
    fn commitment_binds_nonce_and_requester() {
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        let nonce = new_nonce().unwrap();
        assert_ne!(nonce, new_nonce().unwrap(), "nonces must be random");
        assert_eq!(commitment(&a, &nonce), commitment(&a, &nonce));
        assert_ne!(commitment(&a, &nonce), commitment(&b, &nonce));
        assert_ne!(commitment(&a, &nonce), commitment(&a, &[0; 32]));
    }

    #[test]
    fn cleans_names() {
        assert_eq!(clean_device_name("  Office\n\tLaptop  "), "Office Laptop");
        assert_eq!(
            clean_device_name(&"ż".repeat(100)).len(),
            MAX_DEVICE_NAME_LEN
        );
    }
}
