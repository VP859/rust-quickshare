//! TICKET: a shareable text (and QR code) describing how to reach a node.

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::{EndpointAddr, EndpointId};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::protocol::messages::ShareToken;

/// Prefix of the textual form of a TICKET (also used as QR code content).
pub const TICKET_PREFIX: &str = "rqs:";

/// Address of a node (Node ID, relay URL, direct addresses) and an optional
/// token of a file shared for download.
///
/// The text form is `rqs:` followed by URL-safe base64 of the [`postcard`]
/// encoding of this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTicket {
    /// How to reach the node.
    pub addr: EndpointAddr,
    /// Present when the ticket points to a shared file ("pull" mode).
    pub share: Option<ShareToken>,
}

impl NodeTicket {
    /// A pairing ticket: just the node address.
    pub fn new(addr: EndpointAddr) -> Self {
        Self { addr, share: None }
    }

    /// A ticket for downloading the file shared under `token`.
    pub fn with_share(addr: EndpointAddr, token: ShareToken) -> Self {
        Self {
            addr,
            share: Some(token),
        }
    }

    /// Encodes the ticket as `rqs:<base64>`.
    ///
    /// # Errors
    ///
    /// Fails only if serialization fails.
    pub fn serialize_to_string(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(self)?;
        Ok(format!("{TICKET_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Decodes a ticket, tolerating damage typical of copy and paste: spaces and
    /// line breaks (text wrapped by a terminal), quotes and an optional `rqs:` or
    /// `rqs://` prefix.
    ///
    /// # Errors
    ///
    /// Returns a user-friendly error for truncated or corrupted tickets.
    pub fn deserialize_from_str(s: &str) -> Result<Self> {
        let compact: String = s
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
            .collect();
        let body = compact
            .strip_prefix("rqs://")
            .or_else(|| compact.strip_prefix(TICKET_PREFIX))
            .unwrap_or(&compact);
        let broken = || anyhow!(t!("ticket.broken", len = body.len()));
        let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| broken())?;
        postcard::from_bytes(&bytes).map_err(|_| broken())
    }

    /// Whether the text looks like a TICKET rather than a Node ID.
    ///
    /// Used to pick the more helpful error message.
    pub fn looks_like_ticket(s: &str) -> bool {
        let s = s.trim();
        s.starts_with(TICKET_PREFIX) || s.starts_with("rqs://") || s.len() > 64
    }

    /// Accepts a TICKET or a raw Node ID.
    ///
    /// A bare Node ID carries no addresses; they are resolved later through DNS/pkarr.
    ///
    /// # Errors
    ///
    /// Fails when the input is neither a valid TICKET nor a valid Node ID.
    pub fn parse_target(s: &str) -> Result<EndpointAddr> {
        let ticket_err = match Self::deserialize_from_str(s) {
            Ok(ticket) => return Ok(ticket.addr),
            Err(e) => e,
        };
        match EndpointId::from_str(s.trim()) {
            Ok(id) => Ok(id.into()),
            Err(_) if Self::looks_like_ticket(s) => Err(ticket_err),
            Err(e) => Err(e).with_context(|| t!("ticket.invalid_target")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_and_without_prefix() -> Result<()> {
        rust_i18n::set_locale("en");
        let id = iroh::SecretKey::generate().public();
        let t = NodeTicket::with_share(id.into(), [3; 16]);
        let s = t.serialize_to_string()?;
        assert!(s.starts_with(TICKET_PREFIX));
        let back = NodeTicket::deserialize_from_str(&s)?;
        assert_eq!(back.addr.id, id);
        assert_eq!(back.share, Some([3; 16]));
        let bare = NodeTicket::deserialize_from_str(s.trim_start_matches(TICKET_PREFIX))?;
        assert_eq!(bare.addr.id, id);
        let (head, tail) = s.split_at(40);
        let wrapped = format!("  \"{head}\n   {tail}\"\n");
        assert_eq!(NodeTicket::deserialize_from_str(&wrapped)?.addr.id, id);
        let err = NodeTicket::deserialize_from_str(&s[..s.len() - 20]).unwrap_err();
        assert!(err.to_string().contains("incomplete"), "{err}");
        assert_eq!(NodeTicket::parse_target(&id.to_string())?.id, id);
        Ok(())
    }
}
