//! Sending to a LocalSend app: one offer for all files (or a text as a
//! message), then every accepted file, with the PIN when the app asks for one.

use super::devices::LocalSendDevice;
use super::http::{Client, empty, encode, json_body};
use super::{OwnDevice, own_info};
use crate::localsend::dto::{FileInfo, PrepareUpload, PrepareUploadResponse};
use crate::transfer::sender::expand_paths;
use crate::transfer::service::{Cancelled, Direction, EventSink, Tracker, TransferStatus};
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::StatusCode;
use hyper::body::Frame;
use rust_i18n::t;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tracing::info;

/// Time the other user has to accept the files.
const DECISION_TIMEOUT: Duration = Duration::from_secs(300);

/// Size of the pieces of an upload body.
const UPLOAD_PIECE: usize = 256 * 1024;

/// Interval of progress reports during an upload.
const PROGRESS_TICK: Duration = Duration::from_millis(100);

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
pub(super) async fn upload(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_and_types_are_encoded() {
        assert_eq!(encode("a b/c?=&"), "a%20b%2Fc%3F%3D%26");
        assert_eq!(encode("abc-_.~09"), "abc-_.~09");
        assert_eq!(mime_type("IMG_1.JPG"), "image/jpeg");
        assert_eq!(mime_type("notes"), "application/octet-stream");
    }
}
