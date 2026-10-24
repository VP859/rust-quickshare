//! One-button pairing of devices.
//!
//! The initiating device sends [`Hello::Pair`]; the other device asks its user
//! for consent. Both show the same verification code derived from both Node IDs,
//! so nothing has to be typed or scanned. After acceptance both devices store
//! each other as trusted.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId};
use rust_i18n::t;
use std::time::Duration;

use crate::node::NodeEngine;
use crate::protocol::codec::read_msg;
use crate::protocol::messages::{Hello, MAX_DEVICE_NAME_LEN, PairResponse};
use crate::transfer::Channel;

/// Time the other device has to confirm a pairing request.
pub const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
/// The initiator waits a little longer than the decision timeout to cover network delays.
const PAIR_RESPONSE_TIMEOUT: Duration = Duration::from_secs(75);

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
/// use rqs_p2p::pairing::clean_device_name;
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

/// Six-digit verification code (`"123 456"`), identical on both devices
/// regardless of which one started the pairing.
pub fn pairing_code(a: &EndpointId, b: &EndpointId) -> String {
    let (lo, hi) = if a.as_bytes() <= b.as_bytes() {
        (a, b)
    } else {
        (b, a)
    };
    let mut hasher = blake3::Hasher::new_derive_key("rqs pairing code v1");
    hasher.update(lo.as_bytes());
    hasher.update(hi.as_bytes());
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// Sends a pairing request to `target` and waits for the user's decision there.
///
/// The caller is responsible for storing the device on
/// [`PairOutcome::Accepted`].
///
/// # Errors
///
/// Fails when the device cannot be reached, nobody answers in time, or the
/// connection is interrupted.
pub async fn request_pairing(
    engine: &NodeEngine,
    target: EndpointAddr,
    my_name: &str,
) -> Result<PairOutcome> {
    let conn = engine.connect(target).await?;
    let peer = conn.remote_id();
    let hello = Hello::Pair {
        name: clean_device_name(my_name),
        addr: engine.endpoint().addr(),
    };
    let mut channel = Channel::open(&conn, &hello).await?;
    let response: PairResponse =
        tokio::time::timeout(PAIR_RESPONSE_TIMEOUT, read_msg(&mut channel.recv))
            .await
            .context(t!("pairing.no_answer"))?
            .context(t!("pairing.interrupted"))?;
    channel.finish().await;
    conn.close(0u32.into(), b"paired");

    Ok(if response.accepted {
        PairOutcome::Accepted {
            name: clean_device_name(&response.name),
            addr: response.addr.filter(|a| a.id == peer),
        }
    } else {
        PairOutcome::Rejected
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_symmetric_and_formatted() {
        let a = iroh::SecretKey::generate().public();
        let b = iroh::SecretKey::generate().public();
        let code = pairing_code(&a, &b);
        assert_eq!(code, pairing_code(&b, &a));
        assert_eq!(code.len(), 7);
        assert!(code.chars().all(|c| c.is_ascii_digit() || c == ' '));
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
