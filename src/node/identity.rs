use anyhow::{Context, Result};
use iroh_net::key::SecretKey; // <-- Z iroh_net!
use std::path::Path;
use tracing::info;

pub fn load_or_generate_secret_key(path: Option<&Path>) -> Result<SecretKey> {
    if let Some(p) = path {
        if p.exists() {
            let bytes = std::fs::read(p)
                .with_context(|| format!("Nie udało się odczytać pliku klucza: {:?}", p))?;
            let key_array: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Plik klucza ma nieprawidłowy rozmiar (32 bajty)"))?;
            info!("Załadowano klucz z {:?}", p);
            return Ok(SecretKey::from(key_array));
        }
    }

    let secret_key = SecretKey::generate();
    info!("Wygenerowano nowy SecretKey");

    if let Some(p) = path {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, secret_key.to_bytes())
            .with_context(|| format!("Nie udało się zapisać klucza do {:?}", p))?;
    }

    Ok(secret_key)
}
