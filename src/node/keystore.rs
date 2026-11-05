//! Where the device key is kept: a file in the configuration folder (the
//! default), or the system's secret store on the desktop (Secret Service –
//! KWallet, GNOME Keyring – on Linux, the Credential Manager, protected by
//! DPAPI, on Windows).
//!
//! The secret store keeps the key encrypted while the user is logged out and
//! out of copies of the configuration folder. It does not hide it from other
//! programs of the same user, and it needs a logged-in session: a daemon
//! started without one (e.g. a system service at boot) cannot read it.
//!
//! Moving the key is safe against interruptions: the key is written to the
//! new place and read back before the configuration switches to it, and the
//! old copy is removed last.

use anyhow::{Context, Result};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Where the device key is kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStorage {
    /// `secret.key` in the configuration folder (`0600` on Unix).
    #[default]
    File,
    /// The system's secret store (desktop Linux and Windows).
    System,
}

impl KeyStorage {
    /// Whether this is [`KeyStorage::File`] (omitted from `config.toml`).
    pub fn is_file(&self) -> bool {
        *self == Self::File
    }

    /// Whether the system's secret store exists on this platform at all.
    pub fn system_supported() -> bool {
        cfg!(any(
            all(target_os = "linux", not(target_os = "android")),
            windows
        ))
    }
}

impl std::fmt::Display for KeyStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::File => "file",
            Self::System => "system",
        })
    }
}

impl std::str::FromStr for KeyStorage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "file" => Ok(Self::File),
            "system" | "keyring" => Ok(Self::System),
            _ => anyhow::bail!(t!("keystore.bad_storage", value = s)),
        }
    }
}

/// One secret in a store.
pub(crate) trait SecretSlot {
    /// The secret, or `None` when there is none.
    fn get(&self) -> Result<Option<Vec<u8>>>;
    /// Stores `secret`, replacing an existing one.
    fn set(&self, secret: &[u8]) -> Result<()>;
    /// Deletes the secret; nothing when there is none.
    fn delete(&self) -> Result<()>;
}

/// Name of the entries in the secret store.
const SERVICE: &str = "io.github.vp859.ferry";

/// The entry of the device key of the configuration in `config_dir`; the
/// folder is part of the name, so separate profiles (`FERRY_CONFIG_DIR`)
/// keep separate keys.
fn account(config_dir: &Path) -> String {
    format!("device key ({})", config_dir.display())
}

/// The system's secret store, through keyring-core.
#[cfg(any(all(target_os = "linux", not(target_os = "android")), windows))]
struct SystemSlot(keyring_core::Entry);

#[cfg(any(all(target_os = "linux", not(target_os = "android")), windows))]
impl SecretSlot for SystemSlot {
    fn get(&self) -> Result<Option<Vec<u8>>> {
        match self.0.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(unavailable(e)),
        }
    }

    fn set(&self, secret: &[u8]) -> Result<()> {
        self.0.set_secret(secret).map_err(unavailable)
    }

    fn delete(&self) -> Result<()> {
        match self.0.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(unavailable(e)),
        }
    }
}

/// The error shown when the secret store cannot be used.
#[cfg(any(all(target_os = "linux", not(target_os = "android")), windows))]
fn unavailable(e: keyring_core::Error) -> anyhow::Error {
    anyhow::anyhow!(t!("keystore.unavailable", error = e))
}

/// The slot of the device key in the system's secret store.
///
/// # Errors
///
/// Fails when the platform has no secret store or it cannot be reached (no
/// Secret Service on the session bus, no session at all).
pub(crate) fn system_slot(config_dir: &Path) -> Result<Box<dyn SecretSlot>> {
    #[cfg(any(all(target_os = "linux", not(target_os = "android")), windows))]
    {
        use keyring_core::api::CredentialStoreApi;
        #[cfg(target_os = "linux")]
        let store = zbus_secret_service_keyring_store::Store::new().map_err(unavailable)?;
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new().map_err(unavailable)?;
        let entry = store
            .build(SERVICE, &account(config_dir), None)
            .map_err(unavailable)?;
        Ok(Box::new(SystemSlot(entry)))
    }
    #[cfg(not(any(all(target_os = "linux", not(target_os = "android")), windows)))]
    {
        let _ = (config_dir, SERVICE);
        anyhow::bail!(t!("keystore.unsupported"))
    }
}

/// A 32-byte key from a store.
pub(crate) fn key_bytes(secret: Vec<u8>) -> Result<[u8; 32]> {
    secret
        .try_into()
        .map_err(|_| anyhow::anyhow!(t!("identity.bad_size")))
}

/// Writes `key` to `slot` and reads it back.
///
/// # Errors
///
/// Fails when it cannot be written or reads back differently.
pub(crate) fn store_verified(slot: &dyn SecretSlot, key: &[u8; 32]) -> Result<()> {
    slot.set(key)?;
    let back = slot.get()?.context(t!("keystore.verify_failed"))?;
    if back.as_slice() != key {
        anyhow::bail!(t!("keystore.verify_failed"));
    }
    Ok(())
}

/// Overwrites the key file with zeros before deleting it, so the key does not
/// stay readable in its blocks (best effort: SSDs and copy-on-write file
/// systems may keep old data).
pub(crate) fn erase_file(path: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::metadata(path) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.write_all(&vec![0u8; usize::try_from(meta.len()).unwrap_or(32)]);
            let _ = file.sync_all();
        }
        std::fs::remove_file(path).with_context(|| t!("identity.write", path = path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A secret store in memory.
    #[derive(Default)]
    pub(crate) struct MemorySlot(pub Mutex<Option<Vec<u8>>>);

    impl SecretSlot for MemorySlot {
        fn get(&self) -> Result<Option<Vec<u8>>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn set(&self, secret: &[u8]) -> Result<()> {
            *self.0.lock().unwrap() = Some(secret.to_vec());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn storage_names_parse_and_print() {
        assert_eq!("system".parse::<KeyStorage>().unwrap(), KeyStorage::System);
        assert_eq!(" FILE ".parse::<KeyStorage>().unwrap(), KeyStorage::File);
        assert_eq!("keyring".parse::<KeyStorage>().unwrap(), KeyStorage::System);
        assert!("cloud".parse::<KeyStorage>().is_err());
        assert_eq!(KeyStorage::System.to_string(), "system");
        assert_eq!(KeyStorage::default(), KeyStorage::File);
    }

    #[test]
    fn profiles_have_their_own_entries() {
        assert_ne!(account(Path::new("/a")), account(Path::new("/b")));
    }

    #[test]
    fn keys_are_stored_verified_and_files_erased() {
        let slot = MemorySlot::default();
        store_verified(&slot, &[7u8; 32]).unwrap();
        assert_eq!(key_bytes(slot.get().unwrap().unwrap()).unwrap(), [7u8; 32]);
        assert!(key_bytes(vec![1, 2, 3]).is_err());

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secret.key");
        std::fs::write(&file, [9u8; 32]).unwrap();
        erase_file(&file).unwrap();
        assert!(!file.exists());
        erase_file(&file).unwrap();
    }
}
