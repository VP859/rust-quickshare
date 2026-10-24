//! `rqs name` and `rqs language`: device settings.

use anyhow::Result;
use rust_i18n::t;

use crate::config::Config;
use crate::i18n::{self, Language};
use crate::pairing::clean_device_name;

/// `rqs name [new name]`: shows or changes the name visible to other devices.
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

/// `rqs language [code]`: shows or saves the interface language.
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
