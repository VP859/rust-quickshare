//! BLAKE3 hashing of whole files.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::path::Path;

/// Computes the BLAKE3 hash of the file at `path` on a blocking thread.
///
/// Uses memory mapping with multi-threaded hashing when possible and falls back
/// to streaming reads otherwise.
///
/// # Errors
///
/// Fails when the file cannot be opened or read.
pub async fn compute_file_hash(path: &Path) -> Result<[u8; 32]> {
    let path_buf = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if let Ok(hasher) = blake3::Hasher::new().update_mmap_rayon(&path_buf) {
            return Ok(*hasher.finalize().as_bytes());
        }
        let mut file = std::fs::File::open(&path_buf)
            .with_context(|| t!("hash.open", path = path_buf.display()))?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(*hasher.finalize().as_bytes())
    })
    .await
    .with_context(|| t!("hash.task"))?
}

/// BLAKE3 state after hashing the first `len` bytes of the file at `path`, so a
/// resumed download can keep hashing incrementally from there.
///
/// # Errors
///
/// Fails when the file cannot be opened or holds fewer than `len` bytes.
pub async fn hash_prefix(path: &Path, len: u64) -> Result<blake3::Hasher> {
    let path_buf = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path_buf)
            .with_context(|| t!("hash.open", path = path_buf.display()))?;
        let mut hasher = blake3::Hasher::new();
        let copied = std::io::copy(&mut std::io::Read::take(file, len), &mut hasher)?;
        anyhow::ensure!(copied == len, t!("hash.open", path = path_buf.display()));
        Ok(hasher)
    })
    .await
    .with_context(|| t!("hash.task"))?
}
