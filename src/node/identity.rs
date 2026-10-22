use anyhow::{Context, Result};
use iroh::SecretKey;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;

/// Ścieżka stałego klucza Ed25519 węzła (`<config_dir>/secret.key`).
pub fn default_key_path() -> Result<PathBuf> {
    Ok(crate::config::paths::config_dir()?.join("secret.key"))
}

/// Ładuje stały klucz węzła z katalogu konfiguracji, generując go przy pierwszym uruchomieniu.
/// Dzięki temu Node ID nie zmienia się między uruchomieniami (wymóg zaufanych urządzeń).
pub fn load_or_generate_default_key() -> Result<SecretKey> {
    load_or_generate_secret_key(Some(&default_key_path()?))
}

/// `None` → klucz efemeryczny (testy, jednorazowe transfery).
pub fn load_or_generate_secret_key(path: Option<&Path>) -> Result<SecretKey> {
    if let Some(p) = path
        && p.exists()
    {
        let bytes = std::fs::read(p)
            .with_context(|| format!("Nie udało się odczytać pliku klucza: {:?}", p))?;
        let key_array: [u8; 32] = bytes.try_into().map_err(|_| {
            anyhow::anyhow!("Plik klucza ma nieprawidłowy rozmiar (wymagane 32 bajty)")
        })?;
        info!("Załadowano klucz z {:?}", p);
        return Ok(SecretKey::from(key_array));
    }

    let secret_key = SecretKey::generate();
    info!(
        "Wygenerowano nowy SecretKey (PublicKey: {})",
        secret_key.public()
    );

    if let Some(p) = path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_private(p, &secret_key.to_bytes())
            .with_context(|| format!("Nie udało się zapisać klucza do {:?}", p))?;
    }

    Ok(secret_key)
}

/// Zapis pliku czytelnego wyłącznie dla właściciela (0600 na systemach Unix).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_across_loads() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("secret.key");
        let a = load_or_generate_secret_key(Some(&path))?;
        let b = load_or_generate_secret_key(Some(&path))?;
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)?.permissions().mode() & 0o777,
                0o600
            );
        }
        Ok(())
    }
}
