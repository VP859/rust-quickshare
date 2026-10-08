use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub alias: String,
    pub auto_accept: bool,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub peers: HashMap<String, PeerInfo>,
    pub default_download_dir: Option<PathBuf>,
}

impl Config {
    pub fn file_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| {
                anyhow::anyhow!("Nie udało się zlokalizować katalogu konfiguracji użytkownika")
            })?
            .join("rqs");
        std::fs::create_dir_all(&config_dir)
            .context("Nie udało się utworzyć katalogu konfiguracji ~/.config/rqs")?;
        Ok(config_dir.join("config.toml"))
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

    pub fn save(&self) -> Result<()> {
        let path = Self::file_path()?;
        let content =
            toml::to_string_pretty(self).context("Błąd serializacji konfiguracji do TOML")?;
        std::fs::write(path, content).context("Nie udało się zapisać pliku konfiguracji")?;
        Ok(())
    }

    pub fn add_peer(&mut self, node_id: String, alias: String, auto_accept: bool) {
        self.peers.insert(
            node_id,
            PeerInfo {
                alias,
                auto_accept,
                added_at: Utc::now(),
            },
        );
    }

    pub fn remove_peer(&mut self, id_or_alias: &str) -> bool {
        if self.peers.remove(id_or_alias).is_some() {
            return true;
        }

        let key_to_remove = self
            .peers
            .iter()
            .find(|(_, info)| info.alias.eq_ignore_ascii_case(id_or_alias))
            .map(|(k, _)| k.clone());

        if let Some(key) = key_to_remove {
            self.peers.remove(&key);
            true
        } else {
            false
        }
    }

    pub fn find_peer(&self, node_id: &str) -> Option<&PeerInfo> {
        self.peers.get(node_id)
    }
}
