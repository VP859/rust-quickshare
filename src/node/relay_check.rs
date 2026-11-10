//! Checking a relay before relying on it ([`check_relay`]), for `ferry doctor`
//! and the relay settings of the GUI.
//!
//! Two steps, so a failure says what is wrong:
//!
//! 1. `GET /ping` over HTTP(S) – the probe path every iroh relay serves –
//!    with the root certificates iroh itself trusts for relays (Mozilla's,
//!    built in). This names the failing layer: the host name, the
//!    connection, the certificate or the answer. A second request on the same
//!    connection measures the round trip.
//! 2. A real relay connection, as in [`NetworkMode::RelayOnly`]: a throwaway
//!    identity without IP transport has to come online through the relay.
//!
//! [`NetworkMode::RelayOnly`]: super::NetworkMode::RelayOnly

use anyhow::{Context, Result};
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, RelayUrl, SecretKey, Watcher};
use rust_i18n::t;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::ring;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// How long each step of [`check_relay`] may take.
pub const RELAY_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// The probe path of iroh relays (`iroh_relay::http::RELAY_PROBE_PATH`).
const PROBE_PATH: &str = "/ping";

/// A plain or TLS connection to the relay.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Checks that `relay` works as a Ferry relay; returns the round-trip time of
/// an HTTP request to it.
///
/// # Errors
///
/// Fails with the step that failed and its reason: an unknown host name, a
/// refused or timed-out connection, a certificate that is not accepted, an
/// answer that is not from an iroh relay, or a relay connection that does not
/// come up within [`RELAY_CHECK_TIMEOUT`].
pub async fn check_relay(relay: &RelayUrl) -> Result<Duration> {
    let rtt = within(relay, probe(relay)).await?;
    within(relay, connect(relay)).await?;
    Ok(rtt)
}

/// `work`, or the "no answer" error after [`RELAY_CHECK_TIMEOUT`].
async fn within<T>(relay: &RelayUrl, work: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(RELAY_CHECK_TIMEOUT, work)
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!(t!("relay_check.no_answer", url = relay))))
}

/// Step 1: two `GET /ping` requests on one connection; the time of the second.
async fn probe(relay: &RelayUrl) -> Result<Duration> {
    let host = relay
        .host_str()
        .with_context(|| t!("engine.bad_relay", url = relay))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = relay
        .port_or_known_default()
        .with_context(|| t!("engine.bad_relay", url = relay))?;
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| t!("relay_check.unreachable", host = host, port = port))?;
    let io: Box<dyn Io> = if relay.scheme() == "https" {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = ServerName::try_from(host.clone())
            .with_context(|| t!("engine.bad_relay", url = relay))?;
        let tls = TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .with_context(|| t!("relay_check.certificate", host = host))?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .with_context(|| t!("relay_check.not_a_relay", url = relay))?;
    tokio::spawn(conn);
    let authority = relay
        .port()
        .map_or_else(|| host.clone(), |p| format!("{host}:{p}"));
    let mut rtt = Duration::ZERO;
    for _ in 0..2 {
        let request = Request::get(PROBE_PATH)
            .header("host", &authority)
            .body(Empty::<Bytes>::new())?;
        // The connection takes the next request once the previous one is done.
        sender
            .ready()
            .await
            .with_context(|| t!("relay_check.not_a_relay", url = relay))?;
        let started = Instant::now();
        let response = sender
            .send_request(request)
            .await
            .with_context(|| t!("relay_check.not_a_relay", url = relay))?;
        let status = response.status();
        response.into_body().collect().await?;
        rtt = started.elapsed();
        if !status.is_success() {
            anyhow::bail!(t!(
                "relay_check.status",
                url = relay,
                status = status.as_u16()
            ));
        }
    }
    Ok(rtt)
}

/// Step 2: a throwaway endpoint without IP transport comes online through `relay`.
async fn connect(relay: &RelayUrl) -> Result<()> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::custom([relay.clone()]))
        .clear_ip_transports()
        .secret_key(SecretKey::generate())
        .bind()
        .await
        .with_context(|| t!("engine.bind"))?;
    let online = tokio::time::timeout(RELAY_CHECK_TIMEOUT, endpoint.online()).await;
    let error = endpoint
        .home_relay_status()
        .get()
        .iter()
        .find_map(|s| s.last_error().map(ToString::to_string));
    endpoint.close().await;
    match (online, error) {
        (Ok(()), _) => Ok(()),
        (Err(_), Some(error)) => {
            Err(anyhow::anyhow!(error)).with_context(|| t!("relay_check.handshake", url = relay))
        }
        (Err(_), None) => anyhow::bail!(t!("relay_check.handshake", url = relay)),
    }
}
