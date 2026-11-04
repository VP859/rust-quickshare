//! Response bodies of the web server: fixed bytes, or a file streamed by a
//! task through a channel.

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Frame, SizeHint};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

/// Body of every web response.
pub(super) type Body = UnsyncBoxBody<Bytes, std::io::Error>;

/// A body with fixed content.
pub(super) fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// Converts a response of the shared helpers.
pub(super) fn boxed(res: hyper::Response<Full<Bytes>>) -> hyper::Response<Body> {
    res.map(|b| b.map_err(|never| match never {}).boxed_unsync())
}

/// A body of `len` bytes fed by a task; an `Err` from the task aborts it.
pub(super) struct Channel {
    /// Chunks from the task.
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
    /// Announced length (`Content-Length`).
    len: u64,
}

impl Channel {
    /// A body of `len` bytes and the sender that fills it.
    pub(super) fn open(len: u64) -> (mpsc::Sender<Result<Bytes, std::io::Error>>, Body) {
        let (tx, rx) = mpsc::channel(4);
        (tx, Self { rx, len }.boxed_unsync())
    }
}

impl hyper::body::Body for Channel {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
        self.rx
            .poll_recv(cx)
            .map(|chunk| chunk.map(|r| r.map(Frame::data)))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.len)
    }
}
