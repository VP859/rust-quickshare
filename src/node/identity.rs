//! Persistent Ed25519 node identity.
//!
//! The Node ID is the public key of a secret key stored in
//! `<config dir>/secret.key`. Keeping it stable across restarts is what makes
//! trusted devices and Auto-Accept work.

use anyhow::{Context, Result};
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
}
