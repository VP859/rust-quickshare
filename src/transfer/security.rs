//! Protection against malicious file names sent by remote peers.
//!
//! A received file name is never used as a path: it is reduced to a single
//! safe file name, joined with the download folder and checked to stay inside it.

use rust_i18n::t;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use tracing::warn;

/// Reasons why a received file name or path was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum SecurityError {
    /// The name contains `..`, path separators or an absolute path.
    PathTraversalAttempt(String),
    /// The name is empty or consists only of forbidden characters.
    InvalidFileName,
    /// The resolved path points outside the download folder.
    OutsideBaseDirectory(String),
    /// An I/O error while preparing or canonicalizing the download folder.
    IoError(String),
}

impl fmt::Display for SecurityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            SecurityError::PathTraversalAttempt(name) => t!("security.traversal", name = name),
            SecurityError::InvalidFileName => t!("security.invalid_name"),
            SecurityError::OutsideBaseDirectory(path) => t!("security.outside", path = path),
            SecurityError::IoError(error) => t!("security.io", error = error),
        };
        f.write_str(&message)
    }
}

impl std::error::Error for SecurityError {}

/// Names reserved by Windows regardless of extension.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turns a file name received from a peer into a safe single file name.
///
/// Rejects path traversal and absolute paths, replaces characters that are
/// invalid on common file systems with `_`, strips leading dots and trailing
/// dots/spaces, and prefixes Windows reserved names with `_`.
///
/// # Errors
///
/// [`SecurityError::PathTraversalAttempt`] or [`SecurityError::InvalidFileName`].
///
/// # Examples
///
/// ```
/// use rqs_p2p::transfer::security::sanitize_filename;
///
/// assert_eq!(sanitize_filename("report.pdf").unwrap(), "report.pdf");
/// assert_eq!(sanitize_filename("CON.txt").unwrap(), "_CON.txt");
/// assert!(sanitize_filename("../../.bashrc").is_err());
/// ```
pub fn sanitize_filename(raw_name: &str) -> Result<String, SecurityError> {
    let trimmed = raw_name.trim();
    if trimmed.is_empty() {
        return Err(SecurityError::InvalidFileName);
    }

    for component in Path::new(trimmed).components() {
        match component {
            Component::ParentDir => {
                warn!("Rejected file name containing '..': {raw_name}");
                return Err(SecurityError::PathTraversalAttempt(raw_name.to_string()));
            }
            Component::RootDir | Component::Prefix(_) => {
                warn!("Rejected absolute path as file name: {raw_name}");
                return Err(SecurityError::PathTraversalAttempt(raw_name.to_string()));
            }
            _ => {}
        }
    }

    if trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\') {
        return Err(SecurityError::PathTraversalAttempt(raw_name.to_string()));
    }

    let illegal_chars = ['\\', '/', ':', '*', '?', '"', '<', '>', '|', '\0'];
    let sanitized: String = trimmed
        .chars()
        .map(|c| {
            if illegal_chars.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();

    let final_name = sanitized
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .to_string();
    if final_name.is_empty() {
        return Err(SecurityError::InvalidFileName);
    }

    let stem = final_name.split('.').next().unwrap_or_default();
    if WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(stem))
    {
        return Ok(format!("_{final_name}"));
    }

    Ok(final_name)
}

/// Joins a sanitized `raw_file_name` with `base_dir` and verifies the result
/// stays inside `base_dir`. Creates `base_dir` when it does not exist.
///
/// # Errors
///
/// Any [`SecurityError`] from [`sanitize_filename`], plus
/// [`SecurityError::IoError`] and [`SecurityError::OutsideBaseDirectory`].
pub fn resolve_secure_target_path(
    base_dir: &Path,
    raw_file_name: &str,
) -> Result<PathBuf, SecurityError> {
    let safe_name = sanitize_filename(raw_file_name)?;

    if !base_dir.exists() {
        std::fs::create_dir_all(base_dir).map_err(|e| SecurityError::IoError(e.to_string()))?;
    }

    let canonical_base = base_dir
        .canonicalize()
        .map_err(|e| SecurityError::IoError(e.to_string()))?;

    let target_path = canonical_base.join(&safe_name);
    if !target_path.starts_with(&canonical_base) {
        return Err(SecurityError::OutsideBaseDirectory(
            target_path.display().to_string(),
        ));
    }
    Ok(target_path)
}

/// Returns a path that does not exist yet: `file.txt`, `file (1).txt`, `file (2).txt`, …
///
/// Received files therefore never overwrite existing ones.
pub fn unique_destination(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1u32..)
        .map(|n| parent.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_traversal_and_absolute_paths() {
        for bad in [
            "../etc/passwd",
            "/etc/passwd",
            "a/b",
            "a\\b",
            "..",
            "...",
            "",
        ] {
            assert!(sanitize_filename(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn sanitizes_special_names() {
        assert_eq!(sanitize_filename("CON.txt").unwrap(), "_CON.txt");
        assert_eq!(sanitize_filename(".bashrc").unwrap(), "bashrc");
        assert_eq!(sanitize_filename("a\nb:c.txt ").unwrap(), "a_b_c.txt");
        assert_eq!(sanitize_filename("zdjęcie.jpg").unwrap(), "zdjęcie.jpg");
    }

    #[test]
    fn unique_destination_never_overwrites() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = dir.path().join("a.txt");
        assert_eq!(unique_destination(&p), p);
        std::fs::write(&p, b"x")?;
        assert_eq!(unique_destination(&p), dir.path().join("a (1).txt"));
        Ok(())
    }
}
