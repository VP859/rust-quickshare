//! LocalSend compatibility mode (`ferry::localsend`) against a client written
//! from the LocalSend protocol v2 specification.
//!
//! The client behaves like the official one (packages/core of
//! github.com/localsend/localsend): HTTPS with the server certificate pinned
//! to the announced SHA-256 fingerprint, `prepare-upload`, one `upload` per
//! accepted file, `cancel`.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use bytes::Bytes;
use ferry::localsend::{LocalSend, LocalSendOffer, LocalSendOptions};
use ferry::node::NodeEngine;
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::risk::Risk;
use ferry::transfer::service::{OfferDecision, TransferEvent, TransferHandler, TransferStatus};
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use iroh::{EndpointId, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

// ---------------------------------------------------------------------------
// The receiving interface
// ---------------------------------------------------------------------------

/// What the test interface answers to LocalSend offers.
#[derive(Clone)]
enum Answer {
    Accept(PathBuf),
    Reject,
}

struct Interface {
    answer: Mutex<Answer>,
    offers: Mutex<Vec<LocalSendOffer>>,
    events: Mutex<Vec<TransferEvent>>,
}

impl Interface {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            offers: Mutex::default(),
            events: Mutex::default(),
        })
    }

    fn finals(&self) -> Vec<TransferEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.status.is_final())
            .cloned()
            .collect()
    }
}

impl TransferHandler for Interface {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        unreachable!("only LocalSend offers in these tests")
    }

    fn on_event(&self, event: TransferEvent) {
        self.events.lock().unwrap().push(event);
    }

    fn device_name(&self) -> String {
        "Ferry Laptop".into()
    }

    async fn decide_localsend(&self, offer: &LocalSendOffer) -> OfferDecision {
        self.offers.lock().unwrap().push(offer.clone());
        match self.answer.lock().unwrap().clone() {
            Answer::Accept(dest_dir) => OfferDecision::Accept { dest_dir },
            Answer::Reject => OfferDecision::Reject {
                reason: "no".into(),
            },
        }
    }
}

/// A handler that keeps the default `decide_localsend`.
struct DefaultInterface;

impl TransferHandler for DefaultInterface {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        unreachable!()
    }
    fn on_event(&self, _: TransferEvent) {}
}

struct Setup {
    server: LocalSend,
    _engine: NodeEngine,
    _cert: tempfile::TempDir,
}

async fn start<H: TransferHandler>(handler: Arc<H>) -> Setup {
    let engine = NodeEngine::new(SecretKey::generate()).await.unwrap();
    let cert = tempfile::tempdir().unwrap();
    let options = LocalSendOptions {
        bind: Ipv4Addr::LOCALHOST.into(),
        port: 0,
        multicast: false,
        cert_dir: Some(cert.path().to_path_buf()),
    };
    let server = LocalSend::start(&engine, options, handler).await.unwrap();
    Setup {
        server,
        _engine: engine,
        _cert: cert,
    }
}

// ---------------------------------------------------------------------------
// The client, from the specification
// ---------------------------------------------------------------------------

/// Accepts the server certificate only when its SHA-256 (uppercase hex) is
/// the pinned one, like the official client; records what it saw.
#[derive(Debug)]
struct Pinned {
    expected: Option<String>,
    seen: Mutex<Option<String>>,
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
        let fingerprint: String = Sha256::digest(cert.as_ref())
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect();
        *self.seen.lock().unwrap() = Some(fingerprint.clone());
        match &self.expected {
            Some(expected) if *expected != fingerprint => {
                Err(TlsError::General("fingerprint mismatch".into()))
            }
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

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
    fingerprint: Option<String>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// One HTTPS request on a new connection; `pin` as in the official client.
async fn call(
    port: u16,
    pin: Option<&str>,
    method: &str,
    path: &str,
    body: impl Into<Bytes>,
) -> anyhow::Result<Reply> {
    let provider = ring::default_provider();
    let verifier = Arc::new(Pinned {
        expected: pin.map(str::to_string),
        seen: Mutex::new(None),
        provider: provider.clone(),
    });
    let config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let tcp = tokio::net::TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    let tls = TlsConnector::from(Arc::new(config))
        .connect(ServerName::IpAddress(Ipv4Addr::LOCALHOST.into()), tcp)
        .await?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await?;
    tokio::spawn(conn);
    let request = Request::builder()
        .method(method)
        .uri(format!("/api/localsend/v2/{path}"))
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Full::new(body.into()))?;
    let response = sender.send_request(request).await?;
    let status = response.status();
    let body = response.into_body().collect().await?.to_bytes().to_vec();
    let fingerprint = verifier.seen.lock().unwrap().clone();
    Ok(Reply {
        status,
        body,
        fingerprint,
    })
}

fn sender_info() -> Value {
    json!({
        "alias": "Kate's iPhone", "version": "2.1", "deviceModel": "iPhone",
        "deviceType": "mobile", "fingerprint": "SENDER", "port": 53317,
        "protocol": "https", "download": false
    })
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `prepare-upload` for `files` = (id, fileName, content, send the sha256).
async fn offer(port: u16, pin: &str, files: &[(&str, &str, &[u8], bool)]) -> Reply {
    let files: serde_json::Map<String, Value> = files
        .iter()
        .map(|(id, name, data, hash)| {
            let mut file = json!({
                "id": id, "fileName": name, "size": data.len(),
                "fileType": "application/octet-stream", "preview": null,
                "metadata": { "modified": "2026-01-01T12:00:00Z" }
            });
            if *hash {
                file["sha256"] = json!(sha256_hex(data));
            }
            ((*id).to_string(), file)
        })
        .collect();
    let body = json!({ "info": sender_info(), "files": files });
    call(port, Some(pin), "POST", "prepare-upload", body.to_string())
        .await
        .unwrap()
}

async fn upload(
    port: u16,
    pin: &str,
    session: &str,
    id: &str,
    token: &str,
    data: &[u8],
) -> StatusCode {
    let path = format!("upload?sessionId={session}&fileId={id}&token={token}");
    call(port, Some(pin), "POST", &path, data.to_vec())
        .await
        .unwrap()
        .status
}

fn tree(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in walk(dir) {
        out.push(
            entry
                .strip_prefix(dir)
                .unwrap()
                .display()
                .to_string()
                .replace('\\', "/"),
        );
    }
    out.sort();
    out
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn discovery_answers_carry_the_certificate_fingerprint() {
    let s = start(Interface::new(Answer::Reject)).await;
    let port = s.server.port();
    let fingerprint = s.server.fingerprint().to_string();

    // Discovery: the official client registers without a pin and takes the
    // fingerprint from the certificate.
    let reply = call(port, None, "POST", "register", sender_info().to_string())
        .await
        .unwrap();
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.fingerprint.as_deref(), Some(fingerprint.as_str()));
    let info = reply.json();
    assert_eq!(info["alias"], "Ferry Laptop");
    assert_eq!(info["fingerprint"], fingerprint.as_str());
    assert_eq!(info["deviceModel"], "Ferry");
    assert!(info["version"].as_str().unwrap().starts_with("2."));

    let info = call(port, Some(&fingerprint), "GET", "info", "")
        .await
        .unwrap();
    assert_eq!(info.status, StatusCode::OK);
    assert_eq!(info.json()["alias"], "Ferry Laptop");

    // A client pinned to another fingerprint refuses to talk.
    assert!(call(port, Some("00FF"), "GET", "info", "").await.is_err());
    // Unknown routes and bad bodies.
    assert_eq!(
        call(port, None, "GET", "nothing", "").await.unwrap().status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(port, None, "POST", "register", "{")
            .await
            .unwrap()
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn files_and_folders_arrive_after_the_user_accepts() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let s = start(ui.clone()).await;
    let (port, pin) = (s.server.port(), s.server.fingerprint().to_string());

    let photo: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let note = b"hello from an iPhone".to_vec();
    let empty: Vec<u8> = Vec::new();
    let reply = offer(
        port,
        &pin,
        &[
            ("p1", "Album/2026/photo.jpg", &photo, true),
            ("n1", "note.txt", &note, false),
            ("e1", "empty.bin", &empty, true),
        ],
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let answer = reply.json();
    let session = answer["sessionId"].as_str().unwrap();
    let tokens = answer["files"].as_object().unwrap();
    assert_eq!(tokens.len(), 3);

    {
        let offers = ui.offers.lock().unwrap();
        let offer = &offers[0];
        assert_eq!(offer.sender.alias, "Kate's iPhone");
        assert_eq!(offer.sender.label(), "Kate's iPhone (iPhone)");
        assert_eq!(offer.count, 3);
        assert_eq!(offer.total_size, (photo.len() + note.len()) as u64);
        assert!(offer.names.contains(&"Album/2026/photo.jpg".to_string()));
        assert_eq!(offer.risk, None);
    }

    for (id, data) in [("p1", &photo), ("n1", &note), ("e1", &empty)] {
        let token = tokens[id].as_str().unwrap();
        assert_eq!(
            upload(port, &pin, session, id, token, data).await,
            StatusCode::OK,
            "{id}"
        );
    }

    assert_eq!(
        tree(dest.path()),
        ["Album/2026/photo.jpg", "empty.bin", "note.txt"]
    );
    assert_eq!(
        std::fs::read(dest.path().join("Album/2026/photo.jpg")).unwrap(),
        photo
    );
    assert_eq!(std::fs::read(dest.path().join("note.txt")).unwrap(), note);

    let finals = ui.finals();
    assert_eq!(finals.len(), 3);
    for event in &finals {
        assert!(
            matches!(event.status, TransferStatus::Completed { .. }),
            "{event:?}"
        );
        assert_eq!(event.peer, "localsend:127.0.0.1");
        assert_eq!(event.peer_name.as_deref(), Some("Kate's iPhone (iPhone)"));
    }

    // The session ended with its last file: a new offer is welcome.
    let again = offer(port, &pin, &[("x", "x.txt", b"x", false)]).await;
    assert_eq!(again.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_offer_saves_nothing_and_the_sender_must_wait() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Reject);
    let s = start(ui.clone()).await;
    let (port, pin) = (s.server.port(), s.server.fingerprint().to_string());

    let reply = offer(port, &pin, &[("a", "a.txt", b"data", false)]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let reply = offer(port, &pin, &[("a", "a.txt", b"data", false)]).await;
    assert_eq!(
        reply.status,
        StatusCode::TOO_MANY_REQUESTS,
        "no flood of dialogs"
    );
    assert_eq!(ui.offers.lock().unwrap().len(), 1);
    assert!(tree(dest.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_interface_that_asks_every_offer_is_declined() {
    let s = start(Arc::new(DefaultInterface)).await;
    let reply = offer(
        s.server.port(),
        s.server.fingerprint(),
        &[("a", "a.txt", b"data", false)],
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_need_the_session_and_the_token() {
    let dest = tempfile::tempdir().unwrap();
    let s = start(Interface::new(Answer::Accept(dest.path().to_path_buf()))).await;
    let (port, pin) = (s.server.port(), s.server.fingerprint().to_string());
    let reply = offer(
        port,
        &pin,
        &[
            ("a", "a.txt", b"data", false),
            ("b", "b.txt", b"more", false),
        ],
    )
    .await;
    let answer = reply.json();
    let session = answer["sessionId"].as_str().unwrap();
    let token = answer["files"]["a"].as_str().unwrap();

    // Another offer while the session runs.
    let busy = offer(port, &pin, &[("c", "c.txt", b"c", false)]).await;
    assert_eq!(busy.status, StatusCode::CONFLICT);

    assert_eq!(
        upload(port, &pin, session, "a", "wrong", b"data").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upload(port, &pin, "other", "a", token, b"data").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upload(port, &pin, session, "b", token, b"more").await,
        StatusCode::FORBIDDEN,
        "token of another file"
    );
    let missing = call(port, Some(&pin), "POST", "upload?sessionId=x", "")
        .await
        .unwrap();
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert!(tree(dest.path()).is_empty());

    assert_eq!(
        upload(port, &pin, session, "a", token, b"data").await,
        StatusCode::OK
    );
    // A token is good for one upload.
    assert_eq!(
        upload(port, &pin, session, "a", token, b"data").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(tree(dest.path()), ["a.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_content_is_not_kept() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let s = start(ui.clone()).await;
    let (port, pin) = (s.server.port(), s.server.fingerprint().to_string());
    let data = b"the real content".to_vec();
    let reply = offer(
        port,
        &pin,
        &[
            ("h", "hashed.txt", &data, true),
            ("s", "sized.txt", &data, false),
        ],
    )
    .await;
    let answer = reply.json();
    let session = answer["sessionId"].as_str().unwrap();
    let token = |id: &str| answer["files"][id].as_str().unwrap().to_string();

    // Same size, other bytes: the checksum catches it (422, as specified).
    let forged = b"the fake content".to_vec();
    assert_eq!(forged.len(), data.len());
    assert_eq!(
        upload(port, &pin, session, "h", &token("h"), &forged).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // More bytes than announced.
    let longer = [data.as_slice(), b"!!"].concat();
    assert_eq!(
        upload(port, &pin, session, "s", &token("s"), &longer).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(
        tree(dest.path()).is_empty(),
        "no file and no .part left: {:?}",
        tree(dest.path())
    );
    assert!(
        ui.finals()
            .iter()
            .all(|e| matches!(e.status, TransferStatus::Failed { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dangerous_names_are_refused_or_flagged() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let s = start(ui.clone()).await;
    let (port, pin) = (s.server.port(), s.server.fingerprint().to_string());

    for name in ["../../.bashrc", "/etc/passwd", "a/../../x"] {
        let reply = offer(port, &pin, &[("x", name, b"x", false)]).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{name}");
    }
    assert!(
        ui.offers.lock().unwrap().is_empty(),
        "the user is not even asked"
    );

    // A program among many photos: the warning covers every file, not only
    // the listed names.
    let mut files: Vec<(String, String)> = (0..30)
        .map(|i| (format!("p{i}"), format!("photo{i:02}.jpg")))
        .collect();
    files.push(("z".into(), "zz-update.apk".into()));
    let files: Vec<(&str, &str, &[u8], bool)> = files
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str(), b"x".as_slice(), false))
        .collect();
    assert_eq!(offer(port, &pin, &files).await.status, StatusCode::OK);
    let offers = ui.offers.lock().unwrap();
    assert_eq!(offers[0].names.len(), 20);
    assert!(!offers[0].names.iter().any(|n| n.ends_with(".apk")));
    assert_eq!(offers[0].risk, Some(Risk::Program));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sender_can_cancel_a_session() {
    let dest = tempfile::tempdir().unwrap();
    let s = start(Interface::new(Answer::Accept(dest.path().to_path_buf()))).await;
    let (port, pin) = (s.server.port(), s.server.fingerprint().to_string());
    let reply = offer(port, &pin, &[("a", "a.txt", b"data", false)]).await;
    let answer = reply.json();
    let session = answer["sessionId"].as_str().unwrap();
    let token = answer["files"]["a"].as_str().unwrap();

    let cancel = call(
        port,
        Some(&pin),
        "POST",
        &format!("cancel?sessionId={session}"),
        "",
    )
    .await
    .unwrap();
    assert_eq!(cancel.status, StatusCode::OK);
    assert_eq!(
        upload(port, &pin, session, "a", token, b"data").await,
        StatusCode::FORBIDDEN
    );
    // The slot is free again.
    assert_eq!(
        offer(port, &pin, &[("b", "b.txt", b"b", false)])
            .await
            .status,
        StatusCode::OK
    );
    assert!(tree(dest.path()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_certificate_stays_the_same_after_a_restart() {
    let engine = NodeEngine::new(SecretKey::generate()).await.unwrap();
    let cert = tempfile::tempdir().unwrap();
    let options = || LocalSendOptions {
        bind: Ipv4Addr::LOCALHOST.into(),
        port: 0,
        multicast: false,
        cert_dir: Some(cert.path().to_path_buf()),
    };
    let first = LocalSend::start(&engine, options(), Arc::new(DefaultInterface))
        .await
        .unwrap();
    let fingerprint = first.fingerprint().to_string();
    first.stopped().await;
    let second = LocalSend::start(&engine, options(), Arc::new(DefaultInterface))
        .await
        .unwrap();
    assert_eq!(second.fingerprint(), fingerprint, "LocalSend apps pin it");
    // The pinned client still connects.
    let reply = call(second.port(), Some(&fingerprint), "GET", "info", "")
        .await
        .unwrap();
    assert_eq!(reply.status, StatusCode::OK);
}

/// A LocalSend app on the same computer hears the announcements and gets an
/// answer to its own. Needs multicast on the machine; CI runners may not
/// deliver it, so it runs with `--include-ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs multicast delivery between sockets of this computer"]
async fn announcements_reach_a_localsend_app_on_the_same_computer() {
    use std::time::Duration;
    use tokio::net::UdpSocket;

    // The "app": a socket on the LocalSend port in the LocalSend group.
    let app = {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None).unwrap();
        socket.set_reuse_address(true).unwrap();
        #[cfg(unix)]
        socket.set_reuse_port(true).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket
            .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, ferry::localsend::PORT)).into())
            .unwrap();
        let socket = UdpSocket::from_std(socket.into()).unwrap();
        for iface in [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOCALHOST] {
            let _ = socket.join_multicast_v4(ferry::localsend::MULTICAST_GROUP, iface);
        }
        socket.set_multicast_loop_v4(true).unwrap();
        socket
    };

    let engine = NodeEngine::new(SecretKey::generate()).await.unwrap();
    let cert = tempfile::tempdir().unwrap();
    let options = LocalSendOptions {
        bind: Ipv4Addr::LOCALHOST.into(),
        port: 0,
        multicast: true,
        cert_dir: Some(cert.path().to_path_buf()),
    };
    let server = LocalSend::start(&engine, options, Interface::new(Answer::Reject))
        .await
        .unwrap();

    // Messages from this Ferry, ignoring anything else on the network.
    let next_from_ferry = |announce: bool| {
        let app = &app;
        let fingerprint = server.fingerprint().to_string();
        async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let (len, _) = app.recv_from(&mut buf).await.unwrap();
                let Ok(message) = serde_json::from_slice::<Value>(&buf[..len]) else {
                    continue;
                };
                if message["fingerprint"] == fingerprint.as_str() && message["announce"] == announce
                {
                    return message;
                }
            }
        }
    };
    let announcement = tokio::time::timeout(Duration::from_secs(5), next_from_ferry(true))
        .await
        .expect("Ferry announces itself");
    assert_eq!(announcement["alias"], "Ferry Laptop");
    assert_eq!(announcement["port"], server.port());
    assert_eq!(announcement["protocol"], "https");

    // The app announces itself; Ferry answers (and the app would register).
    let hello = json!({
        "alias": "Kate's iPhone", "version": "2.1", "deviceModel": "iPhone", "deviceType": "mobile",
        "fingerprint": "SENDER", "port": 53317, "protocol": "https", "download": false, "announce": true
    });
    let group = SocketAddr::from((ferry::localsend::MULTICAST_GROUP, ferry::localsend::PORT));
    let answer = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let _ = socket2::SockRef::from(&app).set_multicast_if_v4(&Ipv4Addr::LOCALHOST);
            app.send_to(hello.to_string().as_bytes(), group)
                .await
                .unwrap();
            if let Ok(answer) =
                tokio::time::timeout(Duration::from_millis(800), next_from_ferry(false)).await
            {
                return answer;
            }
        }
    })
    .await
    .expect("Ferry answers an announcement");
    assert_eq!(answer["port"], server.port());
}

/// The independent Python client (`scripts/localsend-client.py`, written from
/// the specification only) sends a folder and files. Needs `python3`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs python3; CI runs it in the protocol job"]
async fn the_independent_python_client_sends_files() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let s = start(ui.clone()).await;

    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("Trip/day 1")).unwrap();
    std::fs::write(src.path().join("Trip/day 1/a.jpg"), vec![7u8; 300_000]).unwrap();
    std::fs::write(src.path().join("Trip/b.txt"), b"bbb").unwrap();
    std::fs::write(src.path().join("single.pdf"), b"%PDF").unwrap();

    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/localsend-client.py");
    let output = tokio::process::Command::new("python3")
        .arg("-I")
        .arg(&script)
        .args(["send", "127.0.0.1", "--port", &s.server.port().to_string()])
        .arg(src.path().join("Trip"))
        .arg(src.path().join("single.pdf"))
        .output()
        .await
        .unwrap();
    let log = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{log}");
    assert!(
        log.contains(s.server.fingerprint()),
        "the client saw and pinned the fingerprint: {log}"
    );
    assert_eq!(
        tree(dest.path()),
        ["Trip/b.txt", "Trip/day 1/a.jpg", "single.pdf"]
    );
    assert_eq!(
        std::fs::read(dest.path().join("Trip/day 1/a.jpg")).unwrap(),
        vec![7u8; 300_000]
    );
    assert_eq!(
        ui.offers.lock().unwrap()[0].sender.label(),
        format!("Test client ({}) (Python)", gethostname())
    );
}

fn gethostname() -> String {
    let out = std::process::Command::new("python3")
        .args(["-I", "-c", "import socket; print(socket.gethostname())"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// Sending to a LocalSend app (ferry::localsend::client)
// ---------------------------------------------------------------------------

/// The receiving side is Ferry's own LocalSend server, which the tests above
/// check against the specification; here it stands for a LocalSend app.
fn localsend_device(setup: &Setup, fingerprint: &str) -> ferry::localsend::client::LocalSendDevice {
    ferry::localsend::client::LocalSendDevice {
        alias: "Kate's iPhone".into(),
        model: Some("iPhone".into()),
        device_type: Some("mobile".into()),
        fingerprint: fingerprint.into(),
        address: SocketAddr::from((Ipv4Addr::LOCALHOST, setup.server.port())),
        https: true,
    }
}

fn own_device(dir: &Path) -> ferry::localsend::client::OwnDevice {
    ferry::localsend::client::OwnDevice {
        name: "Ferry Laptop".into(),
        cert_dir: Some(dir.to_path_buf()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn files_and_folders_are_sent_to_a_localsend_app() {
    use ferry::localsend::client::{SendResult, send};
    let dest = tempfile::tempdir().unwrap();
    let interface = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let setup = start(interface.clone()).await;
    let device = localsend_device(&setup, setup.server.fingerprint());

    let src = tempfile::tempdir().unwrap();
    let photo = src.path().join("photo.jpg");
    std::fs::write(&photo, vec![7u8; 700_000]).unwrap();
    let folder = src.path().join("Trip");
    std::fs::create_dir_all(folder.join("day1")).unwrap();
    std::fs::write(folder.join("day1/a.txt"), b"hello").unwrap();
    let mine = tempfile::tempdir().unwrap();

    let events = Mutex::new(Vec::new());
    let result = send(
        &device,
        &[photo, folder],
        &own_device(mine.path()),
        None,
        &|e| events.lock().unwrap().push(e),
    )
    .await
    .unwrap();
    assert_eq!(result, SendResult::Sent);
    assert_eq!(
        std::fs::read(dest.path().join("photo.jpg")).unwrap().len(),
        700_000
    );
    assert_eq!(
        std::fs::read(dest.path().join("Trip/day1/a.txt")).unwrap(),
        b"hello"
    );

    let offer = interface.offers.lock().unwrap()[0].clone();
    assert_eq!(offer.sender.alias, "Ferry Laptop");
    assert_eq!(offer.count, 2);
    let events = events.into_inner().unwrap();
    let finals: Vec<_> = events.iter().filter(|e| e.status.is_final()).collect();
    assert_eq!(finals.len(), 2);
    assert!(
        finals
            .iter()
            .all(|e| matches!(e.status, TransferStatus::Completed { .. })
                && e.peer == "localsend:127.0.0.1"
                && e.peer_name.as_deref() == Some("Kate's iPhone (iPhone)"))
    );
    assert!(
        events
            .iter()
            .any(|e| e.status == TransferStatus::AwaitingDecision)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_offer_is_reported_and_nothing_is_uploaded() {
    use ferry::localsend::client::{SendResult, send};
    let interface = Interface::new(Answer::Reject);
    let setup = start(interface.clone()).await;
    let device = localsend_device(&setup, setup.server.fingerprint());
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.bin");
    std::fs::write(&file, b"x").unwrap();
    let mine = tempfile::tempdir().unwrap();
    let events = Mutex::new(Vec::new());
    let result = send(&device, &[file], &own_device(mine.path()), None, &|e| {
        events.lock().unwrap().push(e)
    })
    .await
    .unwrap();
    assert_eq!(result, SendResult::Rejected);
    assert!(matches!(
        events.lock().unwrap().last().map(|e| &e.status),
        Some(TransferStatus::Rejected { .. })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_with_another_certificate_than_announced_gets_nothing() {
    use ferry::localsend::client::send;
    let dest = tempfile::tempdir().unwrap();
    let interface = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let setup = start(interface.clone()).await;
    // An impostor announced the name with its own fingerprint.
    let device = localsend_device(&setup, &"AB".repeat(32));
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("secret.pdf");
    std::fs::write(&file, b"secret").unwrap();
    let mine = tempfile::tempdir().unwrap();
    let error = send(&device, &[file], &own_device(mine.path()), None, &|_| {})
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("certificate"), "{error:#}");
    assert!(
        interface.offers.lock().unwrap().is_empty(),
        "no offer reached it"
    );
    assert!(std::fs::read_dir(dest.path()).unwrap().next().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_localsend_app_is_found_by_its_address() {
    let setup = start(Interface::new(Answer::Reject)).await;
    let mine = tempfile::tempdir().unwrap();
    let device = ferry::localsend::client::LocalSendDevice::probe(
        Ipv4Addr::LOCALHOST.into(),
        Some(setup.server.port()),
        &own_device(mine.path()),
    )
    .await
    .unwrap();
    assert_eq!(device.alias, "Ferry Laptop");
    assert!(device.https);
    assert!(
        device
            .fingerprint
            .eq_ignore_ascii_case(setup.server.fingerprint())
    );
    assert!(ferry::localsend::client::find("127.0.0.1").is_some());
}

/// A LocalSend app that, like newer versions, requires the sender's
/// certificate (mutual TLS) and checks that it is the one the offer names; it
/// may also ask for a PIN.
struct StrictApp {
    cert: Vec<u8>,
    key: Vec<u8>,
    pin: Option<&'static str>,
    client_fingerprints: Mutex<Vec<String>>,
    offered_fingerprint: Mutex<Option<String>>,
    uploaded: Mutex<Vec<u8>>,
}

#[derive(Debug)]
struct RequireClientCert(CryptoProvider);

impl tokio_rustls::rustls::server::danger::ClientCertVerifier for RequireClientCert {
    fn root_hint_subjects(&self) -> &[tokio_rustls::rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<tokio_rustls::rustls::server::danger::ClientCertVerified, TlsError> {
        Ok(tokio_rustls::rustls::server::danger::ClientCertVerified::assertion())
    }
    fn client_auth_mandatory(&self) -> bool {
        true
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
            &self.0.signature_verification_algorithms,
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
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl StrictApp {
    fn new(pin: Option<&'static str>) -> Arc<Self> {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::default()
            .self_signed(&key)
            .unwrap();
        Arc::new(Self {
            cert: cert.der().to_vec(),
            key: key.serialize_der(),
            pin,
            client_fingerprints: Mutex::default(),
            offered_fingerprint: Mutex::default(),
            uploaded: Mutex::default(),
        })
    }

    fn fingerprint(&self) -> String {
        ferry::localsend::tls::fingerprint_of(&self.cert)
    }

    /// Serves on a free port of 127.0.0.1; returns the port.
    async fn start(self: &Arc<Self>) -> u16 {
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        let provider = ring::default_provider();
        let config = ServerConfig::builder_with_provider(Arc::new(provider.clone()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_client_cert_verifier(Arc::new(RequireClientCert(provider)))
            .with_single_cert(
                vec![CertificateDer::from(self.cert.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone())),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let (acceptor, app) = (acceptor.clone(), app.clone());
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    if let Some(certs) = tls.get_ref().1.peer_certificates() {
                        app.client_fingerprints
                            .lock()
                            .unwrap()
                            .push(ferry::localsend::tls::fingerprint_of(certs[0].as_ref()));
                    }
                    let service = hyper::service::service_fn(move |req| {
                        let app = app.clone();
                        async move { Ok::<_, std::convert::Infallible>(app.answer(req).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service)
                        .await;
                });
            }
        });
        port
    }

    async fn answer(&self, req: Request<hyper::body::Incoming>) -> hyper::Response<Full<Bytes>> {
        let reply = |status: StatusCode, body: String| {
            hyper::Response::builder()
                .status(status)
                .body(Full::new(Bytes::from(body)))
                .unwrap()
        };
        let query = req.uri().query().unwrap_or_default().to_string();
        if let Some(pin) = self.pin
            && req.uri().path().ends_with("prepare-upload")
            && !query.contains(&format!("pin={pin}"))
        {
            return reply(StatusCode::UNAUTHORIZED, "PIN".into());
        }
        let path = req.uri().path().to_string();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        if path.ends_with("prepare-upload") {
            let offer: Value = serde_json::from_slice(&body).unwrap();
            *self.offered_fingerprint.lock().unwrap() =
                offer["info"]["fingerprint"].as_str().map(str::to_string);
            let files: serde_json::Map<String, Value> = offer["files"]
                .as_object()
                .unwrap()
                .keys()
                .map(|id| (id.clone(), json!(format!("token-{id}"))))
                .collect();
            return reply(
                StatusCode::OK,
                json!({ "sessionId": "s1", "files": files }).to_string(),
            );
        }
        if path.ends_with("upload") {
            self.uploaded.lock().unwrap().extend_from_slice(&body);
            return reply(StatusCode::OK, String::new());
        }
        reply(StatusCode::NOT_FOUND, String::new())
    }
}

fn strict_device(app: &StrictApp, port: u16) -> ferry::localsend::client::LocalSendDevice {
    ferry::localsend::client::LocalSendDevice {
        alias: "Phone".into(),
        model: None,
        device_type: Some("mobile".into()),
        fingerprint: app.fingerprint(),
        address: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        https: true,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_that_requires_the_senders_certificate_gets_it() {
    let app = StrictApp::new(None);
    let port = app.start().await;
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("photo.jpg");
    std::fs::write(&file, vec![9u8; 300_000]).unwrap();
    let mine = tempfile::tempdir().unwrap();
    let result = ferry::localsend::client::send(
        &strict_device(&app, port),
        &[file],
        &own_device(mine.path()),
        None,
        &|_| {},
    )
    .await
    .unwrap();
    assert_eq!(result, ferry::localsend::client::SendResult::Sent);
    assert_eq!(app.uploaded.lock().unwrap().len(), 300_000);
    let own = ferry::localsend::tls::Identity::load_or_create(mine.path()).unwrap();
    let seen = app.client_fingerprints.lock().unwrap().clone();
    assert!(!seen.is_empty() && seen.iter().all(|f| *f == own.fingerprint()));
    assert_eq!(
        app.offered_fingerprint.lock().unwrap().clone(),
        Some(own.fingerprint())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_is_asked_for_and_sent() {
    use ferry::localsend::client::{PinRequired, send};
    let app = StrictApp::new(Some("4711"));
    let port = app.start().await;
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    std::fs::write(&file, b"hi").unwrap();
    let mine = tempfile::tempdir().unwrap();
    let device = strict_device(&app, port);
    let me = own_device(mine.path());
    let missing = send(&device, std::slice::from_ref(&file), &me, None, &|_| {})
        .await
        .unwrap_err();
    assert_eq!(
        missing.downcast_ref::<PinRequired>(),
        Some(&PinRequired { wrong: false })
    );
    let wrong = send(
        &device,
        std::slice::from_ref(&file),
        &me,
        Some("0000"),
        &|_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(
        wrong.downcast_ref::<PinRequired>(),
        Some(&PinRequired { wrong: true })
    );
    send(&device, &[file], &me, Some("4711"), &|_| {})
        .await
        .unwrap();
    assert_eq!(*app.uploaded.lock().unwrap(), b"hi");
}

/// While scanning, LocalSend apps that answer with `register` (as they do,
/// instead of a multicast answer) are listed. Uses the fixed port 53317 and
/// announces on the local network, so it is not run by default.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses port 53317 and multicast on the local network"]
async fn apps_that_answer_a_scan_with_register_are_listed() {
    if std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 53317)).is_err() {
        eprintln!("port 53317 is taken (a running Ferry or LocalSend); its server answers instead");
        return;
    }
    let engine = NodeEngine::new(SecretKey::generate()).await.unwrap();
    let mine = tempfile::tempdir().unwrap();
    let me = own_device(mine.path());
    let scan = tokio::spawn(async move {
        ferry::localsend::client::scan(&engine, &me, std::time::Duration::from_secs(2)).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let answer = json!({
        "alias": "Answering Phone", "version": "2.1", "deviceModel": "Pixel",
        "deviceType": "mobile", "fingerprint": "ANSWERING", "port": 53317,
        "protocol": "https", "download": false
    });
    let reply = call(53317, None, "POST", "register", answer.to_string())
        .await
        .unwrap();
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["alias"], "Ferry Laptop");
    let found = scan.await.unwrap().unwrap();
    assert!(
        found.iter().any(|d| d.alias == "Answering Phone"),
        "{found:?}"
    );
}
