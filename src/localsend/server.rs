//! HTTPS server of the LocalSend compatibility mode (protocol v2, receiving).
//!
//! Routes under `/api/localsend/v2`: `register` and `info` (discovery),
//! `prepare-upload` (an offer, answered after the user decided), `upload`
//! (the content of one accepted file) and `cancel`. One upload session exists
//! at a time; other offers get `409` until it ends.

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use rust_i18n::t;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::dto::{DeviceInfo, OfferedFile, PrepareUpload, PrepareUploadResponse};
use super::{LocalSendOffer, LocalSendSender, PROTOCOL_VERSION};
use crate::httpd::{UploadError, json, message, parse_query, read_json, receive};
use crate::protocol::messages::MAX_BATCH_NAMES;
use crate::transfer::receiver::check_free_space;
use crate::transfer::risk;
use crate::transfer::service::{
    Cancelled, Direction, OfferDecision, QUESTION_TIMEOUT, Tracker, TransferHandler,
};

/// Largest `prepare-upload` body (10 000 files with long names fit).
const MAX_OFFER_BODY: usize = 8 * 1024 * 1024;
/// Largest `register` body.
const MAX_REGISTER_BODY: usize = 64 * 1024;
/// An accepted session that receives nothing for this long is dropped, so a
/// sender that disappeared does not block later offers.
const SESSION_IDLE: Duration = Duration::from_secs(120);
/// After a declined offer the same address must wait this long (`429`).
const DECLINE_COOLDOWN: Duration = Duration::from_secs(15);

/// What this device tells LocalSend devices about itself.
#[derive(Debug, Clone)]
pub(super) struct OwnInfo {
    /// Certificate fingerprint.
    pub fingerprint: String,
    /// `desktop` or `mobile`.
    pub device_type: &'static str,
    /// Port of the HTTPS server.
    pub port: u16,
}

impl OwnInfo {
    /// The device description with the current device name.
    pub fn describe(&self, alias: String) -> DeviceInfo {
        DeviceInfo {
            alias,
            version: PROTOCOL_VERSION.to_string(),
            device_model: Some("Ferry".to_string()),
            device_type: Some(self.device_type.to_string()),
            fingerprint: self.fingerprint.clone(),
            port: None,
            protocol: None,
            download: false,
            announce: None,
        }
    }
}

/// State of one offered file in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    /// Accepted, not uploaded yet.
    Pending,
    /// Being uploaded.
    Receiving,
    /// Saved and verified.
    Done,
    /// Failed or cancelled; not retried.
    Failed,
}

/// An accepted file of a session.
#[derive(Debug)]
struct SessionFile {
    /// Validated description.
    file: OfferedFile,
    /// Token the sender must present.
    token: String,
    /// Upload state.
    state: FileState,
}

/// The accepted upload session.
#[derive(Debug)]
struct Session {
    /// Session ID.
    id: String,
    /// Address the offer came from; uploads from elsewhere are refused.
    sender: LocalSendSender,
    /// Folder chosen when the offer was accepted.
    dest_dir: PathBuf,
    /// Accepted files by ID.
    files: HashMap<String, SessionFile>,
    /// Last upload activity.
    last_activity: Instant,
    /// Cancels every upload of the session.
    cancel: CancellationToken,
}

impl Session {
    /// Whether no file is pending or being received any more.
    fn is_finished(&self) -> bool {
        self.files
            .values()
            .all(|f| matches!(f.state, FileState::Done | FileState::Failed))
    }

    /// Whether the sender seems gone: nothing received for [`SESSION_IDLE`].
    fn is_abandoned(&self) -> bool {
        !self.files.values().any(|f| f.state == FileState::Receiving)
            && self.last_activity.elapsed() > SESSION_IDLE
    }
}

/// The single session slot.
#[derive(Debug, Default)]
enum Slot {
    /// No session.
    #[default]
    Free,
    /// An offer waits for the user's decision.
    Asking {
        /// ID the session will get.
        id: String,
        /// Address of the sender.
        sender: IpAddr,
        /// Cancelled when the sender cancels before the decision.
        cancel: CancellationToken,
    },
    /// Files are being received.
    Active(Session),
}

/// Shared state of the server.
pub(super) struct Server<H> {
    /// The interface asked about offers and told about transfers.
    handler: Arc<H>,
    /// This device's description.
    info: OwnInfo,
    /// The session slot.
    slot: Mutex<Slot>,
    /// Addresses whose offer was declined, until when they must wait.
    cooldown: Mutex<HashMap<IpAddr, Instant>>,
}

/// Response body type.
type Body = Full<Bytes>;

impl<H: TransferHandler> Server<H> {
    /// A server answering for `handler`.
    pub fn new(handler: Arc<H>, info: OwnInfo) -> Self {
        Self {
            handler,
            info,
            slot: Mutex::new(Slot::Free),
            cooldown: Mutex::new(HashMap::new()),
        }
    }

    /// Locks the session slot; a poisoned lock only means a panic elsewhere.
    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// This device's description for `register` and `info`.
    fn own_info(&self) -> DeviceInfo {
        self.info.describe(self.handler.device_name())
    }

    /// Routes one request.
    async fn route(self: Arc<Self>, ip: IpAddr, req: Request<Incoming>) -> Response<Body> {
        let path = req.uri().path().trim_end_matches('/').to_string();
        let route = path.strip_prefix("/api/localsend/v2/").unwrap_or_default();
        match (req.method(), route) {
            (&Method::POST, "register") => self.register(ip, req).await,
            (&Method::GET, "info") => json(StatusCode::OK, &self.own_info()),
            (&Method::POST, "prepare-upload") => self.prepare_upload(ip, req).await,
            (&Method::POST, "upload") => self.upload(ip, req).await,
            (&Method::POST, "cancel") => self.cancel(ip, &req),
            _ => message(StatusCode::NOT_FOUND, "Not found"),
        }
    }

    /// `register`: a device answering an announcement or scanning the network.
    async fn register(&self, ip: IpAddr, req: Request<Incoming>) -> Response<Body> {
        match read_json::<DeviceInfo>(req, MAX_REGISTER_BODY).await {
            Ok(device) => {
                debug!("LocalSend device '{}' at {ip}", device.clean_alias());
                json(StatusCode::OK, &self.own_info())
            }
            Err(status) => message(status, "Invalid body"),
        }
    }

    /// Whether `ip` must still wait after a declined offer.
    fn cooling_down(&self, ip: IpAddr) -> bool {
        let mut cooldown = self
            .cooldown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        cooldown.retain(|_, until| *until > now);
        cooldown.contains_key(&ip)
    }

    /// Makes `ip` wait [`DECLINE_COOLDOWN`] before its next offer.
    fn start_cooldown(&self, ip: IpAddr) {
        if let Ok(mut cooldown) = self.cooldown.lock()
            && cooldown.len() < 1024
        {
            cooldown.insert(ip, Instant::now() + DECLINE_COOLDOWN);
        }
    }

    /// `prepare-upload`: asks the user and opens a session for the accepted files.
    async fn prepare_upload(
        self: &Arc<Self>,
        ip: IpAddr,
        req: Request<Incoming>,
    ) -> Response<Body> {
        if self.cooling_down(ip) {
            return message(StatusCode::TOO_MANY_REQUESTS, "Too many requests");
        }
        let offer = match read_json::<PrepareUpload>(req, MAX_OFFER_BODY).await {
            Ok(offer) => offer,
            Err(status) => return message(status, "Invalid body"),
        };
        let files = match offer.validate() {
            Ok(files) => files,
            Err(e) => {
                warn!("LocalSend offer from {ip} refused: {e}");
                return message(StatusCode::BAD_REQUEST, &e.0);
            }
        };

        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        {
            let mut slot = self.slot();
            if let Slot::Active(session) = &*slot
                && session.is_abandoned()
            {
                info!("LocalSend session {} abandoned by its sender", session.id);
                session.cancel.cancel();
                *slot = Slot::Free;
            }
            if !matches!(*slot, Slot::Free) {
                return message(StatusCode::CONFLICT, "Blocked by another session");
            }
            *slot = Slot::Asking {
                id: id.clone(),
                sender: ip,
                cancel: cancel.clone(),
            };
        }
        // Frees the slot when this request ends without a session (declined,
        // cancelled, or the sender closed the connection while waiting).
        let _asking = AskingGuard {
            server: self,
            id: &id,
        };

        let sender = LocalSendSender {
            alias: offer.info.clean_alias(),
            model: offer.info.clean_model(),
            address: ip,
        };
        let total_size = files.iter().map(|f| f.size).sum();
        let request = LocalSendOffer {
            id: id.clone(),
            sender: sender.clone(),
            count: u32::try_from(files.len()).unwrap_or(u32::MAX),
            total_size,
            names: files
                .iter()
                .take(MAX_BATCH_NAMES)
                .map(OfferedFile::display_name)
                .collect(),
            risk: risk::worst_risk(&files.iter().map(|f| f.name.clone()).collect::<Vec<_>>()),
        };
        info!(
            "LocalSend offer from '{}' at {ip}: {} file(s), {total_size} B",
            sender.alias, request.count
        );
        let decision = tokio::select! {
            decision = tokio::time::timeout(QUESTION_TIMEOUT, self.handler.decide_localsend(&request)) => {
                decision.unwrap_or_else(|_| OfferDecision::Reject {
                    reason: t!("transfer.user_rejected").into_owned(),
                })
            }
            () = cancel.cancelled() => return message(StatusCode::FORBIDDEN, "Cancelled by sender"),
        };
        let dest_dir = match decision {
            OfferDecision::Reject { reason } => {
                info!("LocalSend offer from {ip} declined: {reason}");
                self.start_cooldown(ip);
                return message(StatusCode::FORBIDDEN, "Rejected");
            }
            OfferDecision::Accept { dest_dir } => dest_dir,
        };
        if let Err(reason) = check_free_space(&dest_dir, total_size) {
            warn!("LocalSend offer from {ip}: {reason}");
            return message(StatusCode::FORBIDDEN, &reason);
        }

        let mut tokens = HashMap::new();
        let mut accepted = HashMap::new();
        for file in files {
            let token = uuid::Uuid::new_v4().simple().to_string();
            tokens.insert(file.id.clone(), token.clone());
            accepted.insert(
                file.id.clone(),
                SessionFile {
                    file,
                    token,
                    state: FileState::Pending,
                },
            );
        }
        let mut slot = self.slot();
        if !matches!(&*slot, Slot::Asking { id: asking, .. } if *asking == id) {
            return message(StatusCode::FORBIDDEN, "Cancelled by sender");
        }
        *slot = Slot::Active(Session {
            id: id.clone(),
            sender,
            dest_dir,
            files: accepted,
            last_activity: Instant::now(),
            cancel: CancellationToken::new(),
        });
        drop(slot);
        json(
            StatusCode::OK,
            &PrepareUploadResponse {
                session_id: id.clone(),
                files: tokens,
            },
        )
    }

    /// `upload`: receives the content of one accepted file.
    async fn upload(self: &Arc<Self>, ip: IpAddr, req: Request<Incoming>) -> Response<Body> {
        let query = parse_query(req.uri().query().unwrap_or_default());
        let (Some(session_id), Some(file_id), Some(token)) = (
            query.get("sessionId"),
            query.get("fileId"),
            query.get("token"),
        ) else {
            return message(StatusCode::BAD_REQUEST, "Missing parameters");
        };
        let claimed = {
            let mut slot = self.slot();
            match &mut *slot {
                Slot::Active(session)
                    if session.id == *session_id && session.sender.address == ip =>
                {
                    match session.files.get_mut(file_id.as_str()) {
                        Some(f) if f.token == *token && f.state == FileState::Pending => {
                            f.state = FileState::Receiving;
                            session.last_activity = Instant::now();
                            Some((
                                f.file.clone(),
                                session.dest_dir.clone(),
                                session.sender.clone(),
                                session.cancel.clone(),
                            ))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        let Some((file, dest_dir, sender, cancel)) = claimed else {
            return message(StatusCode::FORBIDDEN, "Invalid token or IP address");
        };
        let mut finished = FileGuard {
            server: self,
            session_id,
            file_id,
            state: FileState::Failed,
        };

        let sink = |e| self.handler.on_event(e);
        let tracker = Tracker::for_device(
            sender.peer_id(),
            Some(sender.label()),
            &file.display_name(),
            file.size,
            Direction::Download,
            &sink,
        );
        tracker.progress(0);
        let progress = |d| tracker.progress(d);
        let receiving = async {
            Ok(tokio::select! {
                // The sender cancelled the session.
                () = cancel.cancelled() => Err(UploadError::Cancelled),
                result = receive(req.into_body(), &file, &dest_dir, &progress) => result,
            })
        };
        // An error of `cancellable` means the user cancelled on this device.
        let result = tracker
            .cancellable(receiving)
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
                info!(
                    "Received {} from LocalSend '{}'",
                    path.display(),
                    sender.alias
                );
                finished.state = FileState::Done;
                Response::new(Full::default())
            }
            Err(UploadError::Checksum) => {
                message(StatusCode::UNPROCESSABLE_ENTITY, "Checksum mismatch")
            }
            Err(UploadError::Cancelled) => {
                // The other files of the session are not wanted either.
                self.end_session(session_id);
                message(StatusCode::INTERNAL_SERVER_ERROR, "Cancelled")
            }
            Err(UploadError::Failed(e)) => {
                warn!("LocalSend upload of '{}' failed: {e:#}", file.name);
                message(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}"))
            }
        }
    }

    /// `cancel`: the sender stops the session, or the offer still being asked about.
    fn cancel(&self, ip: IpAddr, req: &Request<Incoming>) -> Response<Body> {
        let query = parse_query(req.uri().query().unwrap_or_default());
        let mut slot = self.slot();
        match &*slot {
            Slot::Active(session)
                if session.sender.address == ip
                    && query.get("sessionId").is_none_or(|id| *id == session.id) =>
            {
                info!("LocalSend session {} cancelled by the sender", session.id);
                session.cancel.cancel();
                *slot = Slot::Free;
            }
            Slot::Asking { sender, cancel, .. } if *sender == ip => cancel.cancel(),
            _ => return message(StatusCode::FORBIDDEN, "No session"),
        }
        Response::new(Full::default())
    }

    /// Ends the session `id`, stopping its uploads.
    fn end_session(&self, id: &str) {
        let mut slot = self.slot();
        if let Slot::Active(session) = &*slot
            && session.id == id
        {
            session.cancel.cancel();
            *slot = Slot::Free;
        }
    }
}

/// Frees the slot of an offer that did not become a session.
struct AskingGuard<'a, H> {
    /// The server.
    server: &'a Server<H>,
    /// ID of the offer.
    id: &'a str,
}

impl<H> Drop for AskingGuard<'_, H> {
    fn drop(&mut self) {
        let mut slot = self
            .server
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&*slot, Slot::Asking { id, .. } if id == self.id) {
            *slot = Slot::Free;
        }
    }
}

/// Records the outcome of one upload (also when the request is dropped
/// midway) and ends the session once every file is done.
struct FileGuard<'a, H> {
    /// The server.
    server: &'a Server<H>,
    /// Session of the file.
    session_id: &'a str,
    /// The file.
    file_id: &'a str,
    /// State to record.
    state: FileState,
}

impl<H> Drop for FileGuard<'_, H> {
    fn drop(&mut self) {
        let mut slot = self
            .server
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Slot::Active(session) = &mut *slot
            && session.id == self.session_id
        {
            if let Some(f) = session.files.get_mut(self.file_id) {
                f.state = self.state;
            }
            session.last_activity = Instant::now();
            if session.is_finished() {
                info!("LocalSend session {} finished", session.id);
                *slot = Slot::Free;
            }
        }
    }
}

impl<H: TransferHandler> crate::httpd::Handler for Server<H> {
    type Body = Body;

    fn handle(
        self: Arc<Self>,
        ip: IpAddr,
        req: Request<Incoming>,
    ) -> impl std::future::Future<Output = Response<Body>> + Send {
        self.route(ip, req)
    }
}
