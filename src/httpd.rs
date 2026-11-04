//! Small HTTP/1.1 server shared by the LocalSend compatibility mode (HTTPS)
//! and sharing through a web browser (plain HTTP): the accept loop with
//! connection limits and timeouts, saving an uploaded file safely, and JSON
//! and query-string helpers.

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rust_i18n::t;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::localsend::dto::OfferedFile;
use crate::transfer::receiver::check_free_space;
use crate::transfer::security::{
    resolve_secure_folder, resolve_secure_target_path, unique_destination,
};

/// Connections served at the same time.
const MAX_CONNECTIONS: usize = 64;
/// Time for the TLS handshake and for the request headers.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Progress is reported at most every this many bytes.
const PROGRESS_STEP: u64 = 512 * 1024;

/// Answers the requests of one server.
pub(crate) trait Handler: Send + Sync + 'static {
    /// Response body type.
    type Body: hyper::body::Body<Data = Bytes, Error: Into<Box<dyn std::error::Error + Send + Sync>>>
        + Send
        + 'static;

    /// Answers `req` from `ip`.
    fn handle(
        self: Arc<Self>,
        ip: IpAddr,
        req: Request<Incoming>,
    ) -> impl Future<Output = Response<Self::Body>> + Send;
}

/// Accepts connections until `stop` is cancelled; with `tls` over HTTPS.
pub(crate) async fn serve<S: Handler>(
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    server: Arc<S>,
    stop: CancellationToken,
) {
    let open = Arc::new(AtomicUsize::new(0));
    loop {
        let (tcp, addr) = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(e) => {
                    debug!("HTTP accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        if open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            debug!("HTTP: too many connections, dropping {addr}");
            continue;
        }
        open.fetch_add(1, Ordering::Relaxed);
        let (tls, server, stop, open) = (tls.clone(), server.clone(), stop.clone(), open.clone());
        tokio::spawn(async move {
            let ip = addr.ip().to_canonical();
            match tls {
                Some(acceptor) => {
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                        Ok(Ok(stream)) => connection(TokioIo::new(stream), ip, server, stop).await,
                        _ => debug!("TLS handshake with {ip} failed"),
                    }
                }
                None => connection(TokioIo::new(tcp), ip, server, stop).await,
            }
            open.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// Serves the requests of one connection.
async fn connection<S, I>(io: I, ip: IpAddr, server: Arc<S>, stop: CancellationToken)
where
    S: Handler,
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let service = service_fn(move |req| {
        let server = server.clone();
        async move { Ok::<_, Infallible>(server.handle(ip, req).await) }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HANDSHAKE_TIMEOUT)
        .serve_connection(io, service);
    tokio::select! {
        result = conn => if let Err(e) = result {
            debug!("HTTP connection from {ip} ended: {e}");
        },
        () = stop.cancelled() => {}
    }
}

/// Why an upload failed.
#[derive(Debug)]
pub(crate) enum UploadError {
    /// The content does not match the announced SHA-256.
    Checksum,
    /// Cancelled on this device or by the sender.
    Cancelled,
    /// Anything else.
    Failed(anyhow::Error),
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Checksum => f.write_str(&t!("receiver.hash_mismatch")),
            Self::Cancelled => f.write_str(&t!("transfer.cancelled")),
            Self::Failed(e) => write!(f, "{e:#}"),
        }
    }
}

/// Wraps any error as [`UploadError::Failed`].
fn fail(e: impl Into<anyhow::Error>) -> UploadError {
    UploadError::Failed(e.into())
}

/// Deletes a partial file unless the upload completed.
struct PartFile(Option<PathBuf>);

impl Drop for PartFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Writes the request body to a `.part` file next to the target, checks the
/// size and the SHA-256 (when announced) and moves it to a name that does not
/// overwrite anything. Returns the saved path.
///
/// # Errors
///
/// [`UploadError::Checksum`] for a wrong SHA-256, [`UploadError::Failed`] for
/// a wrong size, an unsafe name, a full disk or an I/O error; nothing is kept.
pub(crate) async fn receive(
    mut body: Incoming,
    file: &OfferedFile,
    dest_dir: &Path,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<PathBuf, UploadError> {
    let target_dir = resolve_secure_folder(dest_dir, &file.folder).map_err(fail)?;
    let final_target = resolve_secure_target_path(&target_dir, &file.name).map_err(fail)?;
    check_free_space(&target_dir, file.size).map_err(|r| fail(anyhow::anyhow!(r)))?;
    let random = uuid::Uuid::new_v4().simple().to_string();
    let part_name = format!("{}.{}.part", file.name, &random[..8]);
    let part_path = resolve_secure_target_path(&target_dir, &part_name).map_err(fail)?;
    let mut part = PartFile(Some(part_path.clone()));
    let out = tokio::fs::File::create(&part_path).await.map_err(fail)?;
    let mut out = BufWriter::with_capacity(1024 * 1024, out);
    let mut hasher = file.sha256.map(|_| Sha256::new());
    let (mut received, mut reported) = (0u64, 0u64);
    while let Some(frame) = body.frame().await {
        let Ok(data) = frame.map_err(fail)?.into_data() else {
            continue;
        };
        received += data.len() as u64;
        if received > file.size {
            return Err(fail(anyhow::anyhow!(
                "more data than the announced {} B",
                file.size
            )));
        }
        if let Some(h) = hasher.as_mut() {
            h.update(&data);
        }
        out.write_all(&data).await.map_err(fail)?;
        if received - reported >= PROGRESS_STEP {
            reported = received;
            progress(received);
        }
    }
    if received != file.size {
        return Err(fail(anyhow::anyhow!(
            "received {received} of {} B",
            file.size
        )));
    }
    out.flush().await.map_err(fail)?;
    out.into_inner().sync_all().await.map_err(fail)?;
    if let (Some(h), Some(expected)) = (hasher, file.sha256)
        && h.finalize().as_slice() != expected
    {
        return Err(UploadError::Checksum);
    }
    let final_path = unique_destination(&final_target);
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(fail)?;
    part.0 = None;
    crate::transfer::risk::mark_from_network(&final_path);
    Ok(final_path)
}

/// Reads a JSON body of at most `limit` bytes.
pub(crate) async fn read_json<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
    limit: usize,
) -> Result<T, StatusCode> {
    let bytes = Limited::new(req.into_body(), limit)
        .collect()
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)
}

/// A JSON response.
pub(crate) fn json<T: serde::Serialize>(status: StatusCode, body: &T) -> Response<Full<Bytes>> {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let mut res = Response::new(Full::new(Bytes::from(bytes)));
    *res.status_mut() = status;
    res.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    res
}

/// An error response `{"message": …}`.
pub(crate) fn message(status: StatusCode, text: &str) -> Response<Full<Bytes>> {
    json(status, &serde_json::json!({ "message": text }))
}

/// Decodes `a=b&c=d` with percent-encoding and `+` as space.
pub(crate) fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((percent_decode(k)?, percent_decode(v)?))
        })
        .collect()
}

/// Percent-decodes `s`; `None` when the result is not UTF-8.
pub(crate) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_decoded() {
        let q = parse_query("sessionId=abc&fileId=my%20file+1&token=t%C3%B3k&empty");
        assert_eq!(q["sessionId"], "abc");
        assert_eq!(q["fileId"], "my file 1");
        assert_eq!(q["token"], "tók");
        assert_eq!(q["empty"], "");
        assert_eq!(percent_decode("%"), Some("%".into()));
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(percent_decode("%ff"), None, "not UTF-8");
    }
}
