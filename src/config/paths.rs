use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::OnceLock;

static CONFIG_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Ustawia katalog konfiguracji (wymagane na Androidzie, gdzie `dirs` nie zna
/// katalogów aplikacji). Działa tylko przy pierwszym wywołaniu.
pub fn set_config_dir(path: PathBuf) {
    let _ = CONFIG_DIR_OVERRIDE.set(path);
}

/// Katalog konfiguracji RQS (`config.toml`, `secret.key`).
///
/// Kolejność: [`set_config_dir`] → zmienna `RQS_CONFIG_DIR` → `<config_dir>/rqs`.
pub fn config_dir() -> Result<PathBuf> {
    let dir = if let Some(dir) = CONFIG_DIR_OVERRIDE.get() {
        dir.clone()
    } else if let Some(dir) = std::env::var_os("RQS_CONFIG_DIR") {
        PathBuf::from(dir)
    } else {
        dirs::config_dir()
            .context("Nie udało się zlokalizować katalogu konfiguracji użytkownika")?
            .join("rqs")
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Nie udało się utworzyć katalogu konfiguracji {:?}", dir))?;
    Ok(dir)
}

/// Domyślny katalog na odebrane pliki.
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Domyślna nazwa urządzenia widoczna dla innych (nazwa hosta bez domeny).
#[cfg(not(target_os = "android"))]
pub fn default_device_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let short = host.split('.').next().unwrap_or_default().trim();
    if short.is_empty() || short.eq_ignore_ascii_case("localhost") {
        format!("RQS ({})", std::env::consts::OS)
    } else {
        short.to_string()
    }
}

/// Na Androidzie nazwa hosta to zwykle „localhost” – model telefonu ustawia frontend.
#[cfg(target_os = "android")]
pub fn default_device_name() -> String {
    "Urządzenie Android".to_string()
}
