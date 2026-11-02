//! Protocol versions and how two devices agree on one.
//!
//! Every version of the Ferry protocol has its own ALPN identifier
//! (`ferry/<n>`), exchanged during the TLS handshake. A device lists all the
//! versions it speaks; the receiving side picks the best one both support, so
//! devices of different releases keep working together as long as they share
//! a version, with no extra round trip. When they share none, the connection
//! fails with a clear "incompatible version" error instead of garbled data.
//!
//! # Rules
//!
//! * An incompatible change to an existing message (a changed field, a
//!   reordered or removed variant, a different hash input) needs a **new
//!   version**. New enum variants appended at the end are not incompatible:
//!   an older device that receives one closes that stream (see
//!   `docs/PROTOCOL.md`).
//! * A new release speaks its own version and the previous one
//!   ([`ProtocolVersion::SUPPORTED`], newest first), so a single update step is
//!   always possible. A version is dropped only after a release has announced it
//!   (or at once if it has a security flaw).
//! * The negotiated version is available from [`negotiated`]; code that behaves
//!   differently between versions branches on it.

use iroh::endpoint::Connection;

/// A version of the Ferry protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ProtocolVersion {
    /// The first version, ALPN `ferry/1`: the messages of
    /// [`messages`](super::messages).
    V1,
}

impl ProtocolVersion {
    /// Versions this build speaks, **newest first**.
    pub const SUPPORTED: &'static [Self] = &[Self::V1];

    /// The newest version.
    pub const CURRENT: Self = Self::V1;

    /// ALPN identifier of this version.
    pub const fn alpn(self) -> &'static [u8] {
        match self {
            Self::V1 => b"ferry/1",
        }
    }

    /// Version with the given ALPN identifier; `None` for unknown ones.
    ///
    /// # Examples
    ///
    /// ```
    /// use ferry::protocol::version::ProtocolVersion;
    ///
    /// assert_eq!(ProtocolVersion::from_alpn(b"ferry/1"), Some(ProtocolVersion::V1));
    /// assert_eq!(ProtocolVersion::from_alpn(b"ferry/99"), None);
    /// assert_eq!(ProtocolVersion::from_alpn(b"h3"), None);
    /// ```
    pub fn from_alpn(alpn: &[u8]) -> Option<Self> {
        Self::SUPPORTED.iter().copied().find(|v| v.alpn() == alpn)
    }

    /// Number in the ALPN identifier (`ferry/<n>`).
    pub const fn number(self) -> u32 {
        match self {
            Self::V1 => 1,
        }
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ferry/{}", self.number())
    }
}

/// ALPN identifiers a listening node accepts, newest first (the order is the
/// preference: the accepting side decides).
pub fn accept_alpns() -> Vec<Vec<u8>> {
    ProtocolVersion::SUPPORTED
        .iter()
        .map(|v| v.alpn().to_vec())
        .collect()
}

/// Main and additional ALPN identifiers a connecting node offers: the newest
/// version first, all older ones after it.
pub fn offer_alpns() -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut all = accept_alpns().into_iter();
    // `SUPPORTED` is never empty.
    let newest = all
        .next()
        .unwrap_or_else(|| ProtocolVersion::CURRENT.alpn().to_vec());
    (newest, all.collect())
}

/// Version `conn` agreed on; `None` if the peer ended up with an identifier
/// that is not one of ours (which the handshake prevents).
pub fn negotiated(conn: &Connection) -> Option<ProtocolVersion> {
    ProtocolVersion::from_alpn(conn.alpn())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_listed_newest_first_and_round_trip() {
        let supported = ProtocolVersion::SUPPORTED;
        assert!(!supported.is_empty());
        assert!(supported.windows(2).all(|w| w[0] > w[1]), "newest first");
        assert_eq!(supported[0], ProtocolVersion::CURRENT);
        for v in supported {
            assert_eq!(ProtocolVersion::from_alpn(v.alpn()), Some(*v));
            assert_eq!(v.alpn(), format!("ferry/{}", v.number()).as_bytes());
            assert_eq!(v.to_string().as_bytes(), v.alpn());
        }
    }

    #[test]
    fn offers_the_newest_version_first() {
        let (main, rest) = offer_alpns();
        assert_eq!(main, ProtocolVersion::CURRENT.alpn());
        assert_eq!(rest.len() + 1, accept_alpns().len());
    }
}
