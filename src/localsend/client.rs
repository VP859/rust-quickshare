//! Sending files to LocalSend apps (protocol v2, the sending side).
//!
//! LocalSend devices are found in three ways, all collected in one list
//! ([`nearby`]): their multicast announcements, their `register` requests to
//! the compatibility mode's server, and [`scan`], which announces this device
//! once and listens for the answers. A device can also be given by its IP
//! address ([`LocalSendDevice::probe`]), for networks without multicast.
//!
//! [`send`] then offers the files (`prepare-upload`), waits for the user on
//! the other device, and uploads every accepted file. Over HTTPS the
//! certificate must have the fingerprint the device announced, as in the
//! official app.
//!
//! **Security.** As for receiving, this is LocalSend's security model: the
//! receiver is not paired and its name is whatever it announces. Anyone on the
//! local network can announce a device with the name of someone else's phone;
//! the fingerprint only binds the connection to the announcement, not to a
//! person. Interfaces say so before sending.

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use rust_i18n::t;
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
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
use tracing::{debug, info};

use super::dto::{DeviceInfo, FileInfo, PrepareUpload, PrepareUploadResponse};
use super::{PEER_PREFIX, PORT, PROTOCOL_VERSION};
use crate::transfer::sender::expand_paths;
use crate::transfer::service::{Cancelled, Direction, EventSink, Tracker, TransferStatus};

/// How long a device stays in [`nearby`] after it was last heard. LocalSend
/// apps announce themselves only when they are opened or refreshed.
const NEARBY_TTL: Duration = Duration::from_secs(15 * 60);
/// Most devices remembered, so a flood of announcements cannot fill memory.
const MAX_NEARBY: usize = 64;
/// Time to open a connection (TCP and TLS).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time the other user has to accept the files.
const DECISION_TIMEOUT: Duration = Duration::from_secs(300);
/// Size of the pieces of an upload body.
const UPLOAD_PIECE: usize = 256 * 1024;
/// Interval of progress reports during an upload.
const PROGRESS_TICK: Duration = Duration::from_millis(100);

/// A LocalSend app on the local network, as it describes itself (not verified).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LocalSendDevice {
    /// Name it announces.
    pub alias: String,
    /// Device model it announces (`iPhone`, `Samsung`), if any.
    pub model: Option<String>,
    /// `mobile`, `desktop`, … if given.
    pub device_type: Option<String>,
    /// SHA-256 of its certificate (hex) as announced; the connection must
    /// present exactly this certificate.
    pub fingerprint: String,
    /// Address of its server.
    pub address: SocketAddr,
    /// HTTPS (the default) or plain HTTP (when the app's encryption is off).
    pub https: bool,
}

impl LocalSendDevice {
    /// The device described by `info`, which came from `ip`; `None` when it
    /// has no fingerprint (every LocalSend app sends one).
    pub fn from_info(info: &DeviceInfo, ip: IpAddr) -> Option<Self> {
        let fingerprint = info.fingerprint.trim();
        if fingerprint.is_empty() || fingerprint.len() > 128 {
            return None;
        }
        Some(Self {
            alias: info.clean_alias(),
            model: info.clean_model(),
            device_type: info
                .device_type
                .as_deref()
                .map(crate::pairing::clean_device_name)
                .filter(|t| !t.is_empty()),
            fingerprint: fingerprint.to_string(),
            address: SocketAddr::new(ip.to_canonical(), info.port.unwrap_or(PORT)),
            https: info.protocol.as_deref() != Some("http"),
        })
    }

    /// `alias (model)`, for display.
    pub fn label(&self) -> String {
        match &self.model {
            Some(model) => format!("{} ({model})", self.alias),
            None => self.alias.clone(),
        }
    }

    /// Value of [`TransferEvent::peer`](crate::transfer::service::TransferEvent::peer)
    /// for transfers to this device.
    pub fn peer_id(&self) -> String {
        format!("{PEER_PREFIX}{}", self.address.ip())
    }

    /// Asks the LocalSend app at `ip:port` (default [`PORT`]) who it is (as
    /// `me`), for
    /// sending to an address typed by the user. Over HTTPS its certificate is
    /// taken as it is (there is no announcement to compare it with) and
    /// pinned for the rest of the transfer.
    ///
    /// # Errors
    ///
    /// Fails when nothing answers like a LocalSend app there.
    pub async fn probe(ip: IpAddr, port: Option<u16>, me: &OwnDevice) -> Result<Self> {
        let identity = own_info(me).ok().map(|(_, identity)| identity);
        let address = SocketAddr::new(ip.to_canonical(), port.unwrap_or(PORT));
        let mut last_error = None;
        for https in [true, false] {
            let target = Self {
                alias: String::new(),
                model: None,
                device_type: None,
                fingerprint: String::new(),
                address,
                https,
            };
            match Client::new(&target, None, identity.clone())
                .call("GET", "info", empty(), None)
                .await
            {
                Ok((StatusCode::OK, body, seen)) => {
                    let mut info: DeviceInfo = serde_json::from_slice(&body)
                        .with_context(|| t!("localsend.not_localsend", address = address))?;
                    if https {
                        info.fingerprint = seen.unwrap_or_default();
                    }
                    info.port = Some(address.port());
                    info.protocol = Some(if https { "https" } else { "http" }.into());
                    let device = Self::from_info(&info, ip)
                        .with_context(|| t!("localsend.not_localsend", address = address))?;
                    remember(&device);
                    return Ok(device);
                }
                Ok((status, ..)) => {
                    last_error = Some(anyhow::anyhow!("HTTP {status}"));
                }
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("no answer"))
            .context(t!("localsend.not_localsend", address = address)))
    }
}

/// Devices heard recently, by fingerprint.
static NEARBY: LazyLock<Mutex<HashMap<String, (LocalSendDevice, Instant)>>> =
    LazyLock::new(Mutex::default);

/// Counts changes of [`nearby`], for [`changes`].
static CHANGES: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::Sender::new(0));

/// Changes whenever a device is added to [`nearby`] or its description changes.
pub fn changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

/// Adds or refreshes a device in [`nearby`].
pub(crate) fn remember(device: &LocalSendDevice) {
    let changed = {
        let Ok(mut nearby) = NEARBY.lock() else {
            return;
        };
        nearby.retain(|_, (_, seen)| seen.elapsed() < NEARBY_TTL);
        let key = device.fingerprint.to_ascii_uppercase();
        if nearby.len() >= MAX_NEARBY && !nearby.contains_key(&key) {
            return;
        }
        let changed = nearby.get(&key).is_none_or(|(known, _)| known != device);
        if changed {
            debug!("LocalSend device '{}' at {}", device.alias, device.address);
        }
        nearby.insert(key, (device.clone(), Instant::now()));
        changed
    };
    if changed {
        CHANGES.send_modify(|n| *n += 1);
    }
}

/// Records the device described by a multicast message or a `register`
/// request from `ip`, unless it is this device (`own_fingerprint`).
pub(crate) fn remember_info(info: &DeviceInfo, ip: IpAddr, own_fingerprint: &str) {
    if !info.fingerprint.eq_ignore_ascii_case(own_fingerprint)
        && let Some(device) = LocalSendDevice::from_info(info, ip)
    {
        remember(&device);
    }
}

/// LocalSend apps heard on the local network in the last 15 minutes, sorted by
/// name.
pub fn nearby() -> Vec<LocalSendDevice> {
    let mut list: Vec<_> = NEARBY
        .lock()
        .map(|nearby| {
            nearby
                .values()
                .filter(|(_, seen)| seen.elapsed() < NEARBY_TTL)
                .map(|(device, _)| device.clone())
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|d| (d.alias.to_lowercase(), d.address));
    list
}

/// Finds a device of [`nearby`] by its name (case-insensitive), its IP address
/// or its fingerprint.
pub fn find(query: &str) -> Option<LocalSendDevice> {
    let query = query.trim();
    nearby().into_iter().find(|d| {
        d.alias.eq_ignore_ascii_case(query)
            || d.label().eq_ignore_ascii_case(query)
            || d.address.ip().to_string() == query
            || d.address.to_string() == query
            || d.fingerprint.eq_ignore_ascii_case(query)
    })
}

/// How this device presents itself to LocalSend apps.
#[derive(Debug, Clone, Default)]
pub struct OwnDevice {
    /// Device name shown to the other user.
    pub name: String,
    /// Folder of the compatibility mode's certificate; `None` is the
    /// configuration folder (tests: a temporary folder).
    pub cert_dir: Option<PathBuf>,
}

impl OwnDevice {
    /// This device named `name`, with the certificate in the configuration folder.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            cert_dir: None,
        }
    }
}

/// The device of [`nearby`] with exactly this certificate fingerprint.
pub fn find_by_fingerprint(fingerprint: &str) -> Option<LocalSendDevice> {
    nearby()
        .into_iter()
        .find(|d| d.fingerprint.eq_ignore_ascii_case(fingerprint.trim()))
}

/// This device as described to LocalSend apps, and its certificate: the one
/// of the compatibility mode (created on first use), so an app that knows this
/// device from receiving recognizes it when it sends too.
fn own_info(own: &OwnDevice) -> Result<(DeviceInfo, Arc<super::tls::Identity>)> {
    let dir = match &own.cert_dir {
        Some(dir) => dir.clone(),
        None => crate::config::paths::config_dir()?,
    };
    let identity = Arc::new(super::tls::Identity::load_or_create(&dir)?);
    let info = DeviceInfo {
        alias: crate::pairing::clean_device_name(&own.name),
        version: PROTOCOL_VERSION.to_string(),
        device_model: Some("Ferry".to_string()),
        device_type: Some(
            if cfg!(target_os = "android") {
                "mobile"
            } else {
                "desktop"
            }
            .into(),
        ),
        fingerprint: identity.fingerprint(),
        port: Some(PORT),
        protocol: Some("https".to_string()),
        download: false,
        announce: None,
    };
    Ok((info, identity))
}

/// Announces this device (`me`) on the LocalSend multicast group and
/// collects the answers for `wait`; returns [`nearby`] afterwards.
///
/// LocalSend apps answer by multicast or, when the compatibility mode runs, by
/// `register` to its server; both end up in the list. The announcement shows
/// this device's name to every LocalSend app on the network, so it is sent
/// only when the user asks for it.
///
/// # Errors
///
/// Fails when the multicast socket cannot be opened.
pub async fn scan(
    engine: &crate::node::NodeEngine,
    me: &OwnDevice,
    wait: Duration,
) -> Result<Vec<LocalSendDevice>> {
    let (own, identity) = own_info(me)?;
    // LocalSend apps answer an announcement with `register` on the
    // announced port; without the compatibility mode nobody would listen
    // there, so a server that only takes `register` runs while scanning. When
    // the port is taken (the mode runs) its server takes them instead.
    let stop = tokio_util::sync::CancellationToken::new();
    let _stop_server = stop.clone().drop_guard();
    match tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, PORT)).await {
        Ok(listener) => {
            let server = Arc::new(RegisterOnly { own: own.clone() });
            tokio::spawn(crate::httpd::serve(
                listener,
                Some(identity.acceptor()?),
                server,
                stop.clone(),
            ));
        }
        Err(e) => debug!("LocalSend port taken while scanning ({e}); the running server answers"),
    }
    let socket = super::multicast::bind(PORT).context("LocalSend multicast")?;
    let mut joined = std::collections::BTreeSet::new();
    super::multicast::join(engine, &socket, &mut joined);
    let _ = socket.set_multicast_loop_v4(true);
    let _ = socket.set_multicast_ttl_v4(1);
    let interfaces: std::collections::BTreeSet<_> = joined
        .iter()
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
        .copied()
        .collect();
    let message = serde_json::to_vec(&DeviceInfo {
        announce: Some(true),
        ..own.clone()
    })?;
    let deadline = tokio::time::Instant::now() + wait;
    let mut resend = tokio::time::interval(Duration::from_millis(700));
    let mut buf = vec![0u8; 16 * 1024];
    let mut sent = 0;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            _ = resend.tick(), if sent < 3 => {
                super::multicast::send_to_group(&socket, &message, &interfaces).await;
                sent += 1;
            }
            received = socket.recv_from(&mut buf) => {
                if let Ok((len, from)) = received
                    && let Ok(info) = serde_json::from_slice::<DeviceInfo>(&buf[..len])
                {
                    remember_info(&info, from.ip(), &own.fingerprint);
                }
            }
        }
    }
    Ok(nearby())
}

/// The server of [`scan`]: answers `register` and `info`, nothing else.
struct RegisterOnly {
    /// This device's description.
    own: DeviceInfo,
}

impl crate::httpd::Handler for RegisterOnly {
    type Body = Full<Bytes>;

    async fn handle(
        self: Arc<Self>,
        ip: IpAddr,
        req: Request<hyper::body::Incoming>,
    ) -> hyper::Response<Self::Body> {
        let path = req.uri().path().trim_end_matches('/').to_string();
        match (req.method().as_str(), path.as_str()) {
            ("POST", "/api/localsend/v2/register") => {
                match crate::httpd::read_json::<DeviceInfo>(req, 64 * 1024).await {
                    Ok(info) => {
                        remember_info(&info, ip, &self.own.fingerprint);
                        crate::httpd::json(StatusCode::OK, &self.own)
                    }
                    Err(status) => crate::httpd::message(status, "Invalid body"),
                }
            }
            ("GET", "/api/localsend/v2/info") => crate::httpd::json(StatusCode::OK, &self.own),
            _ => crate::httpd::message(StatusCode::NOT_FOUND, "Not found"),
        }
    }
}

/// A LocalSend app asks for a PIN (`401`): none was given, or a wrong one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinRequired {
    /// A PIN was given but the app did not accept it.
    pub wrong: bool,
}

impl std::fmt::Display for PinRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&if self.wrong {
            t!("localsend.pin_wrong")
        } else {
            t!("localsend.pin_required")
        })
    }
}

impl std::error::Error for PinRequired {}

/// What the other device answered to an offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// The accepted files were uploaded.
    Sent,
    /// The user on the other device declined.
    Rejected,
}

/// Sends files and folders to a LocalSend app: one offer for all of them,
/// then every accepted file. Progress and results arrive at `sink` as for
/// Ferry transfers, one transfer per file, from `localsend:<IP address>`.
/// `pin` is for apps that require one.
///
/// # Errors
///
/// Fails when a folder cannot be read, the device cannot be reached, asks for
/// a PIN, is busy, or a file failed.
pub async fn send(
    device: &LocalSendDevice,
    paths: &[PathBuf],
    me: &OwnDevice,
    pin: Option<&str>,
    sink: EventSink<'_>,
) -> Result<SendResult> {
    let owned = paths.to_vec();
    let items = tokio::task::spawn_blocking(move || expand_paths(&owned))
        .await
        .context("folder scan interrupted")??;
    if items.is_empty() {
        anyhow::bail!(t!("gui.no_files"));
    }
    let mut files = BTreeMap::new();
    let mut local = HashMap::new();
    // Photos without their location and other metadata, when asked for; the
    // copies live until the end of the transfer.
    let mut copies = Vec::new();
    let mut send_paths = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let copy = crate::transfer::metadata::copy_without_metadata(&item.path).await?;
        let send_path = copy
            .as_ref()
            .map_or_else(|| item.path.clone(), |c| c.path().to_path_buf());
        copies.extend(copy);
        let metadata = tokio::fs::metadata(&send_path)
            .await
            .with_context(|| t!("sender.read", path = item.path.display()))?;
        send_paths.insert(index.to_string(), send_path);
        let name = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let file_name = item
            .folder
            .iter()
            .chain(std::iter::once(&name))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("/");
        let id = index.to_string();
        files.insert(
            id.clone(),
            FileInfo {
                id: id.clone(),
                file_name: file_name.clone(),
                size: metadata.len(),
                file_type: mime_type(&name).to_string(),
                sha256: None,
                preview: None,
            },
        );
        local.insert(id, (item.path.clone(), file_name, metadata.len()));
    }

    let trackers: BTreeMap<String, Tracker<'_>> = local
        .iter()
        .map(|(id, (path, name, size))| {
            let tracker = Tracker::for_device(
                device.peer_id(),
                Some(device.label()),
                name,
                *size,
                Direction::Upload,
                sink,
            )
            .with_source(path);
            tracker.emit(0, TransferStatus::AwaitingDecision);
            (id.clone(), tracker)
        })
        .collect();
    let reject_all = |reason: &str| {
        for tracker in trackers.values() {
            tracker.emit(
                0,
                TransferStatus::Rejected {
                    reason: reason.to_string(),
                },
            );
        }
    };

    let (info, identity) = own_info(me)?;
    let client = Client::new(device, pin, Some(identity));
    let offer = PrepareUpload {
        info,
        files: files.into_iter().collect(),
    };
    // Any transfer of the batch cancels the offer while the other user decides.
    let first = trackers.values().next().context("no files")?;
    let answer = first
        .cancellable(async {
            tokio::time::timeout(
                DECISION_TIMEOUT,
                client.call("POST", "prepare-upload", json_body(&offer)?, None),
            )
            .await
            .map_err(|_| anyhow::anyhow!(t!("localsend.no_decision")))?
        })
        .await;
    let (status, body, _) = match answer {
        Ok(answer) => answer,
        Err(e) => {
            for tracker in trackers.values() {
                tracker.finish::<()>(&Err(anyhow::anyhow!("{e:#}")), |()| None);
            }
            return Err(e);
        }
    };
    let accepted: PrepareUploadResponse = match status {
        StatusCode::OK => serde_json::from_slice(&body).context("prepare-upload answer")?,
        StatusCode::NO_CONTENT => {
            for tracker in trackers.values() {
                tracker.emit(
                    tracker_total(tracker),
                    TransferStatus::Completed { path: None },
                );
            }
            return Ok(SendResult::Sent);
        }
        StatusCode::FORBIDDEN => {
            reject_all(&t!("transfer.user_rejected"));
            return Ok(SendResult::Rejected);
        }
        StatusCode::UNAUTHORIZED => {
            let error = PinRequired {
                wrong: pin.is_some(),
            };
            for tracker in trackers.values() {
                tracker.emit(
                    0,
                    TransferStatus::Failed {
                        error: error.to_string(),
                    },
                );
            }
            return Err(error.into());
        }
        other => {
            let reason = refusal(other);
            for tracker in trackers.values() {
                tracker.emit(
                    0,
                    TransferStatus::Failed {
                        error: reason.clone(),
                    },
                );
            }
            anyhow::bail!(reason);
        }
    };
    info!(
        "LocalSend '{}' accepted {} of {} files",
        device.alias,
        accepted.files.len(),
        local.len()
    );

    let mut failures = 0usize;
    for (id, tracker) in &trackers {
        let Some(token) = accepted.files.get(id) else {
            // The other user deselected this file.
            tracker.emit(
                0,
                TransferStatus::Rejected {
                    reason: t!("transfer.user_rejected").into_owned(),
                },
            );
            continue;
        };
        let (_, _, size) = &local[id];
        let path = &send_paths[id];
        let query = format!(
            "sessionId={}&fileId={}&token={}",
            encode(&accepted.session_id),
            encode(id),
            encode(token)
        );
        let result = tracker
            .cancellable(upload(&client, &query, path.clone(), *size, tracker))
            .await;
        if result.as_ref().is_err_and(|e| e.is::<Cancelled>()) {
            // Tell the other device, then stop the rest of the batch.
            let cancel = format!("sessionId={}", encode(&accepted.session_id));
            let _ = client
                .call("POST", &format!("cancel?{cancel}"), empty(), None)
                .await;
            tracker.finish(&result, |()| None);
            for rest in trackers.values().filter(|t| t.id() != tracker.id()) {
                rest.emit(0, TransferStatus::Cancelled);
            }
            return Err(Cancelled.into());
        }
        if result.is_err() {
            failures += 1;
        }
        tracker.finish(&result, |()| None);
    }
    if failures > 0 {
        anyhow::bail!(t!(
            "service.send_failed",
            failed = failures,
            total = local.len()
        ));
    }
    Ok(SendResult::Sent)
}

/// Why an offer was refused, for a status other than accepted, declined or
/// "PIN required".
fn refusal(status: StatusCode) -> String {
    match status {
        StatusCode::CONFLICT => t!("localsend.busy"),
        StatusCode::TOO_MANY_REQUESTS => t!("localsend.too_many"),
        _ => t!("localsend.http_error", status = status.as_u16()),
    }
    .into_owned()
}

/// Sends a text or a link to a LocalSend app, which shows it as a message.
///
/// LocalSend sends a message as an offer of one `text/plain` file whose
/// `preview` is the whole text; the app shows it and answers "204 No Content"
/// without asking for the file. Should an app ask for it anyway, the text is
/// uploaded as that file. `pin` is for apps that require one.
///
/// # Errors
///
/// Fails when the text is empty or longer than [`MAX_TEXT_LEN`] bytes, the
/// device cannot be reached, asks for a PIN ([`PinRequired`]) or is busy.
///
/// [`MAX_TEXT_LEN`]: crate::protocol::messages::MAX_TEXT_LEN
pub async fn send_text(
    device: &LocalSendDevice,
    text: &str,
    me: &OwnDevice,
    pin: Option<&str>,
) -> Result<SendResult> {
    if text.trim().is_empty() {
        anyhow::bail!(t!("service.text_empty"));
    }
    if text.len() > crate::protocol::messages::MAX_TEXT_LEN {
        anyhow::bail!(t!(
            "service.text_too_long",
            max = crate::protocol::messages::MAX_TEXT_LEN
        ));
    }
    let id = "0".to_string();
    let file = FileInfo {
        id: id.clone(),
        file_name: format!("{}.txt", uuid::Uuid::new_v4()),
        size: text.len() as u64,
        file_type: "text/plain".into(),
        sha256: None,
        preview: Some(text.to_string()),
    };
    let (info, identity) = own_info(me)?;
    let client = Client::new(device, pin, Some(identity));
    let offer = PrepareUpload {
        info,
        files: [(id.clone(), file)].into_iter().collect(),
    };
    let (status, body, _) = tokio::time::timeout(
        DECISION_TIMEOUT,
        client.call("POST", "prepare-upload", json_body(&offer)?, None),
    )
    .await
    .map_err(|_| anyhow::anyhow!(t!("localsend.no_decision")))??;
    match status {
        StatusCode::NO_CONTENT => Ok(SendResult::Sent),
        StatusCode::FORBIDDEN => Ok(SendResult::Rejected),
        StatusCode::UNAUTHORIZED => Err(PinRequired {
            wrong: pin.is_some(),
        }
        .into()),
        StatusCode::OK => {
            let accepted: PrepareUploadResponse =
                serde_json::from_slice(&body).context("prepare-upload answer")?;
            let Some(token) = accepted.files.get(&id) else {
                return Ok(SendResult::Rejected);
            };
            let query = format!(
                "upload?sessionId={}&fileId={}&token={}",
                encode(&accepted.session_id),
                encode(&id),
                encode(token)
            );
            let bytes = Full::new(Bytes::from(text.to_string()))
                .map_err(|never| match never {})
                .boxed_unsync();
            let (status, ..) = client
                .call("POST", &query, bytes, Some(text.len() as u64))
                .await?;
            if status != StatusCode::OK {
                anyhow::bail!(t!("localsend.http_error", status = status.as_u16()));
            }
            Ok(SendResult::Sent)
        }
        other => anyhow::bail!(refusal(other)),
    }
}

/// The size of the file a tracker follows (its final progress).
fn tracker_total(tracker: &Tracker<'_>) -> u64 {
    tracker.total()
}

/// Uploads one accepted file, reporting progress to `tracker`.
async fn upload(
    client: &Client,
    query: &str,
    path: PathBuf,
    size: u64,
    tracker: &Tracker<'_>,
) -> Result<()> {
    let file = tokio::fs::File::open(&path)
        .await
        .with_context(|| t!("sender.read", path = path.display()))?;
    let sent = Arc::new(AtomicU64::new(0));
    let counter = sent.clone();
    let pieces = futures_util::stream::unfold(
        (file, size, counter),
        |(mut file, left, counter)| async move {
            if left == 0 {
                return None;
            }
            let len = UPLOAD_PIECE.min(usize::try_from(left).unwrap_or(UPLOAD_PIECE));
            crate::transfer::control::pace(Direction::Upload, len as u64).await;
            let mut buf = vec![0u8; len];
            match file.read_exact(&mut buf).await {
                Ok(_) => {
                    counter.fetch_add(len as u64, Ordering::Relaxed);
                    Some((
                        Ok(Frame::data(Bytes::from(buf))),
                        (file, left - len as u64, counter),
                    ))
                }
                // The file became shorter: end the body with an error.
                Err(e) => Some((Err(e), (file, 0, counter))),
            }
        },
    );
    let body = StreamBody::new(pieces).boxed_unsync();
    let route = format!("upload?{query}");
    let request = client.call("POST", &route, body, Some(size));
    let mut request = std::pin::pin!(request);
    let mut tick = tokio::time::interval(PROGRESS_TICK);
    tracker.progress(0);
    let (status, ..) = loop {
        tokio::select! {
            result = &mut request => break result?,
            _ = tick.tick() => tracker.progress(sent.load(Ordering::Relaxed)),
        }
    };
    if status != StatusCode::OK {
        anyhow::bail!(t!("localsend.http_error", status = status.as_u16()));
    }
    Ok(())
}

/// A request body.
type Body = UnsyncBoxBody<Bytes, std::io::Error>;

/// An empty body.
fn empty() -> Body {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// `value` as a JSON body.
fn json_body<T: serde::Serialize>(value: &T) -> Result<Body> {
    Ok(Full::new(Bytes::from(serde_json::to_vec(value)?))
        .map_err(|never| match never {})
        .boxed_unsync())
}

/// Percent-encodes a query value (everything but unreserved characters).
fn encode(value: &str) -> String {
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

/// MIME type LocalSend apps use to sort a file (photos to the gallery).
fn mime_type(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "zip" => "application/zip",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

/// A connection target with its pinned certificate.
struct Client {
    /// The device.
    device: LocalSendDevice,
    /// PIN for apps that require one.
    pin: Option<String>,
    /// Certificate presented to apps that ask the sender for one.
    identity: Option<Arc<super::tls::Identity>>,
}

/// A byte stream: TLS or plain TCP.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

impl Client {
    /// Requests to `device`.
    fn new(
        device: &LocalSendDevice,
        pin: Option<&str>,
        identity: Option<Arc<super::tls::Identity>>,
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
    async fn call(
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
        let fingerprint = super::tls::fingerprint_of(cert.as_ref());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(alias: &str, fingerprint: &str) -> DeviceInfo {
        DeviceInfo {
            alias: alias.into(),
            version: "2.1".into(),
            device_model: Some("iPhone".into()),
            fingerprint: fingerprint.into(),
            port: Some(53318),
            protocol: Some("https".into()),
            ..DeviceInfo::default()
        }
    }

    #[test]
    fn devices_are_described_from_what_they_announce() {
        let ip: IpAddr = "192.168.1.20".parse().unwrap();
        let device = LocalSendDevice::from_info(&info("Kate\n's iPhone", "AB12"), ip).unwrap();
        assert_eq!(device.alias, "Kate 's iPhone");
        assert_eq!(device.label(), "Kate 's iPhone (iPhone)");
        assert_eq!(device.address, "192.168.1.20:53318".parse().unwrap());
        assert!(device.https);
        assert_eq!(device.peer_id(), "localsend:192.168.1.20");
        assert!(
            LocalSendDevice::from_info(&info("x", ""), ip).is_none(),
            "no fingerprint"
        );
        let http = DeviceInfo {
            protocol: Some("http".into()),
            port: None,
            ..info("Mac", "CD")
        };
        let device = LocalSendDevice::from_info(&http, ip).unwrap();
        assert!(!device.https);
        assert_eq!(device.address.port(), PORT);
    }

    #[test]
    fn the_own_device_is_not_listed_and_devices_are_found() {
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        remember_info(&info("Self", "OWNFINGERPRINT"), ip, "ownfingerprint");
        assert!(find("Self").is_none());
        remember_info(&info("Living room TV", "FFEE01"), ip, "OWN");
        assert_eq!(
            find("living room tv").map(|d| d.fingerprint),
            Some("FFEE01".into())
        );
        assert!(find("10.1.2.3").is_some());
        assert!(find("ffee01").is_some());
    }

    #[test]
    fn query_values_and_types_are_encoded() {
        assert_eq!(encode("a b/c?=&"), "a%20b%2Fc%3F%3D%26");
        assert_eq!(encode("abc-_.~09"), "abc-_.~09");
        assert_eq!(mime_type("IMG_1.JPG"), "image/jpeg");
        assert_eq!(mime_type("notes"), "application/octet-stream");
    }
}
