use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum SecurityError {
    #[error("Wykryto próbę ataku Path Traversal: {0}")]
    PathTraversalAttempt(String),
    #[error("Niedozwolona lub pusta nazwa pliku")]
    InvalidFileName,
    #[error("Ścieżka wychodzi poza dozwolony katalog bazowy: {0}")]
    OutsideBaseDirectory(String),
    #[error("Błąd I/O podczas sprawdzania kanoniczności ścieżki: {0}")]
    IoError(String),
}

/// Nazwy zarezerwowane przez Windows (niezależnie od rozszerzenia).
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub fn sanitize_filename(raw_name: &str) -> Result<String, SecurityError> {
    let trimmed = raw_name.trim();
    if trimmed.is_empty() {
        return Err(SecurityError::InvalidFileName);
    }

    let raw_path = Path::new(trimmed);
    for component in raw_path.components() {
        match component {
            Component::ParentDir => {
                warn!("Wykryto sekwencję '..' w nazwie pliku: {}", raw_name);
                return Err(SecurityError::PathTraversalAttempt(raw_name.to_string()));
            }
            Component::RootDir | Component::Prefix(_) => {
                warn!("Wykryto ścieżkę bezwzględną w nazwie pliku: {}", raw_name);
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

/// Zwraca ścieżkę, która jeszcze nie istnieje: `plik.txt`, `plik (1).txt`, `plik (2).txt`…
/// Odebrany plik nigdy nie nadpisuje istniejącego.
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
            assert!(sanitize_filename(bad).is_err(), "przepuszczono {bad:?}");
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
