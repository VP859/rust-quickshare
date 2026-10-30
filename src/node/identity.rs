//! Persistent Ed25519 node identity.
//!
//! The Node ID is the public key of a secret key stored in
//! `<config dir>/secret.key`. Keeping it stable across restarts is what makes
//! trusted devices and Auto-Accept work.

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use iroh::SecretKey;
use rust_i18n::t;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;

/// Path of the persistent secret key: `<config dir>/secret.key`.
///
/// # Errors
///
/// Fails when the configuration directory cannot be determined or created.
pub fn default_key_path() -> Result<PathBuf> {
    Ok(crate::config::paths::config_dir()?.join("secret.key"))
}

/// Loads the persistent secret key, generating and saving it on first use.
///
/// # Errors
///
/// See [`load_or_generate_secret_key`].
pub fn load_or_generate_default_key() -> Result<SecretKey> {
    load_or_generate_secret_key(Some(&default_key_path()?))
}

/// Loads the key stored at `path`, or generates a new one and stores it there.
///
/// With `None` an ephemeral key is generated (tests, one-off transfers). New
/// key files are created with `0600` permissions on Unix.
///
/// # Errors
///
/// Fails when the file cannot be read or written, or does not hold exactly
/// 32 bytes.
pub fn load_or_generate_secret_key(path: Option<&Path>) -> Result<SecretKey> {
    if let Some(p) = path
        && p.exists()
    {
        let bytes = std::fs::read(p).with_context(|| t!("identity.read", path = p.display()))?;
        let key_array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!(t!("identity.bad_size")))?;
        info!("Loaded node key from {}", p.display());
        return Ok(SecretKey::from(key_array));
    }

    let secret_key = SecretKey::generate();
    info!("Generated a new node key (Node ID {})", secret_key.public());

    if let Some(p) = path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_private(p, &secret_key.to_bytes())
            .with_context(|| t!("identity.write", path = p.display()))?;
    }
    Ok(secret_key)
}

/// Prefix of the key line in an exported identity file.
const EXPORT_PREFIX: &str = "ferry-identity-v1:";

/// Text of an identity backup: a warning comment and the key in base64.
pub fn export_identity(key: &SecretKey) -> String {
    format!(
        "# Ferry identity of Node ID {}\n\
         # KEEP THIS FILE SECRET: anyone who has it can pose as this device to its\n\
         # paired devices. Restore it with `ferry identity import <file>`.\n\
         {EXPORT_PREFIX}{}\n",
        key.public(),
        STANDARD.encode(key.to_bytes())
    )
}

/// Reads a key from the text written by [`export_identity`]; lines starting
/// with `#` are ignored.
///
/// # Errors
///
/// Fails when the text holds no valid key line.
pub fn parse_identity(text: &str) -> Result<SecretKey> {
    let encoded = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(EXPORT_PREFIX))
        .with_context(|| t!("identity.bad_export"))?;
    let bytes: [u8; 32] = STANDARD
        .decode(encoded.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .with_context(|| t!("identity.bad_export"))?;
    Ok(SecretKey::from(bytes))
}

/// Writes an identity backup to a new file readable only by the owner.
///
/// # Errors
///
/// Fails when the file already exists or cannot be written.
pub fn write_identity_export(path: &Path, key: &SecretKey) -> Result<()> {
    write_private(path, export_identity(key).as_bytes())
        .with_context(|| t!("identity.write", path = path.display()))
}

/// Replaces the key stored at `path` with `key`. An existing key file is kept
/// as `<path>.<unix time>.bak`, whose path is returned.
///
/// # Errors
///
/// Fails when the old key cannot be moved or the new one cannot be written.
pub fn replace_secret_key(path: &Path, key: &SecretKey) -> Result<Option<PathBuf>> {
    let backup = if path.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{stamp}.bak"));
        let backup = PathBuf::from(name);
        std::fs::rename(path, &backup)
            .with_context(|| t!("identity.write", path = backup.display()))?;
        Some(backup)
    } else {
        None
    };
    write_private(path, &key.to_bytes())
        .with_context(|| t!("identity.write", path = path.display()))?;
    Ok(backup)
}

/// Creates a new file readable only by the owner (`0600` on Unix) and syncs it to disk.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_across_loads() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("secret.key");
        let a = load_or_generate_secret_key(Some(&path))?;
        let b = load_or_generate_secret_key(Some(&path))?;
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o600
            );
        }
        Ok(())
    }

    #[test]
    fn export_round_trip_and_replace() -> Result<()> {
        let key = SecretKey::generate();
        let text = export_identity(&key);
        assert!(text.contains(&key.public().to_string()));
        assert_eq!(parse_identity(&text)?.public(), key.public());
        assert!(parse_identity("# nothing here").is_err());
        assert!(parse_identity("ferry-identity-v1:AAAA").is_err());

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("secret.key");
        assert!(replace_secret_key(&path, &key)?.is_none());
        let other = SecretKey::generate();
        let backup = replace_secret_key(&path, &other)?.expect("old key kept");
        assert_eq!(
            load_or_generate_secret_key(Some(&path))?.public(),
            other.public()
        );
        assert_eq!(
            load_or_generate_secret_key(Some(&backup))?.public(),
            key.public()
        );
        Ok(())
    }
}
