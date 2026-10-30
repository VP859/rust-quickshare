//! Sending half of the transfer protocol.

use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::task::JoinSet;
use tracing::info;

use super::{Channel, ProgressFn};
use crate::protocol::codec::{read_msg, write_msg};
use crate::protocol::messages::{
    CHUNK_WINDOW, ChunkHeader, DEFAULT_CHUNK_SIZE, Hello, MAX_FOLDER_DEPTH, TransferDecision,
    TransferOffer, TransferResult,
};
use crate::transfer::hash::compute_file_hash;
use crate::transfer::security::sanitize_filename;

/// A file ready to be sent, with its BLAKE3 hash computed once up front.
#[derive(Debug, Clone)]
pub struct PreparedFile {
    /// Local path of the file.
    pub path: PathBuf,
    /// Sanitized file name announced to the receiver.
    pub name: String,
    /// File size in bytes.
    pub size: u64,
    /// BLAKE3 hash of the whole file.
    pub hash: [u8; 32],
    /// Folders the receiver saves the file in ([`TransferOffer::folder`]).
    pub folder: Vec<String>,
}

/// Largest number of files sent from the folders of one `send` request.
pub const MAX_FOLDER_FILES: usize = 10_000;

/// A file to send and the folders it belongs in on the receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendItem {
    /// Local path of the file.
    pub path: PathBuf,
    /// Relative folder, starting with the name of the folder that was chosen.
    pub folder: Vec<String>,
}

/// Turns the chosen paths into files to send: files stay as they are, folders
/// are walked recursively (sorted, without following symbolic links) and keep
/// their structure, starting with the folder's own name.
///
/// # Errors
///
/// Fails when a path cannot be read, a folder is deeper than
/// [`MAX_FOLDER_DEPTH`] or holds more than [`MAX_FOLDER_FILES`] files.
pub fn expand_paths(paths: &[PathBuf]) -> Result<Vec<SendItem>> {
    let mut items = Vec::new();
    for path in paths {
        let metadata =
            std::fs::metadata(path).with_context(|| t!("sender.read", path = path.display()))?;
        if metadata.is_dir() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "folder".to_string());
            walk_folder(path, vec![name], &mut items)?;
        } else {
            items.push(SendItem {
                path: path.clone(),
                folder: Vec::new(),
            });
        }
    }
    Ok(items)
}

/// Adds the files of `dir` (saved under `folder`) and its subfolders to `items`.
fn walk_folder(dir: &Path, folder: Vec<String>, items: &mut Vec<SendItem>) -> Result<()> {
    if folder.len() > MAX_FOLDER_DEPTH {
        anyhow::bail!(t!(
            "sender.too_deep",
            path = dir.display(),
            max = MAX_FOLDER_DEPTH
        ));
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| t!("sender.read", path = dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        // `file_type` does not follow links: linked files and folders are skipped,
        // so a link cannot pull in data from outside the chosen folder.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let mut sub = folder.clone();
            sub.push(entry.file_name().to_string_lossy().into_owned());
            walk_folder(&entry.path(), sub, items)?;
        } else if kind.is_file() {
            if items.len() >= MAX_FOLDER_FILES {
                anyhow::bail!(t!("sender.too_many_files", max = MAX_FOLDER_FILES));
            }
            items.push(SendItem {
                path: entry.path(),
                folder: folder.clone(),
            });
        }
    }
    Ok(())
}

/// Reads metadata of the file at `path` and computes its hash.
///
/// # Errors
///
/// Fails when the path does not exist, is not a regular file or cannot be read.
pub async fn prepare_file(path: &Path) -> Result<PreparedFile> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| t!("sender.read", path = path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!(t!("sender.not_a_file", path = path.display()));
    }
    let raw_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = sanitize_filename(&raw_name).unwrap_or_else(|_| "file".to_string());
    let hash = compute_file_hash(path).await?;
    Ok(PreparedFile {
        path: path.to_path_buf(),
        name,
        size: metadata.len(),
        hash,
        folder: Vec::new(),
    })
}

/// How a send attempt ended (errors are reported through `Result`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// The receiver saved the file and verified its hash.
    Delivered,
    /// The receiver declined the offer with the given reason.
    Rejected(String),
}

/// Opens a new control stream with [`Hello::Push`] and sends `file` over it.
///
/// # Errors
///
/// See [`send_on_channel`].
pub async fn push_file(
    conn: &Connection,
    file: &PreparedFile,
    progress: ProgressFn<'_>,
) -> Result<SendOutcome> {
    let mut channel = Channel::open(conn, &Hello::Push).await?;
    let outcome = send_on_channel(conn, &mut channel, file, progress).await?;
    channel.finish().await;
    Ok(outcome)
}

/// Runs the sending side of the protocol on an already opened control stream.
///
/// Sends the offer, waits for the decision, streams the remaining chunks on
/// separate unidirectional streams (up to [`CHUNK_WINDOW`] in flight) and waits
/// for the receiver's verdict. Chunk buffers are handed over to QUIC without
/// copying.
///
/// # Errors
///
/// Fails on network or I/O errors, when the receiver asks for a chunk past the
/// end of the file, or when the receiver reports a failed verification.
pub async fn send_on_channel(
    conn: &Connection,
    channel: &mut Channel,
    file: &PreparedFile,
    progress: ProgressFn<'_>,
) -> Result<SendOutcome> {
    let offer = TransferOffer {
        file_name: file.name.clone(),
        file_size: file.size,
        chunk_size: DEFAULT_CHUNK_SIZE,
        file_hash: file.hash,
        folder: file
            .folder
            .iter()
            .map(|c| sanitize_filename(c).unwrap_or_else(|_| "folder".to_string()))
            .collect(),
    };
    write_msg(&mut channel.send, &offer).await?;

    let start_chunk_index = match read_msg(&mut channel.recv).await? {
        TransferDecision::Accept { start_chunk_index } => start_chunk_index,
        TransferDecision::Reject { reason } => return Ok(SendOutcome::Rejected(reason)),
    };
    let total_chunks = offer.total_chunks();
    if start_chunk_index > total_chunks {
        anyhow::bail!(t!("sender.bad_start", index = start_chunk_index));
    }
    info!(
        "Sending '{}' from chunk {}/{}",
        file.name, start_chunk_index, total_chunks
    );

    let mut source = File::open(&file.path).await?;
    let start_offset = start_chunk_index * offer.chunk_size as u64;
    source.seek(SeekFrom::Start(start_offset)).await?;
    let mut done = start_offset.min(file.size);
    progress(done);

    let mut in_flight: JoinSet<Result<u32>> = JoinSet::new();
    for chunk_index in start_chunk_index..total_chunks {
        while in_flight.len() >= CHUNK_WINDOW {
            done += join_next(&mut in_flight).await? as u64;
            progress(done);
        }

        let len = offer
            .chunk_len(chunk_index)
            .with_context(|| t!("transfer.chunk_len"))?;
        let mut payload = vec![0u8; len as usize];
        source
            .read_exact(&mut payload)
            .await
            .with_context(|| t!("sender.file_changed"))?;

        let mut stream = conn
            .open_uni()
            .await
            .with_context(|| t!("sender.open_stream"))?;
        in_flight.spawn(async move {
            let header = ChunkHeader {
                chunk_index,
                payload_len: len,
                chunk_hash: *blake3::hash(&payload).as_bytes(),
            };
            write_msg(&mut stream, &header).await?;
            stream.write_chunk(bytes::Bytes::from(payload)).await?;
            stream.finish()?;
            Ok(len)
        });
    }
    while !in_flight.is_empty() {
        done += join_next(&mut in_flight).await? as u64;
        progress(done);
    }

    match read_msg(&mut channel.recv).await? {
        TransferResult::Verified => Ok(SendOutcome::Delivered),
        TransferResult::Failed { reason } => {
            anyhow::bail!(t!("sender.verify_failed", reason = reason))
        }
    }
}

/// Waits for the next finished chunk task and returns the bytes it sent.
async fn join_next(set: &mut JoinSet<Result<u32>>) -> Result<u32> {
    set.join_next()
        .await
        .with_context(|| t!("transfer.task_aborted"))?
        .with_context(|| t!("transfer.task_aborted"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_expanded_with_their_structure() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("Photos");
        std::fs::create_dir_all(root.join("2024/summer"))?;
        std::fs::write(root.join("b.jpg"), b"b")?;
        std::fs::write(root.join("2024/summer/a.jpg"), b"a")?;
        let single = dir.path().join("single.txt");
        std::fs::write(&single, b"s")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", root.join("link"))?;

        let items = expand_paths(&[root.clone(), single.clone()])?;
        let folder = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(
            items,
            vec![
                SendItem {
                    path: root.join("2024/summer/a.jpg"),
                    folder: folder(&["Photos", "2024", "summer"]),
                },
                SendItem {
                    path: root.join("b.jpg"),
                    folder: folder(&["Photos"]),
                },
                SendItem {
                    path: single,
                    folder: Vec::new(),
                },
            ],
            "sorted, links skipped"
        );
        Ok(())
    }
}
