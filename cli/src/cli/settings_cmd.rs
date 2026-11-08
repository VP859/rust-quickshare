//! `ferry name`, `ferry language`, `ferry network` and `ferry config`: device settings.

use anyhow::Result;
use rust_i18n::t;
use std::path::PathBuf;

use crate::cli::args::{ConfigCommands, SettingKey};
use ferry::config::Config;
use ferry::i18n::{self, Language};
use ferry::node::NetworkMode;
use ferry::pairing::clean_device_name;

/// `ferry name [new name]`: shows or changes the name visible to other devices.
///
/// # Errors
///
/// Fails when the new name is empty or the configuration cannot be saved.
pub fn run_name(name: Option<&str>) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(name) = name {
        let clean = clean_device_name(name);
        if clean.is_empty() {
            anyhow::bail!(t!("settings.name_empty"));
        }
        config.device_name = Some(clean);
        config.save()?;
    }
    println!("{}", t!("settings.name", name = config.device_name()));
    Ok(())
}

/// `ferry language [code]`: shows or saves the interface language.
///
/// # Errors
///
/// Fails when the code is not supported or the configuration cannot be saved.
pub fn run_language(code: Option<&str>) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(code) = code {
        let language: Language = code.parse()?;
        config.language = Some(language.code().to_string());
        config.save()?;
        i18n::set_language(language);
    }
    let current = i18n::current();
    let available: Vec<String> = Language::ALL
        .iter()
        .map(|l| format!("{} ({})", l.code(), l.native_name()))
        .collect();
    println!(
        "{}",
        t!(
            "settings.language",
            language = current.native_name(),
            available = available.join(", ")
        )
    );
    Ok(())
}

/// Checks every relay URL; the list for `relay_urls`.
fn parse_relays<'a>(urls: impl IntoIterator<Item = &'a str>) -> Result<Vec<String>> {
    urls.into_iter()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| ferry::node::parse_relay_url(u).map(|r| r.to_string()))
        .collect()
}

/// `ferry network [local|internet|relay [URL…]]`: shows or saves the [`NetworkMode`].
///
/// The mode applies to nodes started afterwards (CLI commands, the daemon, the
/// TUI and the GUI after a restart).
///
/// # Errors
///
/// Fails when the mode is unknown or the configuration cannot be saved.
pub fn run_network(mode: Option<&str>, relays: &[String]) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(mode) = mode {
        config.network_mode = mode.parse()?;
        if !relays.is_empty() {
            config.relay_urls = parse_relays(relays.iter().map(String::as_str))?;
        }
        // Refuse a relay mode that could not start.
        ferry::node::Network::from_config(&config)?;
        config.save()?;
    }
    let description = match config.network_mode {
        NetworkMode::Local => t!("settings.network_local"),
        NetworkMode::Internet => t!("settings.network_internet"),
        NetworkMode::Relay => t!(
            "settings.network_relay",
            relays = config.relay_urls.join(", ")
        ),
    };
    println!(
        "{}",
        t!(
            "settings.network",
            mode = config.network_mode,
            description = description
        )
    );
    if mode.is_some() {
        println!("{}", t!("settings.network_restart"));
    }
    Ok(())
}

/// Every key of `ferry config`, in display order.
const SETTING_KEYS: [SettingKey; 15] = [
    SettingKey::DeviceName,
    SettingKey::Language,
    SettingKey::NetworkMode,
    SettingKey::RelayUrls,
    SettingKey::DownloadDir,
    SettingKey::Hidden,
    SettingKey::PairedOnly,
    SettingKey::AnnouncePlatform,
    SettingKey::RandomPort,
    SettingKey::AutoAcceptTexts,
    SettingKey::AutoAcceptExecutables,
    SettingKey::QuitOnClose,
    SettingKey::Localsend,
    SettingKey::History,
    SettingKey::SpeedLimit,
];

/// Name of a setting as typed on the command line (`device-name`, …).
fn key_name(key: SettingKey) -> String {
    use clap::ValueEnum;
    key.to_possible_value()
        .map(|v| v.get_name().to_string())
        .unwrap_or_default()
}

/// Current value of a setting as text (defaults included).
fn get_setting(config: &Config, key: SettingKey) -> String {
    match key {
        SettingKey::DeviceName => config.device_name(),
        SettingKey::Language => config
            .language
            .clone()
            .unwrap_or_else(|| i18n::current().code().to_string()),
        SettingKey::NetworkMode => config.network_mode.to_string(),
        SettingKey::RelayUrls => config.relay_urls.join(","),
        SettingKey::History => (!config.disable_history).to_string(),
        SettingKey::SpeedLimit => config.speed_limit.map_or_else(
            || "off".to_string(),
            |b| format!("{}/s", ferry::size::human_size(b)),
        ),
        SettingKey::DownloadDir => config.download_dir().display().to_string(),
        SettingKey::Hidden => config.hidden.to_string(),
        SettingKey::PairedOnly => config.paired_only.to_string(),
        SettingKey::AnnouncePlatform => config.announce_platform.to_string(),
        SettingKey::RandomPort => config.random_port.to_string(),
        SettingKey::AutoAcceptTexts => config.auto_accept_texts.to_string(),
        SettingKey::AutoAcceptExecutables => config.auto_accept_executables.to_string(),
        SettingKey::QuitOnClose => config.quit_on_close.to_string(),
        SettingKey::Localsend => config.localsend.to_string(),
    }
}

/// Parses a yes/no value (`true`/`false`, `yes`/`no`, `on`/`off`, `1`/`0`).
fn parse_bool(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => anyhow::bail!(t!("settings.bad_bool", value = value)),
    }
}

/// Validates `value` and stores it in `config` (not saved yet).
fn set_setting(config: &mut Config, key: SettingKey, value: &str) -> Result<()> {
    match key {
        SettingKey::DeviceName => {
            let clean = clean_device_name(value);
            if clean.is_empty() {
                anyhow::bail!(t!("settings.name_empty"));
            }
            config.device_name = Some(clean);
        }
        SettingKey::Language => {
            let language: Language = value.parse()?;
            config.language = Some(language.code().to_string());
        }
        SettingKey::NetworkMode => {
            config.network_mode = value.parse()?;
            ferry::node::Network::from_config(config)?;
        }
        SettingKey::RelayUrls => config.relay_urls = parse_relays(value.split(','))?,
        SettingKey::History => config.disable_history = !parse_bool(value)?,
        SettingKey::SpeedLimit => {
            config.speed_limit = ferry::transfer::control::parse_speed(value)?;
        }
        SettingKey::DownloadDir => {
            let dir = PathBuf::from(value);
            if !dir.is_dir() {
                anyhow::bail!(t!("settings.dir_missing", path = dir.display()));
            }
            config.default_download_dir = Some(std::fs::canonicalize(&dir).unwrap_or(dir));
        }
        SettingKey::Hidden => config.hidden = parse_bool(value)?,
        SettingKey::PairedOnly => config.paired_only = parse_bool(value)?,
        SettingKey::AnnouncePlatform => config.announce_platform = parse_bool(value)?,
        SettingKey::RandomPort => config.random_port = parse_bool(value)?,
        SettingKey::AutoAcceptTexts => config.auto_accept_texts = parse_bool(value)?,
        SettingKey::AutoAcceptExecutables => config.auto_accept_executables = parse_bool(value)?,
        SettingKey::QuitOnClose => config.quit_on_close = parse_bool(value)?,
        SettingKey::Localsend => config.localsend = parse_bool(value)?,
    }
    Ok(())
}

/// Resets a setting to its default (not saved yet).
fn unset_setting(config: &mut Config, key: SettingKey) {
    match key {
        SettingKey::DeviceName => config.device_name = None,
        SettingKey::Language => config.language = None,
        SettingKey::NetworkMode => config.network_mode = NetworkMode::default(),
        SettingKey::History => config.disable_history = false,
        SettingKey::SpeedLimit => config.speed_limit = None,
        SettingKey::RelayUrls => {
            config.relay_urls.clear();
            // The relay mode cannot run without a relay.
            if config.network_mode == NetworkMode::Relay {
                config.network_mode = NetworkMode::Local;
            }
        }
        SettingKey::DownloadDir => config.default_download_dir = None,
        SettingKey::Hidden => config.hidden = false,
        SettingKey::PairedOnly => config.paired_only = false,
        SettingKey::AnnouncePlatform => config.announce_platform = false,
        SettingKey::RandomPort => config.random_port = false,
        SettingKey::AutoAcceptTexts => config.auto_accept_texts = false,
        SettingKey::AutoAcceptExecutables => config.auto_accept_executables = false,
        SettingKey::QuitOnClose => config.quit_on_close = false,
        SettingKey::Localsend => config.localsend = false,
    }
}

/// `ferry config [list|get|set|unset|path]`: reads and changes settings.
///
/// Running programs (daemon, TUI, GUI) read most settings when they start, so
/// a change applies after restarting them.
///
/// # Errors
///
/// Fails when a value is invalid or the configuration cannot be loaded or saved.
pub fn run_config(command: Option<ConfigCommands>) -> Result<()> {
    let mut config = Config::load()?;
    match command.unwrap_or(ConfigCommands::List) {
        ConfigCommands::List => {
            for key in SETTING_KEYS {
                println!("{:<18} {}", key_name(key), get_setting(&config, key));
            }
        }
        ConfigCommands::Get { key } => println!("{}", get_setting(&config, key)),
        ConfigCommands::Set { key, value } => {
            set_setting(&mut config, key, &value)?;
            config.save()?;
            println!("{} = {}", key_name(key), get_setting(&config, key));
            println!("{}", t!("settings.restart_running"));
        }
        ConfigCommands::Unset { key } => {
            unset_setting(&mut config, key);
            config.save()?;
            println!("{} = {}", key_name(key), get_setting(&config, key));
            println!("{}", t!("settings.restart_running"));
        }
        ConfigCommands::Path => println!("{}", Config::file_path()?.display()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_settings_are_checked() {
        let mut config = Config::default();
        assert!(
            set_setting(&mut config, SettingKey::NetworkMode, "relay").is_err(),
            "no relay set yet"
        );
        assert!(
            set_setting(
                &mut config,
                SettingKey::RelayUrls,
                "http://relay.example.org"
            )
            .is_err()
        );
        set_setting(
            &mut config,
            SettingKey::RelayUrls,
            "https://a.example.org, http://192.168.1.5:3340",
        )
        .unwrap();
        assert_eq!(config.relay_urls.len(), 2);
        set_setting(&mut config, SettingKey::NetworkMode, "relay").unwrap();
        assert_eq!(get_setting(&config, SettingKey::NetworkMode), "relay");
        unset_setting(&mut config, SettingKey::RelayUrls);
        assert_eq!(
            config.network_mode,
            NetworkMode::Local,
            "never left without a relay"
        );
    }

    #[test]
    fn settings_round_trip() -> Result<()> {
        let mut config = Config::default();
        set_setting(&mut config, SettingKey::Hidden, "on")?;
        assert!(config.hidden);
        set_setting(&mut config, SettingKey::NetworkMode, "internet")?;
        assert_eq!(get_setting(&config, SettingKey::NetworkMode), "internet");
        assert!(set_setting(&mut config, SettingKey::AutoAcceptTexts, "maybe").is_err());
        assert!(set_setting(&mut config, SettingKey::DeviceName, " \n ").is_err());
        unset_setting(&mut config, SettingKey::Hidden);
        assert!(!config.hidden);
        assert_eq!(key_name(SettingKey::AutoAcceptTexts), "auto-accept-texts");
        Ok(())
    }
}
