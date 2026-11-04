//! File transfers: the sending and receiving halves of the protocol, file name
//! security and the shared node service used by every user interface.

pub mod control;
pub mod hash;
pub mod receiver;
pub mod risk;
pub mod security;
pub mod sender;
pub mod service;
pub mod share;

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use rust_i18n::t;
use std::time::Duration;
use tracing::debug;

use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::Hello;

/// Progress callback, called with the number of bytes transferred so far.
pub type ProgressFn<'a> = &'a (dyn Fn(u64) + Send + Sync);

/// How long to wait for the peer to acknowledge the last frames before closing a stream.
const LINGER: Duration = Duration::from_secs(5);

/// Bidirectional control stream of a single transfer or pairing request.
pub struct Channel {
    /// Outgoing half.
    pub send: SendStream,
    /// Incoming half.
    pub recv: RecvStream,
}

impl Channel {
    /// Opens a control stream on the dialing side and sends `hello`.
    ///
    /// # Errors
    ///
    /// Fails when the stream cannot be opened or written to.
    pub async fn open(conn: &Connection, hello: &Hello) -> Result<Self> {
        let (mut send, recv) = conn.open_bi().await.with_context(|| t!("channel.open"))?;
        write_msg(&mut send, hello).await?;
        Ok(Self { send, recv })
    }

    /// Accepts the next control stream and reads its [`Hello`].
    ///
    /// Returns `Ok(None)` when the connection was closed by either side.
    ///
    /// # Errors
    ///
    /// Fails on unexpected connection errors or when the first frame is not a [`Hello`].
    pub async fn accept(conn: &Connection) -> Result<Option<(Self, Hello)>> {
        let (send, mut recv) = match conn.accept_bi().await {
            Ok(pair) => pair,
            Err(
                ConnectionError::ApplicationClosed(_)
                | ConnectionError::ConnectionClosed(_)
                | ConnectionError::LocallyClosed
                | ConnectionError::TimedOut,
            ) => return Ok(None),
            Err(e) => return Err(e).with_context(|| t!("channel.accept")),
        };
        let hello: Hello = read_msg(&mut recv).await?;
        Ok(Some((Self { send, recv }, hello)))
    }

    /// Finishes the outgoing half and waits a bounded time until the peer
    /// acknowledges all data.
    pub async fn finish(mut self) {
        if self.send.finish().is_ok()
            && tokio::time::timeout(LINGER, self.send.stopped())
                .await
                .is_err()
        {
            debug!("Timed out waiting for the peer to acknowledge stream shutdown");
        }
    }
}

/// Lowercase hexadecimal form of `bytes`.
pub(crate) fn short_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
