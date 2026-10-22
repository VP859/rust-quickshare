use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use iroh::{EndpointAddr, EndpointId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use crate::node::NodeTicket;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub alias: String,
    pub auto_accept: bool,
    pub added_at: DateTime<Utc>,
    /// Ostatni znany TICKET (adresy bezpośrednie + relay) – przyspiesza łączenie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub peers: HashMap<String, PeerInfo>,
    pub default_download_dir: Option<PathBuf>,
}

impl Config {
    pub fn file_path() -> Result<PathBuf> {
        Ok(super::paths::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::file_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            std::fs::read_to_string(&path).context("Nie udało się odczytać pliku konfiguracji")?;
        let config: Config =
            toml::from_str(&content).context("Błąd parsowania pliku config.toml")?;
        Ok(config)
    }

    /// Zapis atomowy (plik tymczasowy + rename), aby przerwanie nie uszkodziło konfiguracji.
    pub fn save(&self) -> Result<()> {
        let path = Self::file_path()?;
        let content =
            toml::to_string_pretty(self).context("Błąd serializacji konfiguracji do TOML")?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, content).context("Nie udało się zapisać pliku konfiguracji")?;
        std::fs::rename(&tmp, &path).context("Nie udało się podmienić pliku konfiguracji")?;
        Ok(())
    }

    pub fn download_dir(&self) -> PathBuf {
        self.default_download_dir
            .clone()
            .unwrap_or_else(super::paths::default_download_dir)
    }

    pub fn add_peer(&mut self, node_id: String, alias: String, auto_accept: bool) {
        self.upsert_peer(node_id, alias, auto_accept, None);
    }

    /// Dodaje lub aktualizuje urządzenie, zachowując datę dodania istniejącego wpisu.
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

    /// Dodaje urządzenie na podstawie TICKETU lub surowego Node ID.
    /// Zwraca Node ID dodanego urządzenia.
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
            Err(_) => {
                let id = EndpointId::from_str(input)
                    .map_err(|_| anyhow::anyhow!("Nieprawidłowy TICKET lub Node ID"))?;
                (id.to_string(), None)
            }
        };
        let alias = match alias.trim() {
            "" => format!("Węzeł {}", &node_id[..8]),
            a => a.to_string(),
        };
        let ticket = ticket.or_else(|| self.peers.get(&node_id).and_then(|p| p.ticket.clone()));
        self.upsert_peer(node_id.clone(), alias, auto_accept, ticket);
        Ok(node_id)
    }

    pub fn remove_peer(&mut self, id_or_alias: &str) -> bool {
        match self.find_key(id_or_alias) {
            Some(key) => self.peers.remove(&key).is_some(),
            None => false,
        }
    }

    pub fn find_peer(&self, node_id: &str) -> Option<&PeerInfo> {
        self.peers.get(node_id)
    }

    /// Znajduje klucz (Node ID) po identyfikatorze lub aliasie (bez rozróżniania wielkości liter).
    pub fn find_key(&self, id_or_alias: &str) -> Option<String> {
        if self.peers.contains_key(id_or_alias) {
            return Some(id_or_alias.to_string());
        }
        self.peers
            .iter()
            .find(|(_, info)| info.alias.eq_ignore_ascii_case(id_or_alias))
            .map(|(k, _)| k.clone())
    }

    /// Zamienia alias / Node ID / TICKET na adres docelowy połączenia.
    pub fn resolve_target(&self, target: &str) -> Result<EndpointAddr> {
        let target = target.trim();
        if let Some(key) = self.find_key(target) {
            if let Some(ticket) = self.peers.get(&key).and_then(|p| p.ticket.as_deref())
                && let Ok(t) = NodeTicket::deserialize_from_str(ticket)
            {
                return Ok(t.addr);
            }
            return Ok(EndpointId::from_str(&key)
                .context("Uszkodzony Node ID w konfiguracji")?
                .into());
        }
        NodeTicket::parse_target(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_by_alias_and_remove() {
        let mut c = Config::default();
        let id = iroh::SecretKey::generate().public().to_string();
        c.add_from_input(&id, "Laptop Biurowy", true).unwrap();
        assert_eq!(c.find_key("laptop biurowy"), Some(id.clone()));
        assert!(c.resolve_target("Laptop Biurowy").is_ok());
        assert!(c.remove_peer("LAPTOP BIUROWY"));
        assert!(c.peers.is_empty());
    }

    #[test]
    fn rejects_garbage_input() {
        let mut c = Config::default();
        assert!(c.add_from_input("not-a-ticket", "x", false).is_err());
    }
}
