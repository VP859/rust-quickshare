//! Local history of finished transfers.
//!
//! Every transfer that ends (completed, failed, declined or cancelled) is
//! appended as one JSON line to `history.jsonl` in the configuration folder,
//! readable only by the owner. Nothing leaves the device. Recording is off
//! until an interface calls [`enable`] (tests and library users write nothing)
//! and can be switched off with
//! [`Config::disable_history`](crate::config::Config::disable_history). The
//! file keeps the newest [`MAX_ENTRIES`] transfers.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::transfer::service::{Direction, TransferEvent, TransferStatus};

/// Most transfers kept; older ones are dropped.
pub const MAX_ENTRIES: usize = 1000;
/// File name in the configuration folder.
const FILE_NAME: &str = "history.jsonl";

/// What was transferred.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryKind {
    /// A file; [`HistoryEntry::file_name`] is its name.
    #[default]
    File,
    /// A text or link. Its content is never recorded (it may be a password or
    /// an account number): only that it was sent or received, and its size.
    Text,
}

impl HistoryKind {
    /// Whether this is [`HistoryKind::File`] (omitted from the file).
    fn is_file(&self) -> bool {
        *self == Self::File
    }
}

/// One finished transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// File or text.
    #[serde(default, skip_serializing_if = "HistoryKind::is_file")]
    pub kind: HistoryKind,
    /// When it ended.
    pub time: DateTime<Utc>,
    /// Sent or received.
    pub direction: Direction,
    /// The other device (Node ID, `localsend:…`, `browser:…`).
    pub peer: String,
    /// Its name when the transfer ended (alias of a trusted device, LocalSend
    /// or browser name), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_name: Option<String>,
    /// File name (with folders); empty for a text.
    pub file_name: String,
    /// Size in bytes.
    pub size: u64,
    /// `completed`, `failed`, `rejected` or `cancelled`.
    pub status: String,
    /// Where it was saved, or why it failed or was declined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl HistoryEntry {
    /// The entry for a final `event`; `None` while the transfer is running.
    pub fn from_event(event: &TransferEvent) -> Option<Self> {
        let (status, detail) = match &event.status {
            TransferStatus::Completed { path } => ("completed", path.clone()),
            TransferStatus::Failed { error } => ("failed", Some(error.clone())),
            TransferStatus::Rejected { reason } => ("rejected", Some(reason.clone())),
            TransferStatus::Cancelled => ("cancelled", None),
            _ => return None,
        };
        Some(Self {
            kind: HistoryKind::File,
            time: Utc::now(),
            direction: event.direction,
            peer: event.peer.clone(),
            peer_name: event.peer_name.clone(),
            file_name: event.file_name.clone(),
            size: event.total,
            status: status.to_string(),
            detail,
        })
    }

    /// The entry for a text of `size` bytes sent to or received from `peer`;
    /// `outcome` is `Ok(())` when it was delivered or accepted, otherwise why
    /// not (`rejected`) or the error (`failed`).
    pub fn text(
        direction: Direction,
        peer: &str,
        size: usize,
        outcome: Result<(), (&'static str, String)>,
    ) -> Self {
        let (status, detail) = match outcome {
            Ok(()) => ("completed", None),
            Err((status, detail)) => (status, Some(detail)),
        };
        Self {
            kind: HistoryKind::Text,
            time: Utc::now(),
            direction,
            peer: peer.to_string(),
            peer_name: None,
            file_name: String::new(),
            size: size as u64,
            status: status.to_string(),
            detail,
        }
    }

    /// Whether `query` (lowercase) occurs in the name, the device or the detail.
    fn matches(&self, query: &str) -> bool {
        [
            Some(self.file_name.as_str()),
            self.peer_name.as_deref(),
            Some(self.peer.as_str()),
            self.detail.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(query))
    }
}

/// The file recording is written to; `None` while it is off.
static TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Locks [`TARGET`].
fn target() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    TARGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The history file in the configuration folder.
///
/// # Errors
///
/// Fails when the configuration folder cannot be determined.
pub fn default_path() -> Result<PathBuf> {
    Ok(crate::config::paths::config_dir()?.join(FILE_NAME))
}

/// Starts recording finished transfers into `path`, or stops with `None`.
pub fn enable(path: Option<PathBuf>) {
    *target() = path;
}

/// Records into the default file unless the configuration turned history off.
pub fn enable_from_config(config: &crate::config::Config) {
    enable(
        (!config.disable_history)
            .then(default_path)
            .and_then(Result::ok),
    );
}

/// Appends the transfer of a final `event` when recording is on. Called for
/// every final event by the transfer tracker; failures are only logged.
pub fn record(event: &TransferEvent) {
    let Some(path) = target().clone() else {
        return;
    };
    let Some(entry) = HistoryEntry::from_event(event) else {
        return;
    };
    record_entry(&path, entry);
}

/// Records a text sent or received ([`HistoryEntry::text`]) when recording is on.
pub fn record_text(entry: HistoryEntry) {
    if let Some(path) = target().clone() {
        record_entry(&path, entry);
    }
}

/// Fills in the name of a trusted device and appends `entry`.
fn record_entry(path: &Path, mut entry: HistoryEntry) {
    if entry.peer_name.is_none() {
        entry.peer_name = crate::config::Config::load()
            .ok()
            .and_then(|c| c.find_peer(&entry.peer).map(|p| p.alias.clone()));
    }
    if let Err(e) = append(path, &entry) {
        tracing::warn!("Could not record the transfer in {}: {e:#}", path.display());
    }
}

/// Appends `entry` to `path`, trimming the file to [`MAX_ENTRIES`] now and then.
///
/// # Errors
///
/// Fails when the file cannot be written.
pub fn append(path: &Path, entry: &HistoryEntry) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    writeln!(file, "{}", serde_json::to_string(entry)?)?;
    drop(file);
    // About 300 bytes per line: trim when the file holds about twice the limit.
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > (MAX_ENTRIES as u64) * 600 {
        let mut entries = load(path)?;
        entries.truncate(MAX_ENTRIES);
        entries.reverse();
        let text: String = entries
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|line| line + "\n")
            .collect();
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(())
}

/// The transfers in `path`, newest first; a missing file is an empty history
/// and damaged lines are skipped.
///
/// # Errors
///
/// Fails when the file exists but cannot be read.
pub fn load(path: &Path) -> Result<Vec<HistoryEntry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut entries: Vec<HistoryEntry> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    entries.reverse();
    Ok(entries)
}

/// The entries whose name, device or detail contains `query` (any case); all
/// for an empty query.
pub fn search(entries: Vec<HistoryEntry>, query: &str) -> Vec<HistoryEntry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries;
    }
    entries.into_iter().filter(|e| e.matches(&query)).collect()
}

/// Deletes the history in `path`.
///
/// # Errors
///
/// Fails when the file exists but cannot be deleted.
pub fn clear(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::service::TransferEvent;

    fn event(name: &str, status: TransferStatus) -> TransferEvent {
        TransferEvent {
            id: "1".into(),
            peer: "abcd".into(),
            peer_name: Some("Kate's phone".into()),
            file_name: name.into(),
            direction: Direction::Download,
            total: 42,
            done: 42,
            status,
            route: None,
        }
    }

    #[test]
    fn only_final_events_are_entries() {
        assert!(HistoryEntry::from_event(&event("a", TransferStatus::Running)).is_none());
        assert!(HistoryEntry::from_event(&event("a", TransferStatus::Paused)).is_none());
        let done = HistoryEntry::from_event(&event(
            "a.jpg",
            TransferStatus::Completed {
                path: Some("/d/a.jpg".into()),
            },
        ))
        .unwrap();
        assert_eq!(done.status, "completed");
        assert_eq!(done.detail.as_deref(), Some("/d/a.jpg"));
        assert_eq!(done.size, 42);
    }

    #[test]
    fn entries_are_kept_newest_first_searched_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        assert!(load(&path).unwrap().is_empty(), "no file yet");
        for (name, status) in [
            ("holiday.jpg", TransferStatus::Completed { path: None }),
            (
                "report.pdf",
                TransferStatus::Rejected {
                    reason: "busy".into(),
                },
            ),
        ] {
            append(
                &path,
                &HistoryEntry::from_event(&event(name, status)).unwrap(),
            )
            .unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        let entries = load(&path).unwrap();
        assert_eq!(entries.len(), 2, "damaged lines are skipped");
        assert_eq!(entries[0].file_name, "report.pdf", "newest first");
        assert_eq!(search(entries.clone(), "HOLIDAY").len(), 1);
        assert_eq!(search(entries.clone(), "kate").len(), 2, "by device");
        assert_eq!(search(entries.clone(), "busy").len(), 1, "by reason");
        assert_eq!(search(entries, "  ").len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "only the owner reads it");
        }
        clear(&path).unwrap();
        assert!(load(&path).unwrap().is_empty());
        clear(&path).unwrap();
    }

    #[test]
    fn texts_are_recorded_without_their_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let sent = HistoryEntry::text(Direction::Upload, "abcd", 11, Ok(()));
        let refused = HistoryEntry::text(
            Direction::Download,
            "abcd",
            5,
            Err(("rejected", "no".into())),
        );
        append(&path, &sent).unwrap();
        append(&path, &refused).unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        assert!(line.contains(r#""kind":"text""#), "{line}");
        let entries = load(&path).unwrap();
        assert_eq!(entries[0].kind, HistoryKind::Text);
        assert_eq!(
            (entries[0].size, entries[0].status.as_str()),
            (5, "rejected")
        );
        assert_eq!(entries[1].file_name, "");
        // Files stay as before: no `kind` in the file, read back as a file.
        let file = HistoryEntry::from_event(&event("a", TransferStatus::Cancelled)).unwrap();
        assert!(!serde_json::to_string(&file).unwrap().contains("kind"));
        let old = r#"{"time":"2026-11-01T10:00:00Z","direction":"upload","peer":"p","file_name":"a","size":1,"status":"completed"}"#;
        assert_eq!(
            serde_json::from_str::<HistoryEntry>(old).unwrap().kind,
            HistoryKind::File
        );
    }

    #[test]
    fn the_file_is_trimmed_to_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut entry = HistoryEntry::from_event(&event(
            "x",
            TransferStatus::Failed {
                error: "e".repeat(400),
            },
        ))
        .unwrap();
        for i in 0..(MAX_ENTRIES + 600) {
            entry.file_name = format!("file-{i}");
            append(&path, &entry).unwrap();
        }
        let entries = load(&path).unwrap();
        assert!(entries.len() <= MAX_ENTRIES + 600 && entries.len() >= MAX_ENTRIES);
        assert_eq!(entries[0].file_name, format!("file-{}", MAX_ENTRIES + 599));
    }
}
