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
        .map(|c| if illegal_chars.contains(&c) { '_' } else { c })
        .collect();

    let final_name = sanitized.trim_start_matches('.').to_string();
    if final_name.is_empty() {
        return Err(SecurityError::InvalidFileName);
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
