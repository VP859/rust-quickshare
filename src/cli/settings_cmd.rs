//! `ferry name`, `ferry language` and `ferry network`: device settings.

use anyhow::Result;
use rust_i18n::t;

use crate::config::Config;
use crate::i18n::{self, Language};
use crate::node::NetworkMode;
use crate::pairing::clean_device_name;

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

/// `ferry network [local|internet]`: shows or saves the [`NetworkMode`].
///
/// The mode applies to nodes started afterwards (CLI commands, the daemon, the
/// TUI and the GUI after a restart).
///
/// # Errors
///
/// Fails when the mode is unknown or the configuration cannot be saved.
pub fn run_network(mode: Option<&str>) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(mode) = mode {
        config.network_mode = mode.parse()?;
        config.save()?;
    }
    let description = match config.network_mode {
        NetworkMode::Local => t!("settings.network_local"),
        NetworkMode::Internet => t!("settings.network_internet"),
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
