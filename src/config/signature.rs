//! Finding changes to `config.toml` made outside Ferry
//! ([`Config::sign_config`](super::Config::sign_config)).
//!
//! Whenever Ferry saves the file, it writes next to it (`config.toml.sig`) a
//! keyed BLAKE3 hash of its contents. The key is kept in the system's secret
//! store (Secret Service – KWallet, GNOME Keyring – on Linux, the Credential
//! Manager on Windows), not in the configuration folder, so a program that
//! changes the file cannot write a matching signature without also reading
//! the secret store. The check ([`super::check`]) then tells a file Ferry
//! wrote from one changed since – by the user in an editor, or by someone or
//! something else.
//!
//! The format of `config.toml` does not change, and signing never stops a
//! save: when the secret store cannot be reached (a daemon without a desktop
//! session), the file is saved unsigned and the old signature removed, which
//! the check reports.

use crate::node::keystore::{self, SecretSlot};
use anyhow::Result;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Extension of the signature next to `config.toml`.
pub const EXTENSION: &str = "toml.sig";

/// The signing key of a configuration folder, read from the secret store
/// once per process (reading it may ask to unlock the wallet).
static KEY: Mutex<Option<(PathBuf, [u8; 32])>> = Mutex::new(None);

/// What the check finds about the signature of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The file is as Ferry last saved it.
    Unchanged,
    /// The file was changed since Ferry last saved it.
    Changed,
    /// There is no signature: it was removed, or Ferry saved the file without
    /// reaching the secret store.
    Unsigned,
    /// The secret store has no signing key (any more).
    NoKey,
    /// The secret store cannot be reached; the reason.
    Unavailable(String),
}

/// The signature of `content`.
fn sign(key: &[u8; 32], content: &[u8]) -> String {
    blake3::keyed_hash(key, content).to_hex().to_string()
}

/// The key in `slot`; a new one is made and stored when there is none and
/// `create` is set.
fn key_in(slot: &dyn SecretSlot, create: bool) -> Result<Option<[u8; 32]>> {
    match slot.get()? {
        Some(secret) => keystore::key_bytes(secret).map(Some),
        None if create => {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("{e}"))?;
            keystore::store_verified(slot, &key)?;
            Ok(Some(key))
        }
        None => Ok(None),
    }
}

/// The signing key of the configuration in `dir`, from the cache or the
/// secret store. The store is asked on a thread of its own: its D-Bus client
/// must not block a thread of an async runtime the caller may run on.
fn key(dir: &Path, create: bool) -> Result<Option<[u8; 32]>> {
    if let Ok(cache) = KEY.lock()
        && let Some((cached, key)) = cache.as_ref()
        && cached == dir
    {
        return Ok(Some(*key));
    }
    let owned = dir.to_path_buf();
    let found = std::thread::spawn(move || {
        let slot = keystore::config_signing_slot(&owned)?;
        key_in(slot.as_ref(), create)
    })
    .join()
    .map_err(|_| anyhow::anyhow!(t!("keystore.verify_failed")))??;
    if let (Some(key), Ok(mut cache)) = (found, KEY.lock()) {
        *cache = Some((dir.to_path_buf(), key));
    }
    Ok(found)
}

/// Makes sure the signing key exists in the secret store, before signing is
/// turned on.
///
/// # Errors
///
/// When the secret store cannot be reached or the key cannot be stored.
pub fn prepare() -> Result<()> {
    key(&super::paths::config_dir()?, true).map(|_| ())
}

/// Removes the signing key from the secret store, after signing was turned
/// off (turning it on again makes a new one).
///
/// # Errors
///
/// When the secret store cannot be reached.
pub fn forget() -> Result<()> {
    let dir = super::paths::config_dir()?;
    if let Ok(mut cache) = KEY.lock() {
        *cache = None;
    }
    std::thread::spawn(move || keystore::config_signing_slot(&dir)?.delete())
        .join()
        .map_err(|_| anyhow::anyhow!(t!("keystore.verify_failed")))?
}

/// After `config.toml` at `path` was written with `content`: writes its
/// signature when signing is `enabled`, otherwise removes an old one. Never
/// fails a save; without the secret store the file stays unsigned.
pub(super) fn after_write(path: &Path, content: &[u8], enabled: bool) {
    let signature = path.with_extension(EXTENSION);
    if !enabled {
        if signature.exists() {
            let _ = std::fs::remove_file(&signature);
        }
        return;
    }
    let written = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no folder"))
        .and_then(|dir| key(dir, true))
        .and_then(|key| {
            let key = key.ok_or_else(|| anyhow::anyhow!(t!("keystore.verify_failed")))?;
            std::fs::write(&signature, sign(&key, content))?;
            Ok(())
        });
    if let Err(e) = written {
        tracing::warn!("config.toml saved without a signature: {e:#}");
        let _ = std::fs::remove_file(&signature);
    }
}

/// Whether `content` matches `signature` under `key`.
fn compare(content: &[u8], signature: Option<&str>, key: Result<Option<[u8; 32]>>) -> Status {
    let Some(signature) = signature else {
        return Status::Unsigned;
    };
    match key {
        Err(e) => Status::Unavailable(format!("{e:#}")),
        Ok(None) => Status::NoKey,
        Ok(Some(key)) if sign(&key, content) == signature.trim() => Status::Unchanged,
        Ok(Some(_)) => Status::Changed,
    }
}

/// Checks the signature of `config.toml` at `path` (whose bytes are
/// `content`); never creates a key.
pub fn status(path: &Path, content: &[u8]) -> Status {
    let signature = std::fs::read_to_string(path.with_extension(EXTENSION)).ok();
    if signature.is_none() {
        return Status::Unsigned;
    }
    let key = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no folder"))
        .and_then(|dir| key(dir, false));
    compare(content, signature.as_deref(), key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::keystore::tests::MemorySlot;

    #[test]
    fn a_signature_finds_any_change_and_needs_the_key() {
        let slot = MemorySlot::default();
        assert_eq!(key_in(&slot, false).unwrap(), None, "made only when asked");
        let key = key_in(&slot, true).unwrap().unwrap();
        assert_eq!(
            key_in(&slot, false).unwrap(),
            Some(key),
            "kept in the store"
        );

        let file = b"paired_only = true\n";
        let signature = sign(&key, file);
        assert_eq!(
            compare(file, Some(&signature), Ok(Some(key))),
            Status::Unchanged
        );
        assert_eq!(
            compare(b"paired_only = false\n", Some(&signature), Ok(Some(key))),
            Status::Changed
        );
        // Another key (a program that wrote its own signature) does not match.
        let other = sign(&[7; 32], b"paired_only = false\n");
        assert_eq!(
            compare(b"paired_only = false\n", Some(&other), Ok(Some(key))),
            Status::Changed
        );
        assert_eq!(compare(file, None, Ok(Some(key))), Status::Unsigned);
        assert_eq!(compare(file, Some(&signature), Ok(None)), Status::NoKey);
        assert!(matches!(
            compare(file, Some(&signature), Err(anyhow::anyhow!("no session"))),
            Status::Unavailable(_)
        ));
    }

    /// Without signing, saving removes an old signature, so it never goes stale.
    #[test]
    fn turning_signing_off_removes_the_signature() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let signature = path.with_extension(EXTENSION);
        std::fs::write(&signature, "old").unwrap();
        after_write(&path, b"", false);
        assert!(!signature.exists());
        assert_eq!(status(&path, b""), Status::Unsigned);
    }
}
