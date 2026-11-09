//! Sharing through a web browser (`ferry::web`): what the page does, as HTTP
//! requests, and the protections of a session.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use bytes::Bytes;
use ferry::protocol::messages::TransferOffer;
use ferry::transfer::risk::Risk;
use ferry::transfer::service::{
    Direction, OfferDecision, TransferEvent, TransferHandler, TransferStatus,
};
use ferry::web::seal::{self, SessionKey, purpose};
use ferry::web::{BrowserOffer, WebShare, WebShareOptions};
use http_body_util::{BodyExt, Full};
use hyper::header::HeaderMap;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use iroh::EndpointId;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                      (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

#[derive(Clone)]
enum Answer {
    Accept(PathBuf),
    Reject,
}

struct Interface {
    answer: Answer,
    offers: Mutex<Vec<BrowserOffer>>,
    events: Mutex<Vec<TransferEvent>>,
}

impl Interface {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer,
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
        unreachable!()
    }
    fn on_event(&self, event: TransferEvent) {
        self.events.lock().unwrap().push(event);
    }
    fn device_name(&self) -> String {
        "Kate's laptop".into()
    }
    async fn decide_browser(&self, offer: &BrowserOffer) -> OfferDecision {
        self.offers.lock().unwrap().push(offer.clone());
        match &self.answer {
            Answer::Accept(dir) => OfferDecision::Accept {
                dest_dir: dir.clone(),
            },
            Answer::Reject => OfferDecision::Reject {
                reason: "no".into(),
            },
        }
    }
}

struct DefaultInterface;
impl TransferHandler for DefaultInterface {
    async fn decide(&self, _: EndpointId, _: &str, _: &TransferOffer) -> OfferDecision {
        unreachable!()
    }
    fn on_event(&self, _: TransferEvent) {}
}

fn options(files: Vec<PathBuf>) -> WebShareOptions {
    WebShareOptions {
        bind: Ipv4Addr::LOCALHOST.into(),
        port: 0,
        files,
        ..WebShareOptions::default()
    }
}

/// `/<token>/` of the session.
fn base(share: &WebShare) -> String {
    // http://127.0.0.1:<port>/<token>/#k=<key>
    let url = share.url().split_once('#').unwrap().0;
    format!("/{}", url.splitn(4, '/').nth(3).unwrap())
}

/// The session key, from the link (`#k=…`), as the page takes it.
fn key(share: &WebShare) -> SessionKey {
    SessionKey::from_fragment(share.url().split_once('#').unwrap().1).unwrap()
}

/// `api/info`, decrypted.
async fn info(share: &WebShare) -> Value {
    let reply = call(share.port(), "GET", &format!("{}api/info", base(share)), "").await;
    assert_eq!(reply.status, StatusCode::OK);
    serde_json::from_slice(&seal::open_all(&key(share), purpose::INFO, &reply.body).unwrap())
        .unwrap()
}

/// An offer of `files` as the page sends it: encrypted.
fn sealed_offer(share: &WebShare, offer: &Value) -> Vec<u8> {
    seal::seal_all(&key(share), purpose::OFFER, offer.to_string().as_bytes()).unwrap()
}

/// File `index` of the offer `id` as the page uploads it: encrypted.
fn sealed_upload(share: &WebShare, id: &str, index: usize, data: &[u8]) -> Vec<u8> {
    seal::seal_all(&key(share), &purpose::upload(id, index), data).unwrap()
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// One request from `from` (an address of the loopback network).
async fn call_from(
    from: Ipv4Addr,
    port: u16,
    method: &str,
    path: &str,
    body: impl Into<Bytes>,
) -> Reply {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(SocketAddr::from((from, 0))).unwrap();
    let tcp = socket
        .connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(conn);
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1")
        .header("user-agent", IPHONE)
        .body(Full::new(body.into()))
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

async fn call(port: u16, method: &str, path: &str, body: impl Into<Bytes>) -> Reply {
    call_from(Ipv4Addr::LOCALHOST, port, method, path, body).await
}

/// Offers `files` (name, content) like the page and uploads them; returns
/// the offer answer and the status of every upload.
async fn send(share: &WebShare, files: &[(&str, &[u8])]) -> (Reply, Vec<StatusCode>) {
    let (port, base) = (share.port(), base(share));
    let offer = json!({ "files": files.iter().map(|(n, d)| json!({ "name": n, "size": d.len() })).collect::<Vec<_>>() });
    let reply = call(
        port,
        "POST",
        &format!("{base}api/offer"),
        sealed_offer(share, &offer),
    )
    .await;
    if reply.status != StatusCode::OK {
        return (reply, Vec::new());
    }
    let id = reply.json()["id"].as_str().unwrap().to_string();
    let mut statuses = Vec::new();
    for (i, (_, data)) in files.iter().enumerate() {
        let path = format!("{base}api/upload/{id}/{i}");
        let body = sealed_upload(share, &id, i, data);
        statuses.push(call(port, "PUT", &path, body).await.status);
    }
    (reply, statuses)
}

fn tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .display()
                        .to_string()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_is_only_behind_the_token() {
    let share = WebShare::start(None, options(vec![]), Interface::new(Answer::Reject))
        .await
        .unwrap();
    let (port, base) = (share.port(), base(&share));
    assert!(share.url().starts_with("http://127.0.0.1:"));
    assert!(
        share.url().contains("/#k="),
        "the key is in the link, after #"
    );
    assert_eq!(base.len(), 1 + 32 + 1, "/<128-bit token>/");

    for path in [
        "/",
        "/index.html",
        "/api/info",
        "/0000/",
        &base.replace(&base[1..9], "00000000"),
    ] {
        let reply = call(port, "GET", path, "").await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        assert!(
            !String::from_utf8_lossy(&reply.body).contains("Ferry"),
            "nothing tells what runs here"
        );
    }

    let redirect = call(port, "GET", base.trim_end_matches('/'), "").await;
    assert_eq!(redirect.status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(redirect.headers["location"], base.as_str());

    let page = call(port, "GET", &base, "").await;
    assert_eq!(page.status, StatusCode::OK);
    let csp = page.headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'"));
    assert_eq!(page.headers["cache-control"], "no-store");
    assert_eq!(page.headers["x-content-type-options"], "nosniff");
    assert_eq!(page.headers["referrer-policy"], "no-referrer");
    assert!(String::from_utf8_lossy(&page.body).contains("page.js"));
    for asset in [
        "page.js",
        "seal.js",
        "page.css",
        "vendor/chacha.js",
        "vendor/_arx.js",
        "vendor/_poly1305.js",
        "vendor/utils.js",
    ] {
        assert_eq!(
            call(port, "GET", &format!("{base}{asset}"), "")
                .await
                .status,
            StatusCode::OK,
            "{asset}"
        );
    }
    assert_eq!(
        call(port, "GET", &format!("{base}vendor/aes.js"), "")
            .await
            .status,
        StatusCode::NOT_FOUND,
        "only the four files of the cipher"
    );

    let sealed = call(port, "GET", &format!("{base}api/info"), "").await;
    assert!(
        !String::from_utf8_lossy(&sealed.body).contains("Kate"),
        "the device name is not readable on the network"
    );
    let info = info(&share).await;
    assert_eq!(info["device"], "Kate's laptop");
    assert_eq!(info["receive"], true);
    assert!(info["expires_in"].as_u64().unwrap() > 29 * 60);
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_files_can_be_downloaded() {
    let src = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
    let path = src.path().join("zdjęcie wakacje.jpg");
    std::fs::write(&path, &data).unwrap();
    let ui = Interface::new(Answer::Reject);
    let share = WebShare::start(None, options(vec![path]), ui.clone())
        .await
        .unwrap();
    let (port, base) = (share.port(), base(&share));

    let info = info(&share).await;
    assert_eq!(info["files"][0]["name"], "zdjęcie wakacje.jpg");
    assert_eq!(info["files"][0]["size"], data.len());

    let file = call(port, "GET", &format!("{base}api/files/0"), "").await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.headers["content-type"], "application/octet-stream");
    assert!(
        file.headers.get("content-disposition").is_none(),
        "the name is not in a header: the page saves the file"
    );
    assert_eq!(file.body.len() as u64, seal::sealed_len(data.len() as u64));
    assert!(
        !file
            .body
            .windows(64)
            .any(|w| data.windows(64).next() == Some(w)),
        "the content is not readable on the network"
    );
    let opened = seal::open_all(&key(&share), &purpose::download(0), &file.body).unwrap();
    assert_eq!(opened, data);
    assert!(
        seal::open_all(&key(&share), &purpose::download(1), &file.body).is_err(),
        "a stream only stands for its own file"
    );
    assert_eq!(
        call(port, "GET", &format!("{base}api/files/1"), "")
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // The download is reported as an upload to the browser.
    for _ in 0..50 {
        if !ui.finals().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let finals = ui.finals();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].direction, Direction::Upload);
    assert!(matches!(finals[0].status, TransferStatus::Completed { .. }));
    assert_eq!(finals[0].peer, "browser:127.0.0.1");
    assert!(
        finals[0]
            .peer_name
            .as_deref()
            .unwrap()
            .contains("Safari, iPhone")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn files_from_the_browser_arrive_after_the_user_accepts() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let share = WebShare::start(None, options(vec![]), ui.clone())
        .await
        .unwrap();
    let photo = vec![9u8; 700_000];
    let (reply, statuses) = send(
        &share,
        &[
            ("IMG_0001.HEIC", &photo),
            ("Notes/list.txt", b"milk"),
            ("empty.txt", b""),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(statuses, [StatusCode::OK; 3]);
    assert_eq!(
        tree(dest.path()),
        ["IMG_0001.HEIC", "Notes/list.txt", "empty.txt"]
    );
    assert_eq!(
        std::fs::read(dest.path().join("IMG_0001.HEIC")).unwrap(),
        photo
    );

    let offer = ui.offers.lock().unwrap()[0].clone();
    assert_eq!(offer.count, 3);
    assert_eq!(offer.total_size, 700_004);
    assert_eq!(offer.browser, "Safari, iPhone");
    assert_eq!(offer.address, IpAddr::from(Ipv4Addr::LOCALHOST));
    assert_eq!(offer.risk, None);
    assert!(
        ui.finals()
            .iter()
            .all(|e| e.direction == Direction::Download
                && matches!(e.status, TransferStatus::Completed { .. }))
    );

    // The upload ended with its last file: the browser can send again.
    let (again, statuses) = send(&share, &[("second.txt", b"2")]).await;
    assert_eq!(
        (again.status, statuses),
        (StatusCode::OK, vec![StatusCode::OK])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn declined_files_are_not_saved_and_the_browser_must_wait() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Reject);
    let share = WebShare::start(None, options(vec![]), ui.clone())
        .await
        .unwrap();
    let (reply, _) = send(&share, &[("a.txt", b"a")]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let (reply, _) = send(&share, &[("a.txt", b"a")]).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(ui.offers.lock().unwrap().len(), 1, "one question, no flood");
    assert!(tree(dest.path()).is_empty());

    // An interface that does not ask declines.
    let share = WebShare::start(None, options(vec![]), Arc::new(DefaultInterface))
        .await
        .unwrap();
    assert_eq!(
        send(&share, &[("a.txt", b"a")]).await.0.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn names_sizes_and_tokens_are_checked() {
    let dest = tempfile::tempdir().unwrap();
    let ui = Interface::new(Answer::Accept(dest.path().to_path_buf()));
    let share = WebShare::start(None, options(vec![]), ui.clone())
        .await
        .unwrap();
    let (port, base) = (share.port(), base(&share));

    for name in ["../../.bashrc", "/etc/passwd", "a/../../x", ""] {
        let (reply, _) = send(&share, &[(name, b"x")]).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{name:?}");
    }
    assert!(ui.offers.lock().unwrap().is_empty(), "not even asked");

    let offer =
        json!({ "files": [{ "name": "a.txt", "size": 4 }, { "name": "setup.exe", "size": 2 }] });
    // An offer in the clear, or with another key, is not even read.
    for body in [
        offer.to_string().into_bytes(),
        seal::seal_all(
            &SessionKey::from_bytes([9; 32]),
            purpose::OFFER,
            offer.to_string().as_bytes(),
        )
        .unwrap(),
    ] {
        let reply = call(port, "POST", &format!("{base}api/offer"), body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }
    assert!(ui.offers.lock().unwrap().is_empty(), "not asked");

    let reply = call(
        port,
        "POST",
        &format!("{base}api/offer"),
        sealed_offer(&share, &offer),
    )
    .await;
    assert_eq!(ui.offers.lock().unwrap()[0].risk, Some(Risk::Program));
    let id = reply.json()["id"].as_str().unwrap().to_string();

    // Another offer while this one runs.
    let busy = call(
        port,
        "POST",
        &format!("{base}api/offer"),
        sealed_offer(&share, &offer),
    )
    .await;
    assert_eq!(busy.status, StatusCode::CONFLICT);
    // Wrong offer, wrong index.
    for path in [
        format!("{base}api/upload/{}/0", "0".repeat(32)),
        format!("{base}api/upload/{id}/7"),
    ] {
        let body = sealed_upload(&share, &id, 0, b"data");
        assert_eq!(
            call(port, "PUT", &path, body).await.status,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    // Wrong size, and content encrypted for another file: refused, nothing kept.
    let longer = sealed_upload(&share, &id, 0, b"toolong");
    let longer = call(port, "PUT", &format!("{base}api/upload/{id}/0"), longer).await;
    assert_eq!(longer.status, StatusCode::INTERNAL_SERVER_ERROR);
    let foreign = sealed_upload(&share, &id, 0, b"bb");
    let foreign = call(port, "PUT", &format!("{base}api/upload/{id}/1"), foreign).await;
    assert_eq!(foreign.status, StatusCode::INTERNAL_SERVER_ERROR);
    // A file is uploaded once.
    let again = sealed_upload(&share, &id, 0, b"data");
    let again = call(port, "PUT", &format!("{base}api/upload/{id}/0"), again).await;
    assert_eq!(again.status, StatusCode::FORBIDDEN);
    assert!(
        tree(dest.path()).is_empty(),
        "nothing and no .part left: {:?}",
        tree(dest.path())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_without_receiving_offers_only_downloads() {
    let ui = Interface::new(Answer::Reject);
    let share = WebShare::start(
        None,
        WebShareOptions {
            receive: false,
            ..options(vec![])
        },
        ui.clone(),
    )
    .await
    .unwrap();
    let info = info(&share).await;
    assert_eq!(info["receive"], false);
    assert_eq!(
        send(&share, &[("a.txt", b"a")]).await.0.status,
        StatusCode::FORBIDDEN
    );
    assert!(ui.offers.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_and_missing_files_cannot_be_shared() {
    let dir = tempfile::tempdir().unwrap();
    for path in [dir.path().to_path_buf(), dir.path().join("missing.txt")] {
        let result = WebShare::start(None, options(vec![path]), Arc::new(DefaultInterface)).await;
        assert!(result.is_err());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_ends_after_its_lifetime_or_when_stopped() {
    let mut share = WebShare::start(
        None,
        WebShareOptions {
            lifetime: Some(Duration::from_millis(300)),
            ..options(vec![])
        },
        Arc::new(DefaultInterface),
    )
    .await
    .unwrap();
    let port = share.port();
    tokio::time::timeout(Duration::from_secs(5), share.finished())
        .await
        .expect("the session expires");
    assert!(share.is_finished());
    assert!(
        tokio::net::TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .await
            .is_err(),
        "nobody listens any more"
    );

    let mut share = WebShare::start(None, options(vec![]), Arc::new(DefaultInterface))
        .await
        .unwrap();
    share.stop();
    tokio::time::timeout(Duration::from_secs(5), share.finished())
        .await
        .expect("stopped");
}

/// Linux routes all of 127.0.0.0/8 to the loopback interface, which gives a
/// second client address.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn the_first_device_owns_the_session() {
    let share = WebShare::start(None, options(vec![]), Arc::new(DefaultInterface))
        .await
        .unwrap();
    let (port, base) = (share.port(), base(&share));
    let page = format!("{base}api/info");
    // This device (127.0.0.1) can try its own link without taking it …
    assert_eq!(call(port, "GET", &page, "").await.status, StatusCode::OK);
    assert_eq!(share.owner(), None);
    // … so the first other device gets it.
    let phone = Ipv4Addr::new(127, 0, 0, 2);
    let owner = call_from(phone, port, "GET", &page, "").await;
    assert_eq!(owner.status, StatusCode::OK);
    assert_eq!(share.owner(), Some(phone.into()));
    let other = call_from(Ipv4Addr::new(127, 0, 0, 3), port, "GET", &page, "").await;
    assert_eq!(
        other.status,
        StatusCode::FORBIDDEN,
        "a token seen by someone else is of no use"
    );
    assert_eq!(share.refused(), [IpAddr::from(Ipv4Addr::new(127, 0, 0, 3))]);
    assert_eq!(
        call_from(phone, port, "GET", &page, "").await.status,
        StatusCode::OK
    );
    assert_eq!(
        call(port, "GET", &page, "").await.status,
        StatusCode::OK,
        "this device still can"
    );
}
