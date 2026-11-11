//! A password-protected backup of this device: its key (the Node ID that
//! paired devices recognize) and its paired devices, so a new phone or a
//! reinstalled computer keeps every pairing instead of pairing again.
//!
//! ## Format
//!
//! A text file: comment lines starting with `#` and one line
//! `FERRY-BACKUP-1:` followed by base64 of
//!
//! ```text
//! salt (16 bytes) ‖ iterations (u32, big-endian) ‖ nonce (12 bytes) ‖ ciphertext with tag
//! ```
//!
//! The key is derived from the password with PBKDF2-HMAC-SHA256
//! ([`ITERATIONS`] rounds, a random salt) and encrypts a JSON [`Contents`]
//! with ChaCha20-Poly1305; `FERRY-BACKUP-1` is the associated data. Both come
//! from `ring`, which Ferry already uses. A wrong password and a damaged file
//! are indistinguishable: the tag does not match.
//!
//! Without the password the file reveals nothing but its size. With it,
//! anyone can act as this device towards its paired devices, so the password
//! must be good ([`MIN_PASSWORD_LEN`]) and the file kept like a key.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use iroh::SecretKey;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::config::peers::PeerInfo;

/// Marks the encrypted line; also the associated data of the encryption.
const PREFIX: &str = "FERRY-BACKUP-1:";
/// PBKDF2 rounds of new backups (OWASP's recommendation for PBKDF2-HMAC-SHA256).
pub const ITERATIONS: u32 = 600_000;
/// Rounds accepted when opening a backup: fewer would be too weak, more
/// would let a crafted file keep the device busy.
const ITERATION_RANGE: std::ops::RangeInclusive<u32> = 100_000..=10_000_000;
/// Shortest password accepted for a new backup (characters).
pub const MIN_PASSWORD_LEN: usize = 8;
/// File name extension of backups.
pub const EXTENSION: &str = "ferry-backup";

/// Length of the random salt of the password derivation.
const SALT_LEN: usize = 16;
/// Length of the ChaCha20-Poly1305 nonce.
const NONCE_LEN: usize = 12;

/// What a backup holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contents {
    /// Node ID of the key (checked when opening).
    pub node_id: String,
    /// The secret key (base64).
    key: String,
    /// Name of the device, if one was set.
    #[serde(default)]
    pub device_name: Option<String>,
    /// Paired devices, keyed by Node ID, with their aliases and discovery secrets.
    #[serde(default)]
    pub peers: HashMap<String, PeerInfo>,
    /// When the backup was made.
    pub created: chrono::DateTime<chrono::Utc>,
}

impl Contents {
    /// The secret key.
    ///
    /// # Errors
    ///
    /// Fails when it is not a valid key for [`Contents::node_id`].
    pub fn key(&self) -> Result<SecretKey> {
        let bytes: [u8; 32] = STANDARD
            .decode(&self.key)
            .ok()
            .and_then(|b| b.try_into().ok())
            .with_context(|| t!("backup.damaged"))?;
        let key = SecretKey::from(bytes);
        anyhow::ensure!(
            key.public().to_string() == self.node_id,
            t!("backup.damaged")
        );
        Ok(key)
    }
}

/// The encryption key for `password`, `salt` and `iterations`.
fn derive(password: &str, salt: &[u8], iterations: NonZeroU32) -> Result<LessSafeKey> {
    let mut key = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        password.as_bytes(),
        &mut key,
    );
    let unbound =
        UnboundKey::new(&CHACHA20_POLY1305, &key).map_err(|_| anyhow::anyhow!("bad key length"))?;
    Ok(LessSafeKey::new(unbound))
}

/// A backup of `key` and the paired devices and name in `config`, encrypted
/// with `password`, as the text of a backup file.
///
/// # Errors
///
/// Fails when the password is shorter than [`MIN_PASSWORD_LEN`] or the
/// system has no randomness.
pub fn create(key: &SecretKey, config: &Config, password: &str) -> Result<String> {
    anyhow::ensure!(
        password.chars().count() >= MIN_PASSWORD_LEN,
        t!("backup.password_short", min = MIN_PASSWORD_LEN)
    );
    let contents = Contents {
        node_id: key.public().to_string(),
        key: STANDARD.encode(key.to_bytes()),
        device_name: config.device_name.clone(),
        peers: config.peers.clone(),
        created: chrono::Utc::now(),
    };
    let mut plain = serde_json::to_vec(&contents)?;

    let random = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    random
        .fill(&mut salt)
        .and_then(|()| random.fill(&mut nonce))
        .map_err(|_| anyhow::anyhow!("no randomness"))?;
    let iterations = NonZeroU32::new(ITERATIONS).context("iterations")?;
    derive(password, &salt, iterations)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(PREFIX.as_bytes()),
            &mut plain,
        )
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut blob = Vec::with_capacity(SALT_LEN + 4 + NONCE_LEN + plain.len());
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&ITERATIONS.to_be_bytes());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&plain);
    Ok(format!(
        "# Ferry backup of {} (Node ID {}), {} paired device(s), {}.\n\
         # Encrypted with your password. Restore it in Ferry: Settings → Device backup.\n\
         # With the password, anyone can act as this device: keep both safe.\n\
         {PREFIX}{}\n",
        config.device_name(),
        contents.node_id,
        contents.peers.len(),
        contents.created.format("%Y-%m-%d"),
        STANDARD.encode(blob)
    ))
}

/// Opens the backup `text` with `password`.
///
/// # Errors
///
/// Fails when the text is not a Ferry backup, the password is wrong or the
/// file is damaged (the two cannot be told apart).
pub fn open(text: &str, password: &str) -> Result<Contents> {
    let encoded = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(PREFIX))
        .with_context(|| t!("backup.not_a_backup"))?;
    let blob = STANDARD
        .decode(encoded.trim())
        .ok()
        .filter(|b| b.len() > SALT_LEN + 4 + NONCE_LEN)
        .with_context(|| t!("backup.damaged"))?;
    let (salt, rest) = blob.split_at(SALT_LEN);
    let (rounds, rest) = rest.split_at(4);
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let rounds = u32::from_be_bytes(rounds.try_into().context("iterations")?);
    anyhow::ensure!(ITERATION_RANGE.contains(&rounds), t!("backup.damaged"));
    let iterations = NonZeroU32::new(rounds).context("iterations")?;
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow::anyhow!("nonce"))?;
    let mut sealed = sealed.to_vec();
    let plain = derive(password, salt, iterations)?
        .open_in_place(nonce, Aad::from(PREFIX.as_bytes()), &mut sealed)
        .map_err(|_| anyhow::anyhow!(t!("backup.wrong_password")))?;
    let contents: Contents = serde_json::from_slice(plain).with_context(|| t!("backup.damaged"))?;
    contents.key()?;
    Ok(contents)
}

/// What [`restore`] changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// Node ID this device has from now on.
    pub node_id: String,
    /// The key was another one before (Ferry must restart to use the new one).
    pub key_changed: bool,
    /// Where the previous key was kept, when it was replaced.
    pub previous_key: Option<PathBuf>,
    /// Paired devices added (devices already paired here are kept as they are).
    pub peers_added: usize,
}

/// Makes the backup's key this device's key (where `config` keeps keys) and
/// adds its paired devices and name to `config`, which the caller saves.
///
/// The previous key, when different, is kept as `secret.key.<time>.bak`
/// next to the configuration, readable only by the user.
///
/// # Errors
///
/// Fails when the key cannot be written (file or system store).
pub fn restore(contents: &Contents, config: &mut Config) -> Result<Restored> {
    use super::identity::{default_key_path, load_or_generate_default_key, write_private};
    use super::keystore::KeyStorage;

    let key = contents.key()?;
    let path = default_key_path()?;
    let current = load_or_generate_default_key().ok();
    let key_changed = current.as_ref().is_none_or(|c| c.public() != key.public());
    let mut previous_key = None;
    if key_changed {
        match config.key_storage {
            KeyStorage::File => previous_key = super::identity::replace_secret_key(&path, &key)?,
            KeyStorage::System => {
                if let Some(old) = &current {
                    let stamp = chrono::Utc::now().timestamp();
                    let backup = path.with_file_name(format!("secret.key.{stamp}.bak"));
                    write_private(&backup, &old.to_bytes())
                        .with_context(|| t!("identity.write", path = backup.display()))?;
                    previous_key = Some(backup);
                }
                let slot = super::keystore::system_slot(&crate::config::paths::config_dir()?)?;
                super::keystore::store_verified(slot.as_ref(), &key.to_bytes())?;
            }
        }
    }

    let own = key.public().to_string();
    let mut peers_added = 0;
    for (id, peer) in &contents.peers {
        if *id != own && !config.peers.contains_key(id) {
            config.peers.insert(id.clone(), peer.clone());
            peers_added += 1;
        }
    }
    if config.device_name.is_none() {
        config.device_name.clone_from(&contents.device_name);
    }
    Ok(Restored {
        node_id: own,
        key_changed,
        previous_key,
        peers_added,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_peer() -> Config {
        let mut config = Config {
            device_name: Some("Kate's phone".into()),
            ..Config::default()
        };
        config.peers.insert(
            "b".repeat(64),
            PeerInfo {
                alias: "Laptop".into(),
                auto_accept: true,
                added_at: chrono::Utc::now(),
                verified: true,
                ticket: None,
                discovery_key: Some("ab".repeat(32)),
            },
        );
        config
    }

    #[test]
    fn a_backup_opens_with_its_password_only() {
        let key = SecretKey::generate();
        let text = create(&key, &config_with_peer(), "correct horse").unwrap();
        assert!(text.starts_with("# Ferry backup of Kate's phone"));
        // Neither the key nor the devices are readable without the password.
        assert!(!text.contains(&STANDARD.encode(key.to_bytes())));
        assert!(!text.contains("Laptop"));

        let opened = open(&text, "correct horse").unwrap();
        assert_eq!(opened.key().unwrap().to_bytes(), key.to_bytes());
        assert_eq!(opened.node_id, key.public().to_string());
        assert_eq!(opened.device_name.as_deref(), Some("Kate's phone"));
        let laptop = &opened.peers[&"b".repeat(64)];
        assert_eq!(laptop.alias, "Laptop");
        assert!(laptop.verified && laptop.auto_accept);
        assert_eq!(laptop.discovery_key.as_deref(), Some(&*"ab".repeat(32)));

        assert!(open(&text, "wrong horse").is_err());
        assert!(open("hello", "correct horse").is_err());
    }

    #[test]
    fn a_changed_byte_is_noticed() {
        let text = create(&SecretKey::generate(), &Config::default(), "password1").unwrap();
        let line = text.lines().find(|l| l.starts_with(PREFIX)).unwrap();
        let mut blob = STANDARD.decode(&line[PREFIX.len()..]).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 1;
        let tampered = format!("{PREFIX}{}", STANDARD.encode(&blob));
        assert!(open(&tampered, "password1").is_err());
        // A file asking for absurd work is refused before any of it.
        blob[SALT_LEN..SALT_LEN + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        let costly = format!("{PREFIX}{}", STANDARD.encode(&blob));
        assert!(open(&costly, "password1").is_err());
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(create(&SecretKey::generate(), &Config::default(), "1234567").is_err());
        assert!(create(&SecretKey::generate(), &Config::default(), "żółwiątk").is_ok());
    }
}
