//! HTTP server of a browser session.
//!
//! Everything lives under `/<token>/`: the page (`index.html`, `page.js`,
//! `seal.js`, `page.css`, `vendor/*.js`), `api/info`, `api/files/<n>`
//! (download), `api/offer` (files the browser wants to send, answered after
//! the user decided) and `api/upload/<offer>/<n>` (the content of one accepted
//! file). Everything under `api/` with content is encrypted with the session
//! key ([`super::seal`]).

use bytes::Bytes;
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Method, Request, Response, StatusCode, header};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use super::body::{self, Body};
use super::seal::{self, Opener, Sealer, SessionKey, purpose};
use super::{BrowserOffer, PEER_PREFIX};
use crate::httpd::{UploadError, json, message, read_bytes, receive};
use crate::localsend::dto::{OfferedFile, split_path};
use crate::protocol::messages::MAX_BATCH_NAMES;
use crate::transfer::receiver::check_free_space;
use crate::transfer::risk;
use crate::transfer::service::{
    Cancelled, Direction, OfferDecision, QUESTION_TIMEOUT, Tracker, TransferHandler,
};

/// Most files one offer may contain.
const MAX_FILES: usize = 1000;
/// Largest offer body.
const MAX_OFFER_BODY: usize = 1024 * 1024;
/// After a declined offer the browser must wait this long (`429`).
const DECLINE_COOLDOWN: Duration = Duration::from_secs(15);
/// An accepted offer that receives nothing for this long is dropped.
const SESSION_IDLE: Duration = Duration::from_secs(120);
/// Chunk size of downloads: one encrypted record.
const CHUNK: usize = seal::RECORD;

/// The page of the session.
const INDEX_HTML: &str = include_str!("page/index.html");
/// Its script.
const PAGE_JS: &str = include_str!("page/page.js");
/// Its encryption ([`super::seal`] in JavaScript).
const SEAL_JS: &str = include_str!("page/seal.js");
/// Its style sheet.
const PAGE_CSS: &str = include_str!("page/page.css");
/// ChaCha20-Poly1305 of `@noble/ciphers` (unchanged copies of the npm package,
/// MIT licence in `page/vendor/LICENSE`).
const VENDOR: [(&str, &str); 4] = [
    ("chacha.js", include_str!("page/vendor/chacha.js")),
    ("_arx.js", include_str!("page/vendor/_arx.js")),
    ("_poly1305.js", include_str!("page/vendor/_poly1305.js")),
    ("utils.js", include_str!("page/vendor/utils.js")),
];
/// The page runs only its own script and style and talks only to this server.
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
                   img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// A file the browser may download.
#[derive(Debug, Clone)]
pub(super) struct SharedFile {
    /// Name shown and suggested for saving.
    pub name: String,
    /// Where it is.
    pub path: PathBuf,
    /// Size when the session started.
    pub size: u64,
}

/// State of one accepted file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    /// Not uploaded yet.
    Pending,
    /// Being uploaded.
    Receiving,
    /// Saved, failed or cancelled.
    Done,
}

/// An accepted offer.
#[derive(Debug)]
struct Upload {
    /// Offer ID, part of the upload URLs.
    id: String,
    /// Folder chosen when it was accepted.
    dest_dir: PathBuf,
    /// The files in the order of the offer.
    files: Vec<(OfferedFile, FileState)>,
    /// How the browser is shown in transfers.
    label: String,
    /// Last upload activity.
    last_activity: Instant,
}

/// The single offer slot.
#[derive(Debug, Default)]
enum Slot {
    /// No offer.
    #[default]
    Free,
    /// The user is being asked.
    Asking(String),
    /// Files are being received.
    Active(Upload),
}

/// Who may use the session: the first other device that opened the link.
/// This device itself (its own addresses) never takes the session, so trying
/// the link here first does not lock out the phone it was meant for.
#[derive(Debug, Default)]
pub(super) struct Access {
    /// Addresses of this device.
    own: std::collections::HashSet<IpAddr>,
    /// The device that owns the session.
    owner: Mutex<Option<IpAddr>>,
    /// Other devices that were refused, for the interface (at most 16).
    refused: Mutex<Vec<IpAddr>>,
}

impl Access {
    /// Access for a session on a device with the addresses `own`.
    pub(super) fn new(own: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            own: own.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Whether `ip` may use the session; the first other device takes it.
    fn admit(&self, ip: IpAddr) -> bool {
        if self.own.contains(&ip) {
            return true;
        }
        let mut owner = lock(&self.owner);
        match *owner {
            None => {
                info!("Browser session: opened by {ip}, which now owns it");
                *owner = Some(ip);
                true
            }
            Some(o) if o == ip => true,
            Some(_) => {
                let mut refused = lock(&self.refused);
                if !refused.contains(&ip) && refused.len() < 16 {
                    refused.push(ip);
                }
                false
            }
        }
    }

    /// The device that owns the session, if one opened it.
    pub(super) fn owner(&self) -> Option<IpAddr> {
        *lock(&self.owner)
    }

    /// Other devices that were refused.
    pub(super) fn refused(&self) -> Vec<IpAddr> {
        lock(&self.refused).clone()
    }
}

/// Shared state of the server.
pub(super) struct WebServer<H> {
    /// The interface asked about offers and told about transfers.
    handler: Arc<H>,
    /// The token in every path.
    token: String,
    /// The key everything with content is encrypted with.
    key: SessionKey,
    /// Files for download.
    files: Vec<SharedFile>,
    /// Whether the browser may send files.
    receive: bool,
    /// When the session ends.
    expires_at: Option<Instant>,
    /// Who may use the session.
    access: Arc<Access>,
    /// The offer slot.
    slot: Mutex<Slot>,
    /// A declined browser may offer again from this instant.
    cooldown_until: Mutex<Option<Instant>>,
}

/// Locks `m`; a poisoned lock only means a panic elsewhere.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `a` equals `b`, in time that does not depend on where they differ.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Adds the headers every response carries.
fn harden(mut res: Response<Body>) -> Response<Body> {
    let headers = res.headers_mut();
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (
            HeaderName::from_static("cross-origin-resource-policy"),
            "same-origin",
        ),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    res
}

/// A static asset.
fn asset(content_type: &'static str, text: &'static str) -> Response<Body> {
    let mut res = Response::new(body::full(text));
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    res
}

/// A short plain-text answer.
fn text(status: StatusCode, text: &str) -> Response<Body> {
    let mut res = Response::new(body::full(text.to_string()));
    *res.status_mut() = status;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    res
}

/// A short description of the browser from its `User-Agent`.
pub(super) fn describe_browser(user_agent: &str) -> String {
    let browser = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("SamsungBrowser", "Samsung Internet"),
        ("Firefox/", "Firefox"),
        ("FxiOS", "Firefox"),
        ("CriOS", "Chrome"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .iter()
    .find(|(needle, _)| user_agent.contains(needle))
    .map_or("Browser", |(_, name)| name);
    let system = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Windows", "Windows"),
        ("Mac OS X", "macOS"),
        ("CrOS", "ChromeOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(needle, _)| user_agent.contains(needle))
    .map(|(_, name)| *name);
    match system {
        Some(system) => format!("{browser}, {system}"),
        None => browser.to_string(),
    }
}

/// What the page learns about the session.
#[derive(Serialize)]
struct Info<'a> {
    /// Name of this device.
    device: String,
    /// Files for download.
    files: Vec<FileEntry<'a>>,
    /// Whether files may be sent.
    receive: bool,
    /// Seconds until the session ends.
    expires_in: Option<u64>,
    /// Most files in one offer.
    max_files: usize,
}

/// A file for download, as listed on the page.
#[derive(Serialize)]
struct FileEntry<'a> {
    /// Index in the download URL.
    id: usize,
    /// File name.
    name: &'a str,
    /// Size in bytes.
    size: u64,
}

/// Files the browser wants to send.
#[derive(Deserialize)]
struct Offer {
    /// The files, in the order they will be uploaded.
    files: Vec<OfferEntry>,
}

/// One file of an [`Offer`].
#[derive(Deserialize)]
struct OfferEntry {
    /// Name, or a relative path with `/` for files of a folder.
    name: String,
    /// Size in bytes.
    size: u64,
}

impl<H: TransferHandler> WebServer<H> {
    /// A server for one session.
    pub fn new(
        handler: Arc<H>,
        token: String,
        key: SessionKey,
        files: Vec<SharedFile>,
        receive: bool,
        expires_at: Option<Instant>,
        access: Arc<Access>,
    ) -> Self {
        Self {
            handler,
            token,
            key,
            files,
            receive,
            expires_at,
            access,
            slot: Mutex::new(Slot::Free),
            cooldown_until: Mutex::new(None),
        }
    }

    /// Routes one request.
    async fn route(self: Arc<Self>, ip: IpAddr, req: Request<Incoming>) -> Response<Body> {
        let path = req.uri().path().to_string();
        let mut parts = path.trim_start_matches('/').splitn(2, '/');
        let (token, rest) = (parts.next().unwrap_or_default(), parts.next());
        if !same_token(token, &self.token) {
            return text(StatusCode::NOT_FOUND, "Not found");
        }
        if !self.access.admit(ip) {
            warn!("Browser session: refused {ip}, the link is used by another device");
            return text(StatusCode::FORBIDDEN, &t!("web.taken"));
        }
        let Some(rest) = rest else {
            // `/<token>` → `/<token>/`, so the page's relative links work.
            let mut res = Response::new(body::full(Bytes::new()));
            *res.status_mut() = StatusCode::PERMANENT_REDIRECT;
            if let Ok(location) = HeaderValue::from_str(&format!("/{}/", self.token)) {
                res.headers_mut().insert(header::LOCATION, location);
            }
            return res;
        };
        let method = req.method().clone();
        let user_agent = req
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let segments: Vec<&str> = rest.split('/').collect();
        match (&method, segments.as_slice()) {
            (&Method::GET, [""] | ["index.html"]) => {
                let mut res = asset("text/html; charset=utf-8", INDEX_HTML);
                res.headers_mut().insert(
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static(CSP),
                );
                res
            }
            (&Method::GET, ["page.js"]) => asset("text/javascript; charset=utf-8", PAGE_JS),
            (&Method::GET, ["seal.js"]) => asset("text/javascript; charset=utf-8", SEAL_JS),
            (&Method::GET, ["page.css"]) => asset("text/css; charset=utf-8", PAGE_CSS),
            (&Method::GET, ["vendor", name]) => match VENDOR.iter().find(|(n, _)| n == name) {
                Some((_, script)) => asset("text/javascript; charset=utf-8", script),
                None => text(StatusCode::NOT_FOUND, "Not found"),
            },
            (&Method::GET, ["api", "info"]) => self.sealed_info(),
            (&Method::GET, ["api", "files", id]) => self.download(ip, &user_agent, id),
            (&Method::POST, ["api", "offer"]) => {
                body::boxed(self.offer(ip, &user_agent, req).await)
            }
            (&Method::PUT | &Method::POST, ["api", "upload", offer, index]) => {
                let (offer, index) = (offer.to_string(), index.to_string());
                body::boxed(self.upload(ip, &offer, &index, req).await)
            }
            _ => text(StatusCode::NOT_FOUND, "Not found"),
        }
    }

    /// `api/info`, encrypted.
    fn sealed_info(&self) -> Response<Body> {
        let sealed = serde_json::to_vec(&self.info())
            .map_err(anyhow::Error::from)
            .and_then(|plain| seal::seal_all(&self.key, purpose::INFO, &plain));
        match sealed {
            Ok(bytes) => {
                let mut res = Response::new(body::full(bytes));
                res.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                );
                res
            }
            Err(e) => text(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        }
    }

    /// What `api/info` says.
    fn info(&self) -> Info<'_> {
        Info {
            device: self.handler.device_name(),
            files: self
                .files
                .iter()
                .enumerate()
                .map(|(id, f)| FileEntry {
                    id,
                    name: &f.name,
                    size: f.size,
                })
                .collect(),
            receive: self.receive,
            expires_in: self
                .expires_at
                .map(|at| at.saturating_duration_since(Instant::now()).as_secs()),
            max_files: MAX_FILES,
        }
    }

    /// `api/files/<n>`: streams a shared file, encrypted, reported as an
    /// upload. The page decrypts it and saves it under its name (which is
    /// therefore not in a header).
    fn download(&self, ip: IpAddr, user_agent: &str, id: &str) -> Response<Body> {
        let Some((index, file)) = id
            .parse::<usize>()
            .ok()
            .and_then(|i| self.files.get(i).map(|f| (i, f.clone())))
        else {
            return text(StatusCode::NOT_FOUND, "Not found");
        };
        let Ok(mut sealer) = Sealer::new(&self.key, &purpose::download(index)) else {
            return text(StatusCode::INTERNAL_SERVER_ERROR, "no random numbers");
        };
        let (tx, body) = body::Channel::open(seal::sealed_len(file.size));
        let handler = self.handler.clone();
        let label = super::label(&describe_browser(user_agent), ip);
        tokio::spawn(async move {
            let sink = |e| handler.on_event(e);
            let tracker = Tracker::for_device(
                format!("{PEER_PREFIX}{ip}"),
                Some(label),
                &file.name,
                file.size,
                Direction::Upload,
                &sink,
            )
            .with_source(&file.path);
            tracker.progress(0);
            let result = tracker
                .cancellable(async {
                    let closed = || anyhow::anyhow!(t!("web.download_closed"));
                    let mut source = tokio::fs::File::open(&file.path).await?;
                    tx.send(Ok(Bytes::copy_from_slice(&sealer.prefix())))
                        .await
                        .map_err(|_| closed())?;
                    let (mut sent, mut buf) = (0u64, vec![0u8; CHUNK]);
                    // One record per chunk; an empty file is one empty record.
                    loop {
                        let want = usize::try_from((file.size - sent).min(CHUNK as u64))?;
                        crate::transfer::control::pace(Direction::Upload, want as u64).await;
                        source.read_exact(&mut buf[..want]).await.map_err(|_| {
                            anyhow::anyhow!("the file became shorter while it was downloaded")
                        })?;
                        sent += want as u64;
                        let record = sealer.seal(&buf[..want], sent == file.size)?;
                        tx.send(Ok(Bytes::from(record)))
                            .await
                            .map_err(|_| closed())?;
                        tracker.progress(sent);
                        if sent == file.size {
                            return Ok(());
                        }
                    }
                })
                .await;
            if result.is_err() {
                let _ = tx
                    .send(Err(std::io::Error::other("download stopped")))
                    .await;
            }
            tracker.finish(&result, |()| None);
        });
        let mut res = Response::new(body);
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        res
    }

    /// Whether the browser must still wait after a declined offer.
    fn cooling_down(&self) -> bool {
        lock(&self.cooldown_until).is_some_and(|until| Instant::now() < until)
    }

    /// `api/offer`: asks the user and opens an upload for the accepted files.
    async fn offer(
        self: &Arc<Self>,
        ip: IpAddr,
        user_agent: &str,
        req: Request<Incoming>,
    ) -> Response<http_body_util::Full<Bytes>> {
        if !self.receive {
            return message(StatusCode::FORBIDDEN, &t!("web.receive_off"));
        }
        if self.cooling_down() {
            return message(StatusCode::TOO_MANY_REQUESTS, &t!("web.wait"));
        }
        let sealed = match read_bytes(
            req,
            usize::try_from(seal::sealed_len(MAX_OFFER_BODY as u64)).unwrap_or(usize::MAX),
        )
        .await
        {
            Ok(sealed) => sealed,
            Err(status) => return message(status, "Invalid body"),
        };
        let Some(offer) = seal::open_all(&self.key, purpose::OFFER, &sealed)
            .ok()
            .and_then(|plain| serde_json::from_slice::<Offer>(&plain).ok())
        else {
            warn!("Browser offer refused: it does not decrypt with the session key");
            return message(StatusCode::BAD_REQUEST, "Invalid body");
        };
        if offer.files.is_empty() || offer.files.len() > MAX_FILES {
            return message(StatusCode::BAD_REQUEST, "Invalid number of files");
        }
        let mut files = Vec::with_capacity(offer.files.len());
        let mut total: u64 = 0;
        for (index, entry) in offer.files.iter().enumerate() {
            let Ok((folder, name)) = split_path(&entry.name) else {
                warn!("Browser offer refused: unsafe name {:?}", entry.name);
                return message(
                    StatusCode::BAD_REQUEST,
                    &t!("web.bad_name", name = &entry.name),
                );
            };
            let Some(sum) = total.checked_add(entry.size) else {
                return message(StatusCode::BAD_REQUEST, "Invalid size");
            };
            total = sum;
            files.push(OfferedFile {
                id: index.to_string(),
                folder,
                name,
                size: entry.size,
                sha256: None,
            });
        }

        let id = uuid::Uuid::new_v4().simple().to_string();
        {
            let mut slot = lock(&self.slot);
            if let Slot::Active(upload) = &*slot
                && !upload.files.iter().any(|(_, s)| *s == FileState::Receiving)
                && (upload.files.iter().all(|(_, s)| *s == FileState::Done)
                    || upload.last_activity.elapsed() > SESSION_IDLE)
            {
                *slot = Slot::Free;
            }
            if !matches!(*slot, Slot::Free) {
                return message(StatusCode::CONFLICT, &t!("web.busy"));
            }
            *slot = Slot::Asking(id.clone());
        }
        let _asking = AskingGuard {
            server: self,
            id: &id,
        };

        let request = BrowserOffer {
            id: id.clone(),
            address: ip,
            browser: describe_browser(user_agent),
            count: u32::try_from(files.len()).unwrap_or(u32::MAX),
            total_size: total,
            names: files
                .iter()
                .take(MAX_BATCH_NAMES)
                .map(OfferedFile::display_name)
                .collect(),
            risk: risk::worst_risk(&files.iter().map(|f| f.name.clone()).collect::<Vec<_>>()),
        };
        info!(
            "Browser at {ip} offers {} file(s), {total} B",
            request.count
        );
        let decision =
            tokio::time::timeout(QUESTION_TIMEOUT, self.handler.decide_browser(&request))
                .await
                .unwrap_or_else(|_| OfferDecision::Reject {
                    reason: t!("transfer.user_rejected").into_owned(),
                });
        let dest_dir = match decision {
            OfferDecision::Reject { .. } => {
                *lock(&self.cooldown_until) = Some(Instant::now() + DECLINE_COOLDOWN);
                return message(StatusCode::FORBIDDEN, &t!("web.declined"));
            }
            OfferDecision::Accept { dest_dir } => dest_dir,
        };
        if let Err(reason) = check_free_space(&dest_dir, total) {
            return message(StatusCode::FORBIDDEN, &reason);
        }
        let mut slot = lock(&self.slot);
        if !matches!(&*slot, Slot::Asking(asking) if *asking == id) {
            return message(StatusCode::CONFLICT, &t!("web.busy"));
        }
        *slot = Slot::Active(Upload {
            id: id.clone(),
            dest_dir,
            files: files.into_iter().map(|f| (f, FileState::Pending)).collect(),
            label: request.label(),
            last_activity: Instant::now(),
        });
        json(StatusCode::OK, &serde_json::json!({ "id": id }))
    }

    /// `api/upload/<offer>/<n>`: receives one accepted file.
    async fn upload(
        self: &Arc<Self>,
        ip: IpAddr,
        offer: &str,
        index: &str,
        req: Request<Incoming>,
    ) -> Response<http_body_util::Full<Bytes>> {
        let index: usize = index.parse().unwrap_or(usize::MAX);
        let claimed = {
            let mut slot = lock(&self.slot);
            match &mut *slot {
                Slot::Active(upload) if same_token(&upload.id, offer) => {
                    upload.last_activity = Instant::now();
                    let (dest_dir, label) = (upload.dest_dir.clone(), upload.label.clone());
                    match upload.files.get_mut(index) {
                        Some((file, state)) if *state == FileState::Pending => {
                            *state = FileState::Receiving;
                            Some((file.clone(), dest_dir, label))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        let Some((file, dest_dir, label)) = claimed else {
            return message(StatusCode::FORBIDDEN, &t!("web.not_accepted"));
        };
        let _done = FileGuard {
            server: self,
            offer,
            index,
        };
        let sink = |e| self.handler.on_event(e);
        let tracker = Tracker::for_device(
            format!("{PEER_PREFIX}{ip}"),
            Some(label),
            &file.display_name(),
            file.size,
            Direction::Download,
            &sink,
        );
        tracker.progress(0);
        let progress = |d| tracker.progress(d);
        let result = tracker
            .cancellable(async {
                let opener = Opener::new(&self.key, &purpose::upload(offer, index), file.size);
                Ok(receive(req.into_body(), &file, &dest_dir, &progress, Some(opener)).await)
            })
            .await
            .unwrap_or(Err(UploadError::Cancelled));
        let report: anyhow::Result<&PathBuf> = match &result {
            Ok(path) => Ok(path),
            Err(UploadError::Cancelled) => Err(Cancelled.into()),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        };
        tracker.finish(&report, |p| Some(p.display().to_string()));
        match result {
            Ok(path) => {
                info!("Received {} from a browser at {ip}", path.display());
                json(StatusCode::OK, &serde_json::json!({ "ok": true }))
            }
            Err(UploadError::Cancelled) => {
                // Cancelled in Ferry: the rest of the offer is not wanted either.
                self.end_upload(offer);
                message(StatusCode::GONE, &t!("transfer.cancelled"))
            }
            Err(e) => {
                warn!("Upload of '{}' from a browser failed: {e}", file.name);
                message(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            }
        }
    }

    /// Ends the accepted offer `id`.
    fn end_upload(&self, id: &str) {
        let mut slot = lock(&self.slot);
        if matches!(&*slot, Slot::Active(upload) if upload.id == id) {
            *slot = Slot::Free;
        }
    }
}

impl<H: TransferHandler> crate::httpd::Handler for WebServer<H> {
    type Body = Body;

    async fn handle(self: Arc<Self>, ip: IpAddr, req: Request<Incoming>) -> Response<Body> {
        harden(self.route(ip, req).await)
    }
}

/// Frees the slot of an offer that did not become an upload.
struct AskingGuard<'a, H> {
    /// The server.
    server: &'a WebServer<H>,
    /// ID of the offer.
    id: &'a str,
}

impl<H> Drop for AskingGuard<'_, H> {
    fn drop(&mut self) {
        let mut slot = lock(&self.server.slot);
        if matches!(&*slot, Slot::Asking(id) if id == self.id) {
            *slot = Slot::Free;
        }
    }
}

/// Marks a file as done (also when its request is dropped midway) and frees
/// the slot once every file of the offer is done.
struct FileGuard<'a, H> {
    /// The server.
    server: &'a WebServer<H>,
    /// The offer.
    offer: &'a str,
    /// The file.
    index: usize,
}

impl<H> Drop for FileGuard<'_, H> {
    fn drop(&mut self) {
        let mut slot = lock(&self.server.slot);
        if let Slot::Active(upload) = &mut *slot
            && upload.id == self.offer
        {
            if let Some((_, state)) = upload.files.get_mut(self.index) {
                *state = FileState::Done;
            }
            upload.last_activity = Instant::now();
            if upload.files.iter().all(|(_, s)| *s == FileState::Done) {
                *slot = Slot::Free;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_are_described_briefly() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                      (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        assert_eq!(describe_browser(iphone), "Safari, iPhone");
        let android = "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 (KHTML, like Gecko) \
                       Chrome/140.0 Mobile Safari/537.36";
        assert_eq!(describe_browser(android), "Chrome, Android");
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
        assert_eq!(describe_browser(firefox), "Firefox, Linux");
        assert_eq!(describe_browser(""), "Browser");
    }

    #[test]
    fn tokens_are_compared_whole() {
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abc", "abd"));
        assert!(!same_token("abc", "ab"));
        assert!(!same_token("", "a"));
    }
}
