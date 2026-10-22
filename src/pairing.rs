//! Parowanie urządzeń jednym przyciskiem: strona inicjująca wysyła `Hello::Pair`,
//! druga strona pyta użytkownika o zgodę, a obie pokazują ten sam kod weryfikacyjny
//! (liczony z obu Node ID) – bez przepisywania TICKETU i bez skanowania QR.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId};
use std::time::Duration;

use crate::node::NodeEngine;
use crate::protocol::codec::read_msg;
use crate::protocol::messages::{Hello, MAX_DEVICE_NAME_LEN, PairResponse};
use crate::transfer::Channel;

/// Czas na potwierdzenie parowania po drugiej stronie.
pub const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
/// Strona inicjująca czeka nieco dłużej niż na samą decyzję (opóźnienia sieci).
const PAIR_RESPONSE_TIMEOUT: Duration = Duration::from_secs(75);

/// Prośba o sparowanie przekazywana do interfejsu użytkownika.
#[derive(Debug, Clone)]
pub struct PairRequest {
    pub peer: EndpointId,
    pub name: String,
    /// Kod do porównania z ekranem drugiego urządzenia.
    pub code: String,
    /// Adres zgłoszony przez proszącego (zweryfikowany względem Node ID połączenia).
    pub addr: Option<EndpointAddr>,
}

#[derive(Debug, Clone)]
pub enum PairOutcome {
    Accepted {
        name: String,
        addr: Option<EndpointAddr>,
    },
    Rejected,
}

/// Nazwa urządzenia bez znaków sterujących, przycięta do [`MAX_DEVICE_NAME_LEN`].
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

/// Sześciocyfrowy kod weryfikacyjny – identyczny na obu urządzeniach,
/// niezależnie od tego, które rozpoczęło parowanie.
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

/// Wysyła prośbę o sparowanie i czeka na decyzję drugiej strony.
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
            .context("Brak odpowiedzi – nikt nie potwierdził parowania na drugim urządzeniu")?
            .context("Drugie urządzenie przerwało parowanie")?;
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
        assert_eq!(clean_device_name("  Laptop\n\tKasi  "), "Laptop Kasi");
        assert_eq!(
            clean_device_name(&"ż".repeat(100)).len(),
            MAX_DEVICE_NAME_LEN
        );
    }
}
