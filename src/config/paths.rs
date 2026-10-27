//! Locations of the configuration directory and default user folders.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Directory set with [`set_config_dir`]; takes precedence over everything else.
static CONFIG_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Overrides the configuration directory for the whole process.
///
/// Required on Android, where [`dirs`] does not know the app directories.
/// Only the first call has an effect.
pub fn set_config_dir(path: PathBuf) {
    let _ = CONFIG_DIR_OVERRIDE.set(path);
}

/// Directory holding `config.toml` and `secret.key`; created if missing.
///
/// Resolution order: [`set_config_dir`] → `RQS_CONFIG_DIR` → `<user config dir>/rqs`.
///
/// # Errors
///
/// Fails when no user configuration directory exists or it cannot be created.
pub fn config_dir() -> Result<PathBuf> {
    let dir = if let Some(dir) = CONFIG_DIR_OVERRIDE.get() {
        dir.clone()
    } else if let Some(dir) = std::env::var_os("RQS_CONFIG_DIR") {
        PathBuf::from(dir)
    } else {
        dirs::config_dir()
            .with_context(|| t!("config.no_config_dir"))?
            .join("rqs")
    };
    std::fs::create_dir_all(&dir).with_context(|| t!("config.create_dir", path = dir.display()))?;
    Ok(dir)
}

/// Default folder for received files: the user's download folder.
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Default device name shown to other devices: the host name without its domain.
#[cfg(not(target_os = "android"))]
pub fn default_device_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let short = host.split('.').next().unwrap_or_default().trim();
    if short.is_empty() || short.eq_ignore_ascii_case("localhost") {
        format!("Ferry ({})", std::env::consts::OS)
    } else {
        short.to_string()
    }
}

/// Default device name on Android, where the host name is usually `localhost`.
///
/// The GUI replaces it with the phone model on first start.
#[cfg(target_os = "android")]
pub fn default_device_name() -> String {
    t!("config.android_device").into_owned()
}
