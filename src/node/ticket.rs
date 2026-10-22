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

    pub fn deserialize_from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let body = s
            .strip_prefix("rqs://")
            .or_else(|| s.strip_prefix(TICKET_PREFIX))
            .unwrap_or(s);
        let bytes = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|e| anyhow!("Błąd dekodowania Base64: {}", e))?;
        let ticket: NodeTicket = postcard::from_bytes(&bytes)
            .map_err(|e| anyhow!("Błąd deserializacji Postcard: {}", e))?;
        Ok(ticket)
    }

    /// Akceptuje TICKET lub surowy Node ID (adres zostanie odnaleziony przez DNS/pkarr).
    pub fn parse_target(s: &str) -> Result<EndpointAddr> {
        if let Ok(ticket) = Self::deserialize_from_str(s) {
            return Ok(ticket.addr);
        }
        let id = EndpointId::from_str(s.trim()).context("Nieprawidłowy TICKET lub Node ID")?;
        Ok(id.into())
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
        assert_eq!(NodeTicket::parse_target(&id.to_string())?.id, id);
        Ok(())
    }
}
