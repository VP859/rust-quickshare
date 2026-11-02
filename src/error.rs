//! Classifying errors returned by the library.
//!
//! Functions of this crate return [`anyhow::Result`]: errors carry a localized
//! chain of context that interfaces show as-is. Callers that need to react
//! differently to different failures do not have to parse that text –
//! [`kind`] maps any error to a stable [`ErrorKind`] by looking for the typed
//! errors below (and the ones of iroh, Tokio and the standard library) in its
//! chain, much like [`std::io::ErrorKind`].
//!
//! ```
//! use ferry::error::{ErrorKind, ProtocolViolation, kind};
//!
//! let error = anyhow::Error::new(ProtocolViolation("bad frame".into())).context("receiving");
//! assert_eq!(kind(&error), ErrorKind::Protocol);
//! assert_eq!(kind(&anyhow::anyhow!("something else")), ErrorKind::Other);
//! ```

use std::fmt;

use crate::transfer::security::SecurityError;
use crate::transfer::service::Cancelled;

/// Broad category of a failure, for deciding how to react to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The user cancelled the transfer ([`Cancelled`]).
    Cancelled,
    /// The other device could not be reached, or the connection broke.
    Connection,
    /// The other device did not answer in time.
    Timeout,
    /// The other device sent something the protocol does not allow
    /// ([`ProtocolViolation`]); the peer is faulty or hostile.
    Protocol,
    /// Received data did not match its BLAKE3 hash ([`IntegrityError`]).
    Integrity,
    /// The other device speaks none of the protocol versions of this one
    /// ([`IncompatibleVersion`]); one of them needs an update.
    Incompatible,
    /// A file or folder name from the other device was unsafe and refused
    /// ([`SecurityError`]).
    UnsafePath,
    /// There is not enough free disk space ([`NotEnoughSpace`]).
    DiskFull,
    /// A local file system error.
    Io,
    /// Anything else, e.g. invalid user input or configuration.
    Other,
}

/// The other device broke the protocol: an invalid message, size, order or
/// pairing commitment. The text is localized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolViolation(pub String);

/// Received data does not match its hash; nothing corrupted was kept. The text
/// is localized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityError(pub String);

/// The other device and this one share no protocol version. The text is
/// localized and tells the user to update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibleVersion(pub String);

/// A file does not fit on the receiving disk. The text is localized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotEnoughSpace(pub String);

/// Implements `Display` (the localized text) and `Error` for the message types.
macro_rules! message_error {
    ($($name:ident),*) => {$(
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::error::Error for $name {}
    )*};
}

message_error!(
    ProtocolViolation,
    IntegrityError,
    NotEnoughSpace,
    IncompatibleVersion
);

/// TLS alert `no_application_protocol` (120) as a QUIC transport error code
/// (`0x100` + alert): the peer supports none of the offered ALPN identifiers.
const NO_APPLICATION_PROTOCOL: u64 = 0x100 + 120;

/// Whether `error` is a connection that the peer refused because it speaks none
/// of our protocol versions.
pub fn is_no_common_protocol(error: &anyhow::Error) -> bool {
    use iroh::endpoint::ConnectionError;
    error.chain().any(|e| {
        matches!(
            e.downcast_ref::<ConnectionError>(),
            Some(ConnectionError::ConnectionClosed(close))
                if u64::from(close.error_code) == NO_APPLICATION_PROTOCOL
        )
    })
}

/// Category of `error`, decided by the first recognized error in its chain
/// (outermost first).
pub fn kind(error: &anyhow::Error) -> ErrorKind {
    error.chain().find_map(classify).unwrap_or(ErrorKind::Other)
}

/// Category of one error of a chain, if it is a known type.
fn classify(error: &(dyn std::error::Error + 'static)) -> Option<ErrorKind> {
    use iroh::endpoint::{
        ConnectError, ConnectWithOptsError, ConnectionError, ReadError, ReadExactError, WriteError,
    };

    let typed = [
        (error.is::<Cancelled>(), ErrorKind::Cancelled),
        (error.is::<IncompatibleVersion>(), ErrorKind::Incompatible),
        (error.is::<ProtocolViolation>(), ErrorKind::Protocol),
        (error.is::<IntegrityError>(), ErrorKind::Integrity),
        (error.is::<NotEnoughSpace>(), ErrorKind::DiskFull),
        (error.is::<SecurityError>(), ErrorKind::UnsafePath),
        (
            error.is::<tokio::time::error::Elapsed>(),
            ErrorKind::Timeout,
        ),
        (
            error.is::<ConnectError>()
                || error.is::<ConnectWithOptsError>()
                || error.is::<ConnectionError>()
                || error.is::<ReadError>()
                || error.is::<ReadExactError>()
                || error.is::<WriteError>(),
            ErrorKind::Connection,
        ),
    ];
    if let Some((_, kind)) = typed.into_iter().find(|(matches, _)| *matches) {
        return Some(kind);
    }
    let io = error.downcast_ref::<std::io::Error>()?;
    // QUIC streams read through `tokio::io` report a broken connection as an
    // `io::Error` wrapping the QUIC error.
    if let Some(kind) = io.get_ref().and_then(|inner| classify(inner)) {
        return Some(kind);
    }
    Some(match io.kind() {
        std::io::ErrorKind::StorageFull => ErrorKind::DiskFull,
        std::io::ErrorKind::TimedOut => ErrorKind::Timeout,
        std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::BrokenPipe => ErrorKind::Connection,
        _ => ErrorKind::Io,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn finds_the_typed_error_in_the_chain() {
        let wrapped = |e: anyhow::Error| e.context("outer").context("outermost");
        assert_eq!(kind(&wrapped(Cancelled.into())), ErrorKind::Cancelled);
        assert_eq!(
            kind(&wrapped(IntegrityError("x".into()).into())),
            ErrorKind::Integrity
        );
        assert_eq!(
            kind(&wrapped(NotEnoughSpace("x".into()).into())),
            ErrorKind::DiskFull
        );
        assert_eq!(
            kind(&wrapped(SecurityError::InvalidFileName.into())),
            ErrorKind::UnsafePath
        );
        let io: anyhow::Result<()> =
            Err(std::io::Error::from(std::io::ErrorKind::NotFound)).context("reading");
        assert_eq!(kind(&io.unwrap_err()), ErrorKind::Io);
        assert_eq!(kind(&anyhow::anyhow!("plain")), ErrorKind::Other);
        // A QUIC read error seen through tokio::io, as when a connection broke.
        let lost = std::io::Error::other(iroh::endpoint::ReadError::ClosedStream);
        let read: anyhow::Result<()> = Err(lost).context("codec.read");
        assert_eq!(kind(&read.unwrap_err()), ErrorKind::Connection);
    }

    #[test]
    fn displays_the_message() {
        assert_eq!(ProtocolViolation("bad".into()).to_string(), "bad");
    }
}
