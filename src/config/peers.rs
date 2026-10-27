//! Persistent settings: trusted devices, download folder, device name and language.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use iroh::{EndpointAddr, EndpointId};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use crate::node::NodeTicket;

/// A trusted device stored in the configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// User-facing name of the device.
    pub alias: String,
    /// Accept files from this device without asking.
    pub auto_accept: bool,
    /// When the device was first added.
    pub added_at: DateTime<Utc>,
    /// Last known TICKET (relay and direct addresses); speeds up connecting.
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
        let added_at = self
            .peers
            .get(&node_id)
            .map(|p| p.added_at)
            .unwrap_or_else(Utc::now);
        self.peers.insert(
            node_id,
            PeerInfo {
                alias,
                auto_accept,
                added_at,
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
    /// Known devices use their stored TICKET when available; otherwise the
    /// address is resolved later through DNS/pkarr.
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
    fn rejects_garbage_input() {
        let mut c = Config::default();
        assert!(c.add_from_input("not-a-ticket", "x", false).is_err());
    }
}
