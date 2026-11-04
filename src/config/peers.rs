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
    /// Speed limit in bytes per second, for sending and separately for
    /// receiving ([`crate::transfer::control`]); none when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_limit: Option<u64>,
    /// Hide this device on the local network from everyone except paired devices.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
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
        std::fs::write(&tmp, content).with_context(|| t!("config.write"))?;
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
        self.upsert_peer(key, alias, auto_accept, ticket);
        Ok(())
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
        self.peers.insert(
            node_id,
            PeerInfo {
                alias,
                auto_accept,
                added_at,
                verified,
                ticket,
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
        c.add_paired(id, "Office\nLaptop", Some(id.into()), false)?;
        let key = id.to_string();
        assert_eq!(c.peers[&key].alias, "Office Laptop");
        assert!(c.peers[&key].ticket.is_some());
        c.peers.get_mut(&key).unwrap().alias = "My laptop".into();
        c.add_paired(id, "Other name", None, true)?;
        assert_eq!(c.peers[&key].alias, "My laptop");
        assert!(c.peers[&key].auto_accept);
        let other = iroh::SecretKey::generate().public();
        c.add_paired(other, "X", Some(id.into()), false)?;
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
