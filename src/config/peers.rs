//! Persistent settings: trusted devices, download folder, device name and language.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use iroh::{EndpointAddr, EndpointId};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use crate::node::{NetworkMode, NodeTicket};

/// A trusted device stored in the configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// User-facing name of the device.
    pub alias: String,
    /// Accept files from this device without asking.
    pub auto_accept: bool,
    /// When the device was first added.
    pub added_at: DateTime<Utc>,
    /// The user compared the [safety number](crate::pairing::safety_number) with
    /// the other device and found it equal.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verified: bool,
    /// Last known TICKET (direct addresses); used when the device is not visible
    /// on the local network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    /// Secret shared with the device at pairing (hex), with which a hidden
    /// device encrypts its answers to discovery; none for devices paired
    /// before it existed (they are answered in the clear).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_key: Option<String>,
}

/// Contents of `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Trusted devices keyed by Node ID.
    pub peers: HashMap<String, PeerInfo>,
    /// Folder for received files; [`default_download_dir`](super::paths::default_download_dir) when unset.
    pub default_download_dir: Option<PathBuf>,
    /// Name of this device shown during discovery and pairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    /// User interface language code (`en`, `pl`); English when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether external servers may be used; [`NetworkMode::Local`] when unset.
    /// Applies when the node starts.
    #[serde(default, skip_serializing_if = "NetworkMode::is_local")]
    pub network_mode: NetworkMode,
    /// Relays of [`NetworkMode::Relay`] (`https://relay.example.org`); kept
    /// when another mode is chosen, so switching back needs no typing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay_urls: Vec<String>,
    /// Do not keep a history of transfers ([`crate::history`]); it is kept
    /// by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_history: bool,
    /// Where the device key is kept ([`crate::node::keystore`]); change it
    /// only with [`crate::node::identity::set_key_storage`], which moves the key.
    #[serde(
        default,
        skip_serializing_if = "crate::node::keystore::KeyStorage::is_file"
    )]
    pub key_storage: crate::node::keystore::KeyStorage,
    /// Speed limit in bytes per second, for sending and separately for
    /// receiving ([`crate::transfer::control`]); none when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_limit: Option<u64>,
    /// Hide this device on the local network from everyone except paired devices.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Send photos (JPEG, PNG) without their location and other metadata
    /// ([`crate::transfer::metadata`]). Off by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strip_metadata: bool,
    /// Announce the operating system on the local network (it only chooses an
    /// icon on the other devices). Off by default; a hidden device tells it
    /// to its paired devices anyway, since only they can read the answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub announce_platform: bool,
    /// Listen for direct connections on a random UDP port instead of the fixed
    /// [`QUIC_PORT`](crate::node::QUIC_PORT), so a scan of that port does not
    /// find the device. Other devices learn the port from announcements and
    /// TICKETs; a firewall rule must then allow the program, not a port.
    /// Applies at the next start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub random_port: bool,
    /// Devices removed from the trusted devices that could not be told about it
    /// yet ([`Hello::Unpair`](crate::protocol::messages::Hello::Unpair)), with
    /// the time of removal. They are told when they appear on the local
    /// network, and forgotten after [`Config::UNPAIR_KEPT_DAYS`] days.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub pending_unpair: std::collections::BTreeMap<String, DateTime<Utc>>,
    /// Accept offers, texts and batches only from paired devices, and
    /// pairing requests only while pairing is open ([`Config::open_pairing`]).
    /// Others are declined without the user being asked. Off by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paired_only: bool,
    /// Until when pairing requests from unknown devices are let through
    /// although [`Config::paired_only`] is on; kept in the file so that the
    /// command line can open it for a running daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_until: Option<DateTime<Utc>>,
    /// Copy texts from Auto-Accept devices to the clipboard without asking.
    /// Off by default: a text silently replacing the clipboard is easy to miss
    /// and could be pasted unknowingly (e.g. a changed account number).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_accept_texts: bool,
    /// Files that can run code (programs, scripts, documents with macros; see
    /// [`risk`](crate::transfer::risk)) are accepted from Auto-Accept devices
    /// without asking. Off by default: they always ask first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_accept_executables: bool,
    /// Desktop app: the welcome card for new users was closed, or the user
    /// paired a device or sent a file; it is not shown again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub welcome_dismissed: bool,
    /// Desktop app: closing the window quits Ferry instead of keeping it
    /// running in the system tray.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quit_on_close: bool,
    /// LocalSend compatibility mode ([`crate::localsend`]): receive files from
    /// LocalSend apps (iPhone, Mac, …). Off by default: senders are not
    /// authenticated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub localsend: bool,
}

impl Config {
    /// Path of `config.toml` inside [`config_dir`](super::paths::config_dir).
    ///
    /// # Errors
    ///
    /// Fails when the configuration directory cannot be determined or created.
    pub fn file_path() -> Result<PathBuf> {
        Ok(super::paths::config_dir()?.join("config.toml"))
    }

    /// Loads the configuration, returning defaults when the file does not exist.
    ///
    /// # Errors
    ///
    /// Fails when the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self> {
        let path = Self::file_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path).with_context(|| t!("config.read"))?;
        toml::from_str(&content).with_context(|| t!("config.parse"))
    }

    /// Saves the configuration atomically (temporary file + rename), so an
    /// interruption never leaves a corrupted file behind.
    ///
    /// # Errors
    ///
    /// Fails on serialization or I/O errors.
    pub fn save(&self) -> Result<()> {
        let path = Self::file_path()?;
        let content = toml::to_string_pretty(self).with_context(|| t!("config.serialize"))?;
        let tmp = path.with_extension("toml.tmp");
        // The file holds the secrets shared with paired devices: only the owner reads it.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options
            .open(&tmp)
            .and_then(|mut file| std::io::Write::write_all(&mut file, content.as_bytes()))
            .with_context(|| t!("config.write"))?;
        std::fs::rename(&tmp, &path).with_context(|| t!("config.write"))?;
        Ok(())
    }

    /// Folder for received files.
    pub fn download_dir(&self) -> PathBuf {
        self.default_download_dir
            .clone()
            .unwrap_or_else(super::paths::default_download_dir)
    }

    /// Name of this device, falling back to [`default_device_name`](super::paths::default_device_name).
    pub fn device_name(&self) -> String {
        self.device_name
            .as_deref()
            .map(crate::pairing::clean_device_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(super::paths::default_device_name)
    }

    /// Stores a device after successful pairing.
    ///
    /// An existing entry keeps its alias and Auto-Accept setting (both chosen by
    /// the user); only its address is refreshed. `addr` is ignored unless it
    /// belongs to `node_id`.
    ///
    /// # Errors
    ///
    /// Fails only if the address cannot be encoded as a TICKET.
    pub fn add_paired(
        &mut self,
        node_id: EndpointId,
        name: &str,
        addr: Option<EndpointAddr>,
        auto_accept: bool,
        discovery_key: Option<crate::pairing::DiscoveryKey>,
    ) -> Result<()> {
        let key = node_id.to_string();
        let ticket = match addr.filter(|a| a.id == node_id) {
            Some(a) => Some(NodeTicket::new(a).serialize_to_string()?),
            None => self.peers.get(&key).and_then(|p| p.ticket.clone()),
        };
        let (alias, auto_accept) = match self.peers.get(&key) {
            Some(existing) => (existing.alias.clone(), existing.auto_accept || auto_accept),
            None => {
                let name = crate::pairing::clean_device_name(name);
                let alias = if name.is_empty() {
                    fallback_alias(&key)
                } else {
                    name
                };
                (alias, auto_accept)
            }
        };
        let discovery_key = discovery_key
            .map(|k| crate::transfer::short_hex(&k))
            .or_else(|| self.peers.get(&key).and_then(|p| p.discovery_key.clone()));
        self.upsert_peer(key.clone(), alias, auto_accept, ticket);
        if let Some(peer) = self.peers.get_mut(&key) {
            peer.discovery_key = discovery_key;
        }
        Ok(())
    }

    /// The [`DiscoveryKeys`](crate::pairing::DiscoveryKey) of the paired devices
    /// that have one, by Node ID.
    pub fn discovery_keys(&self) -> HashMap<EndpointId, crate::pairing::DiscoveryKey> {
        self.peers
            .iter()
            .filter_map(|(id, peer)| {
                let id = EndpointId::from_str(id).ok()?;
                let hex = peer.discovery_key.as_deref()?;
                let mut key = [0u8; 32];
                if hex.len() != 64 {
                    return None;
                }
                for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
                    *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
                }
                Some((id, key))
            })
            .collect()
    }

    /// Adds a device without a known address.
    pub fn add_peer(&mut self, node_id: String, alias: String, auto_accept: bool) {
        self.upsert_peer(node_id, alias, auto_accept, None);
    }

    /// Adds or replaces a device, keeping the original `added_at` of an existing entry.
    pub fn upsert_peer(
        &mut self,
        node_id: String,
        alias: String,
        auto_accept: bool,
        ticket: Option<String>,
    ) {
        let existing = self.peers.get(&node_id);
        let added_at = existing.map(|p| p.added_at).unwrap_or_else(Utc::now);
        // Re-adding a device keeps what the user already verified.
        let verified = existing.is_some_and(|p| p.verified);
        let discovery_key = existing.and_then(|p| p.discovery_key.clone());
        // A device that is trusted again does not need to be told it was removed.
        self.pending_unpair.remove(&node_id);
        self.peers.insert(
            node_id,
            PeerInfo {
                alias,
                auto_accept,
                added_at,
                verified,
                ticket,
                discovery_key,
            },
        );
    }

    /// Adds a device from a TICKET or a raw Node ID and returns its Node ID.
    ///
    /// An empty `alias` is replaced with a name derived from the Node ID.
    ///
    /// # Errors
    ///
    /// Fails when `input` is neither a valid TICKET nor a valid Node ID. A
    /// truncated TICKET produces a dedicated, user-friendly error.
    pub fn add_from_input(
        &mut self,
        input: &str,
        alias: &str,
        auto_accept: bool,
    ) -> Result<String> {
        let input = input.trim();
        let (node_id, ticket) = match NodeTicket::deserialize_from_str(input) {
            Ok(t) => (
                t.addr.id.to_string(),
                Some(NodeTicket::new(t.addr).serialize_to_string()?),
            ),
            Err(ticket_err) => match EndpointId::from_str(input) {
                Ok(id) => (id.to_string(), None),
                Err(_) if NodeTicket::looks_like_ticket(input) => return Err(ticket_err),
                Err(_) => anyhow::bail!(t!("config.bad_node_id")),
            },
        };
        let alias = match alias.trim() {
            "" => fallback_alias(&node_id),
            a => a.to_string(),
        };
        let ticket = ticket.or_else(|| self.peers.get(&node_id).and_then(|p| p.ticket.clone()));
        self.upsert_peer(node_id.clone(), alias, auto_accept, ticket);
        Ok(node_id)
    }

    /// Removes a device by Node ID or alias; returns whether anything was removed.
    pub fn remove_peer(&mut self, id_or_alias: &str) -> bool {
        match self.find_key(id_or_alias) {
            Some(key) => self.peers.remove(&key).is_some(),
            None => false,
        }
    }

    /// Whether the file `file_name` from `node_id` is saved without asking: the
    /// device has Auto-Accept, and the file is harmless or
    /// [`Config::auto_accept_executables`] is on.
    pub fn auto_accepts_file(&self, node_id: &str, file_name: &str) -> bool {
        self.find_peer(node_id).is_some_and(|p| p.auto_accept)
            && (self.auto_accept_executables
                || crate::transfer::risk::file_risk(file_name).is_none())
    }

    /// Days a removed device is waited for ([`Config::pending_unpair`]).
    pub const UNPAIR_KEPT_DAYS: i64 = 30;
    /// Most removed devices waited for.
    const MAX_PENDING_UNPAIR: usize = 64;

    /// Remembers that the removed device `node_id` still has to be told.
    /// Entries older than [`Config::UNPAIR_KEPT_DAYS`] are dropped.
    pub fn queue_unpair(&mut self, node_id: &str) {
        let oldest = Utc::now() - chrono::Duration::days(Self::UNPAIR_KEPT_DAYS);
        self.pending_unpair.retain(|_, at| *at > oldest);
        if self.pending_unpair.len() < Self::MAX_PENDING_UNPAIR {
            self.pending_unpair.insert(node_id.to_string(), Utc::now());
        }
    }

    /// The removed devices that still have to be told.
    pub fn pending_unpair_ids(&self) -> Vec<String> {
        let oldest = Utc::now() - chrono::Duration::days(Self::UNPAIR_KEPT_DAYS);
        self.pending_unpair
            .iter()
            .filter(|(_, at)| **at > oldest)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Longest time pairing can be left open, in minutes.
    pub const MAX_PAIRING_MINUTES: i64 = 30;

    /// Lets unknown devices send pairing requests for `minutes` (at most
    /// [`Config::MAX_PAIRING_MINUTES`]) although [`Config::paired_only`] is on.
    /// The caller saves the configuration.
    pub fn open_pairing(&mut self, minutes: i64) {
        let minutes = minutes.clamp(1, Self::MAX_PAIRING_MINUTES);
        self.pairing_until = Some(Utc::now() + chrono::Duration::minutes(minutes));
    }

    /// Closes pairing again before its time is up.
    pub fn close_pairing(&mut self) {
        self.pairing_until = None;
    }

    /// Whether pairing is open at the moment.
    pub fn pairing_open(&self) -> bool {
        self.pairing_until.is_some_and(|until| until > Utc::now())
    }

    /// Whether a request from `node_id` is let through to the user. Always,
    /// unless [`Config::paired_only`] is on; then only paired devices, and
    /// unknown ones only with a pairing request (`pairing`) while pairing is
    /// open.
    pub fn admits(&self, node_id: &str, pairing: bool) -> bool {
        !self.paired_only || self.peers.contains_key(node_id) || (pairing && self.pairing_open())
    }

    /// Like [`Config::auto_accepts_file`] for a batch announced with these (at
    /// most the first 20) `names`. Files of an accepted batch that are risky
    /// are asked about one by one anyway.
    pub fn auto_accepts_batch(&self, node_id: &str, names: &[String]) -> bool {
        self.find_peer(node_id).is_some_and(|p| p.auto_accept)
            && (self.auto_accept_executables || crate::transfer::risk::worst_risk(names).is_none())
    }

    /// Whether a text from `node_id` is copied to the clipboard without asking:
    /// only when [`Config::auto_accept_texts`] is on and the device is trusted
    /// with Auto-Accept.
    pub fn auto_accepts_texts_from(&self, node_id: &str) -> bool {
        self.auto_accept_texts && self.find_peer(node_id).is_some_and(|p| p.auto_accept)
    }

    /// Records whether the user verified the safety number of a trusted device;
    /// `false` when no device matches `id_or_alias`.
    pub fn set_verified(&mut self, id_or_alias: &str, verified: bool) -> bool {
        let Some(key) = self.find_key(id_or_alias) else {
            return false;
        };
        self.peers.get_mut(&key).is_some_and(|p| {
            p.verified = verified;
            true
        })
    }

    /// Node IDs of all trusted devices; malformed entries are skipped.
    pub fn trusted_ids(&self) -> impl Iterator<Item = EndpointId> + '_ {
        self.peers
            .keys()
            .filter_map(|key| EndpointId::from_str(key).ok())
    }

    /// Looks up a device by its exact Node ID.
    pub fn find_peer(&self, node_id: &str) -> Option<&PeerInfo> {
        self.peers.get(node_id)
    }

    /// Finds the Node ID of a device by Node ID or alias (case-insensitive).
    pub fn find_key(&self, id_or_alias: &str) -> Option<String> {
        if self.peers.contains_key(id_or_alias) {
            return Some(id_or_alias.to_string());
        }
        self.peers
            .iter()
            .find(|(_, info)| info.alias.eq_ignore_ascii_case(id_or_alias))
            .map(|(k, _)| k.clone())
    }

    /// Turns an alias, Node ID or TICKET into a connection address.
    ///
    /// Known devices use their stored TICKET when available; otherwise only the
    /// Node ID is known and the device must be found on the local network.
    ///
    /// # Errors
    ///
    /// Fails when `target` matches no device and is not a valid TICKET or Node ID.
    pub fn resolve_target(&self, target: &str) -> Result<EndpointAddr> {
        let target = target.trim();
        if let Some(key) = self.find_key(target) {
            if let Some(ticket) = self.peers.get(&key).and_then(|p| p.ticket.as_deref())
                && let Ok(t) = NodeTicket::deserialize_from_str(ticket)
            {
                return Ok(t.addr);
            }
            return Ok(EndpointId::from_str(&key)
                .with_context(|| t!("config.corrupted_node_id"))?
                .into());
        }
        NodeTicket::parse_target(target)
    }
}

/// Alias for a device without a name: `Device <first 8 characters of the Node ID>`.
fn fallback_alias(node_id: &str) -> String {
    t!(
        "config.fallback_alias",
        id = &node_id[..8.min(node_id.len())]
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configuration holds secrets shared with paired devices.
    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner_and_keeps_the_keys() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public();
        c.add_paired(id, "Friend", None, false, Some([0xAB; 32]))?;
        // `save` writes to the configuration folder; write the same way here.
        let content = toml::to_string_pretty(&c)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&path)?, content.as_bytes())?;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );

        let loaded: Config = toml::from_str(&std::fs::read_to_string(&path)?)?;
        assert_eq!(loaded.discovery_keys().get(&id), Some(&[0xAB; 32]));
        // Pairing again without a key keeps the old one; with one, replaces it.
        let mut again = loaded;
        again.add_paired(id, "Friend", None, false, None)?;
        assert_eq!(again.discovery_keys().get(&id), Some(&[0xAB; 32]));
        again.add_paired(id, "Friend", None, false, Some([0xCD; 32]))?;
        assert_eq!(again.discovery_keys().get(&id), Some(&[0xCD; 32]));
        Ok(())
    }

    #[test]
    fn paired_only_lets_strangers_pair_only_while_pairing_is_open() {
        let mut c = Config::default();
        let known = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public().to_string();
        c.add_paired(known, "Known", None, false, None).unwrap();
        let known = known.to_string();

        assert!(c.admits(&stranger, false), "off: everyone asks");
        c.paired_only = true;
        assert!(c.admits(&known, false) && c.admits(&known, true));
        assert!(!c.admits(&stranger, false) && !c.admits(&stranger, true));

        c.open_pairing(5);
        assert!(c.admits(&stranger, true), "pairing request while open");
        assert!(!c.admits(&stranger, false), "but no files or texts");
        c.close_pairing();
        assert!(!c.admits(&stranger, true));

        c.pairing_until = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(!c.pairing_open(), "expired");
        c.open_pairing(1000);
        let left = c.pairing_until.unwrap() - Utc::now();
        assert!(left <= chrono::Duration::minutes(Config::MAX_PAIRING_MINUTES));
    }

    #[test]
    fn removed_devices_wait_to_be_told_until_they_are_trusted_again_or_too_old() {
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public();
        let key = id.to_string();
        assert!(c.pending_unpair_ids().is_empty());
        c.queue_unpair(&key);
        assert_eq!(c.pending_unpair_ids(), std::slice::from_ref(&key));

        c.add_paired(id, "Back again", None, false, None).unwrap();
        assert!(c.pending_unpair_ids().is_empty(), "trusted again");

        c.queue_unpair(&key);
        c.pending_unpair.insert(
            key.clone(),
            Utc::now() - chrono::Duration::days(Config::UNPAIR_KEPT_DAYS + 1),
        );
        assert!(c.pending_unpair_ids().is_empty(), "too old");
        c.queue_unpair("other");
        assert!(
            !c.pending_unpair.contains_key(&key),
            "old entries are dropped"
        );

        for i in 0..100 {
            c.queue_unpair(&format!("device{i}"));
        }
        assert!(c.pending_unpair.len() <= 64);
    }

    #[test]
    fn find_by_alias_and_remove() {
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public().to_string();
        c.add_from_input(&id, "Office Laptop", true).unwrap();
        assert_eq!(c.find_key("office laptop"), Some(id.clone()));
        assert!(c.resolve_target("Office Laptop").is_ok());
        assert!(c.remove_peer("OFFICE LAPTOP"));
        assert!(c.peers.is_empty());
    }

    #[test]
    fn add_paired_keeps_user_settings() -> Result<()> {
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public();
        c.add_paired(id, "Office\nLaptop", Some(id.into()), false, None)?;
        let key = id.to_string();
        assert_eq!(c.peers[&key].alias, "Office Laptop");
        assert!(c.peers[&key].ticket.is_some());
        c.peers.get_mut(&key).unwrap().alias = "My laptop".into();
        c.add_paired(id, "Other name", None, true, None)?;
        assert_eq!(c.peers[&key].alias, "My laptop");
        assert!(c.peers[&key].auto_accept);
        let other = iroh::SecretKey::generate().public();
        c.add_paired(other, "X", Some(id.into()), false, None)?;
        assert!(c.peers[&other.to_string()].ticket.is_none());
        Ok(())
    }

    #[test]
    fn auto_accept_does_not_cover_programs_unless_allowed() {
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public().to_string();
        let stranger = iroh::SecretKey::generate().public().to_string();
        c.add_from_input(&id, "Phone", true).unwrap();
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert!(c.auto_accepts_file(&id, "photo.jpg"));
        assert!(!c.auto_accepts_file(&id, "setup.exe"));
        assert!(!c.auto_accepts_file(&id, "invoice.pdf.exe"));
        assert!(
            !c.auto_accepts_file(&stranger, "photo.jpg"),
            "no Auto-Accept"
        );
        assert!(c.auto_accepts_batch(&id, &names(&["a.jpg", "b.pdf"])));
        assert!(!c.auto_accepts_batch(&id, &names(&["a.jpg", "run.sh"])));

        c.auto_accept_executables = true;
        assert!(c.auto_accepts_file(&id, "setup.exe"));
        assert!(c.auto_accepts_batch(&id, &names(&["run.sh"])));
        assert!(
            !c.auto_accepts_file(&stranger, "setup.exe"),
            "still needs Auto-Accept"
        );

        c.peers.get_mut(&id).unwrap().auto_accept = false;
        assert!(!c.auto_accepts_file(&id, "photo.jpg"));
    }

    #[test]
    fn verification_is_kept_and_saved() -> Result<()> {
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public().to_string();
        c.add_from_input(&id, "Phone", false)?;
        assert!(!c.peers[&id].verified);
        assert!(c.set_verified("phone", true), "found by alias");
        assert!(!c.set_verified("nobody", true));
        assert!(c.peers[&id].verified);
        // Adding the device again (e.g. a new TICKET) keeps the verification.
        c.add_from_input(&id, "Phone", true)?;
        assert!(c.peers[&id].verified);
        // The flag survives saving; an unverified device writes nothing.
        let text = toml::to_string_pretty(&c)?;
        assert!(text.contains("verified = true"));
        let back: Config = toml::from_str(&text)?;
        assert!(back.peers[&id].verified);
        assert!(c.set_verified(&id, false));
        assert!(!toml::to_string_pretty(&c)?.contains("verified"));
        Ok(())
    }

    #[test]
    fn rejects_garbage_input() {
        let mut c = Config::default();
        assert!(c.add_from_input("not-a-ticket", "x", false).is_err());
    }
}
