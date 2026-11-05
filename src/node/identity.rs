//! Persistent Ed25519 node identity.
//!
//! The Node ID is the public key of a secret key stored in
//! `<config dir>/secret.key`, or in the system's secret store when chosen
//! ([`KeyStorage`]). Keeping it stable across
//! restarts is what makes trusted devices and Auto-Accept work.

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use iroh::SecretKey;
use rust_i18n::t;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;

use super::keystore::{self, KeyStorage, SecretSlot};

/// Path of the persistent secret key: `<config dir>/secret.key`.
///
/// # Errors
///
/// Fails when the configuration directory cannot be determined or created.
pub fn default_key_path() -> Result<PathBuf> {
    Ok(crate::config::paths::config_dir()?.join("secret.key"))
}

/// Loads the persistent secret key from where the configuration keeps it,
/// generating and saving it on first use.
///
/// # Errors
///
/// See [`load_or_generate_secret_key`]; with the system's secret store, also
/// when it cannot be reached. A new key is then **not** generated, since that
/// would give the device another identity.
pub fn load_or_generate_default_key() -> Result<SecretKey> {
    let config = crate::config::Config::load().unwrap_or_default();
    let path = default_key_path()?;
    match config.key_storage {
        KeyStorage::File => load_or_generate_secret_key(Some(&path)),
        KeyStorage::System => {
            let slot = keystore::system_slot(&crate::config::paths::config_dir()?)?;
            load_from_slot(slot.as_ref(), &path)
        }
    }
}

/// The key in `slot`; a new one is generated and stored when there is none
/// and no key file exists either (a file next to an empty store means the
/// configuration was changed by hand: refuse rather than replace the key).
fn load_from_slot(slot: &dyn SecretSlot, file: &Path) -> Result<SecretKey> {
    if let Some(secret) = slot.get()? {
        info!("Loaded node key from the system's secret store");
        return Ok(SecretKey::from(keystore::key_bytes(secret)?));
    }
    if file.exists() {
        anyhow::bail!(t!("keystore.missing", path = file.display()));
    }
    let key = SecretKey::generate();
    keystore::store_verified(slot, &key.to_bytes())?;
    info!(
        "Generated a new node key in the system's secret store (Node ID {})",
        key.public()
    );
    Ok(key)
}

/// Moves the device key to `target`; nothing when it is already there.
///
/// The key is written to the new place and read back, then the configuration
/// is switched, and only then the old copy is removed (the file is
/// overwritten first). An interruption leaves a working key in one place.
///
/// # Errors
///
/// Fails when the secret store cannot be used, or a key file that differs
/// from the stored key is in the way.
pub fn set_key_storage(target: KeyStorage) -> Result<()> {
    let mut config = crate::config::Config::load()?;
    if config.key_storage == target {
        return Ok(());
    }
    let slot = keystore::system_slot(&crate::config::paths::config_dir()?)?;
    let path = default_key_path()?;
    move_key(slot.as_ref(), &path, target, |storage| {
        config.key_storage = storage;
        config.save()
    })
}

/// [`set_key_storage`] with the store, the key file and how to save the
/// choice given.
fn move_key(
    slot: &dyn SecretSlot,
    file: &Path,
    target: KeyStorage,
    save: impl FnOnce(KeyStorage) -> Result<()>,
) -> Result<()> {
    match target {
        KeyStorage::System => {
            let key = load_or_generate_secret_key(Some(file))?;
            keystore::store_verified(slot, &key.to_bytes())?;
            save(KeyStorage::System)?;
            keystore::erase_file(file)?;
            info!("Node key moved to the system's secret store");
        }
        KeyStorage::File => {
            let key = slot
                .get()?
                .map(keystore::key_bytes)
                .transpose()?
                .context(t!("keystore.empty"))?;
            if file.exists() {
                let existing = load_or_generate_secret_key(Some(file))?;
                if existing.to_bytes() != key {
                    anyhow::bail!(t!("keystore.file_differs", path = file.display()));
                }
            } else {
                if let Some(parent) = file.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_private(file, &key)
                    .with_context(|| t!("identity.write", path = file.display()))?;
            }
            if load_or_generate_secret_key(Some(file))?.to_bytes() != key {
                anyhow::bail!(t!("keystore.verify_failed"));
            }
            save(KeyStorage::File)?;
            slot.delete()?;
            info!("Node key moved to {}", file.display());
        }
    }
    Ok(())
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
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
    use crate::node::keystore::tests::MemorySlot;

    #[test]
    fn the_key_moves_to_the_store_and_back_unchanged() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("secret.key");
        let original = load_or_generate_secret_key(Some(&file))?;
        let slot = MemorySlot::default();
        let saved = std::cell::Cell::new(KeyStorage::File);

        move_key(&slot, &file, KeyStorage::System, |s| {
            saved.set(s);
            Ok(())
        })?;
        assert_eq!(saved.get(), KeyStorage::System);
        assert!(!file.exists(), "the file is gone");
        let loaded = load_from_slot(&slot, &file)?;
        assert_eq!(loaded.public(), original.public(), "same identity");

        move_key(&slot, &file, KeyStorage::File, |s| {
            saved.set(s);
            Ok(())
        })?;
        assert_eq!(saved.get(), KeyStorage::File);
        assert!(slot.get()?.is_none(), "the store is emptied");
        assert_eq!(
            load_or_generate_secret_key(Some(&file))?.public(),
            original.public()
        );
        Ok(())
    }

    #[test]
    fn a_failed_switch_keeps_the_key_where_it_was() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("secret.key");
        let original = load_or_generate_secret_key(Some(&file))?;
        let slot = MemorySlot::default();
        // Saving the configuration fails: the file must still be there.
        let result = move_key(&slot, &file, KeyStorage::System, |_| {
            anyhow::bail!("disk full")
        });
        assert!(result.is_err());
        assert_eq!(
            load_or_generate_secret_key(Some(&file))?.public(),
            original.public()
        );
        Ok(())
    }

    #[test]
    fn an_empty_store_never_replaces_an_existing_key_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("secret.key");
        let slot = MemorySlot::default();
        load_or_generate_secret_key(Some(&file))?;
        assert!(load_from_slot(&slot, &file).is_err(), "no new identity");
        // A new device with nothing yet: the key is created in the store.
        std::fs::remove_file(&file)?;
        let key = load_from_slot(&slot, &file)?;
        assert_eq!(load_from_slot(&slot, &file)?.public(), key.public());
        assert!(!file.exists());
        // A different key file in the way of moving back is refused.
        std::fs::write(&file, [1u8; 32])?;
        assert!(move_key(&slot, &file, KeyStorage::File, |_| Ok(())).is_err());
        Ok(())
    }

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
