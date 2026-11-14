//! HTTPS to a LocalSend app: one request per connection, the certificate
//! pinned to the fingerprint the app announced, and the request bodies.

use super::devices::LocalSendDevice;
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use rust_i18n::t;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{
    CryptoProvider, ring, verify_tls12_signature, verify_tls13_signature,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme,
};

/// Time to open a connection (TCP and TLS).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A request body.
pub(super) type Body = UnsyncBoxBody<Bytes, std::io::Error>;

/// An empty body.
pub(super) fn empty() -> Body {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// `value` as a JSON body.
pub(super) fn json_body<T: serde::Serialize>(value: &T) -> Result<Body> {
    Ok(Full::new(Bytes::from(serde_json::to_vec(value)?))
        .map_err(|never| match never {})
        .boxed_unsync())
}

/// Percent-encodes a query value (everything but unreserved characters).
pub(super) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A connection target with its pinned certificate.
pub(super) struct Client {
    /// The device.
    device: LocalSendDevice,
    /// PIN for apps that require one.
    pin: Option<String>,
    /// Certificate presented to apps that ask the sender for one.
    identity: Option<Arc<crate::localsend::tls::Identity>>,
}

/// A byte stream: TLS or plain TCP.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

impl Client {
    /// Requests to `device`.
    pub(super) fn new(
        device: &LocalSendDevice,
        pin: Option<&str>,
        identity: Option<Arc<crate::localsend::tls::Identity>>,
    ) -> Self {
        Self {
            device: device.clone(),
            pin: pin.map(str::to_string),
            identity,
        }
    }

    /// Opens a connection; returns it and, over HTTPS, the certificate
    /// fingerprint it presented.
    async fn connect(&self) -> Result<(Box<dyn Io>, Option<Arc<Pinned>>)> {
        let tcp = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect(self.device.address),
        )
        .await
        .map_err(|_| anyhow::anyhow!(t!("localsend.unreachable", address = self.device.address)))?
        .with_context(|| t!("localsend.unreachable", address = self.device.address))?;
        if !self.device.https {
            return Ok((Box::new(tcp), None));
        }
        let provider = ring::default_provider();
        let verifier = Arc::new(Pinned {
            expected: (!self.device.fingerprint.is_empty())
                .then(|| self.device.fingerprint.clone()),
            seen: Mutex::new(None),
            provider: provider.clone(),
        });
        let builder = ClientConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone());
        // Newer LocalSend apps require the sender's certificate (mutual TLS).
        let config = match &self.identity {
            Some(identity) => {
                let (chain, key) = identity.client_auth();
                builder.with_client_auth_cert(chain, key)?
            }
            None => builder.with_no_client_auth(),
        };
        let tls = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TlsConnector::from(Arc::new(config))
                .connect(ServerName::IpAddress(self.device.address.ip().into()), tcp),
        )
        .await
        .map_err(|_| anyhow::anyhow!(t!("localsend.unreachable", address = self.device.address)))?
        .with_context(|| t!("localsend.wrong_certificate", name = self.device.label()))?;
        Ok((Box::new(tls), Some(verifier)))
    }

    /// One request on a new connection to `/api/localsend/v2/{route}`;
    /// returns the status, the body and the certificate fingerprint seen.
    pub(super) async fn call(
        &self,
        method: &str,
        route: &str,
        body: Body,
        length: Option<u64>,
    ) -> Result<(StatusCode, Bytes, Option<String>)> {
        let (io, verifier) = self.connect().await?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
        let mut route = format!("/api/localsend/v2/{route}");
        if let Some(pin) = &self.pin {
            route.push(if route.contains('?') { '&' } else { '?' });
            route.push_str(&format!("pin={}", encode(pin)));
        }
        let mut request = Request::builder()
            .method(method)
            .uri(route)
            .header("host", self.device.address.ip().to_string());
        request = match length {
            Some(length) => request
                .header("content-type", "application/octet-stream")
                .header("content-length", length),
            None => request.header("content-type", "application/json"),
        };
        let request = request.body(body)?;
        let work = async {
            let response = sender.send_request(request).await?;
            let status = response.status();
            let body = response.into_body().collect().await?.to_bytes();
            anyhow::Ok((status, body))
        };
        let mut work = std::pin::pin!(work);
        // The connection is driven here, not in another task, so a paused
        // transfer (a task-local switch) also pauses the upload body.
        let (status, body) = tokio::select! {
            result = &mut work => result?,
            closed = conn => {
                closed?;
                work.await?
            }
        };
        let seen = verifier.and_then(|v| v.seen.lock().ok().and_then(|s| s.clone()));
        Ok((status, body, seen))
    }
}

/// Accepts the server certificate only when its fingerprint is the announced
/// one (case-insensitive hex); without one, takes any and records it.
#[derive(Debug)]
struct Pinned {
    /// The announced fingerprint, if any.
    expected: Option<String>,
    /// The fingerprint of the certificate presented.
    seen: Mutex<Option<String>>,
    /// Signature algorithms.
    provider: CryptoProvider,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let fingerprint = crate::localsend::tls::fingerprint_of(cert.as_ref());
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(fingerprint.clone());
        }
        match &self.expected {
            Some(expected) if !expected.eq_ignore_ascii_case(&fingerprint) => Err(
                TlsError::General("the certificate is not the announced one".into()),
            ),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
