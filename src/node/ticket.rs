use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::protocol::messages::ShareToken;

/// Prefiks tekstowej postaci TICKETU (również treść kodu QR).
pub const TICKET_PREFIX: &str = "rqs:";

/// TICKET: adres węzła (Node ID + relay + adresy bezpośrednie) i opcjonalny
/// token jednorazowego udostępnienia pliku.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTicket {
    pub addr: EndpointAddr,
    pub share: Option<ShareToken>,
}

impl NodeTicket {
    pub fn new(addr: EndpointAddr) -> Self {
        Self { addr, share: None }
    }

    pub fn with_share(addr: EndpointAddr, token: ShareToken) -> Self {
        Self {
            addr,
            share: Some(token),
        }
    }

    pub fn serialize_to_string(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(self)?;
        Ok(format!("{TICKET_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Odporny na typowe uszkodzenia przy kopiowaniu: spacje i złamania linii
    /// (zawinięty tekst w terminalu), cudzysłowy, prefiks `rqs:` / `rqs://`.
    pub fn deserialize_from_str(s: &str) -> Result<Self> {
        let compact: String = s
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
            .collect();
        let body = compact
            .strip_prefix("rqs://")
            .or_else(|| compact.strip_prefix(TICKET_PREFIX))
            .unwrap_or(&compact);
        let broken = || {
            anyhow!(
                "TICKET jest niepełny lub uszkodzony ({} znaków) – skopiuj go w całości \
                 (np. przyciskiem „Kopiuj”) albo użyj Node ID",
                body.len()
            )
        };
        let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| broken())?;
        postcard::from_bytes(&bytes).map_err(|_| broken())
    }

    /// Czy tekst wygląda na TICKET (a nie Node ID) – do czytelnych komunikatów błędów.
    pub fn looks_like_ticket(s: &str) -> bool {
        let s = s.trim();
        s.starts_with(TICKET_PREFIX) || s.starts_with("rqs://") || s.len() > 64
    }

    /// Akceptuje TICKET lub surowy Node ID (adres zostanie odnaleziony przez DNS/pkarr).
    pub fn parse_target(s: &str) -> Result<EndpointAddr> {
        let ticket_err = match Self::deserialize_from_str(s) {
            Ok(ticket) => return Ok(ticket.addr),
            Err(e) => e,
        };
        match EndpointId::from_str(s.trim()) {
            Ok(id) => Ok(id.into()),
            Err(_) if Self::looks_like_ticket(s) => Err(ticket_err),
            Err(e) => Err(e).context("Nieprawidłowy TICKET lub Node ID"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_and_without_prefix() -> Result<()> {
        let id = iroh::SecretKey::generate().public();
        let t = NodeTicket::with_share(id.into(), [3; 16]);
        let s = t.serialize_to_string()?;
        assert!(s.starts_with(TICKET_PREFIX));
        let back = NodeTicket::deserialize_from_str(&s)?;
        assert_eq!(back.addr.id, id);
        assert_eq!(back.share, Some([3; 16]));
        let bare = NodeTicket::deserialize_from_str(s.trim_start_matches(TICKET_PREFIX))?;
        assert_eq!(bare.addr.id, id);
        // Zawinięty w terminalu i wklejony z odstępami/cudzysłowami.
        let (head, tail) = s.split_at(40);
        let wrapped = format!("  \"{head}\n   {tail}\"\n");
        assert_eq!(NodeTicket::deserialize_from_str(&wrapped)?.addr.id, id);
        // Ucięty TICKET daje czytelny błąd.
        let err = NodeTicket::deserialize_from_str(&s[..s.len() - 20]).unwrap_err();
        assert!(err.to_string().contains("niepełny"), "{err}");
        assert_eq!(NodeTicket::parse_target(&id.to_string())?.id, id);
        Ok(())
    }
}
