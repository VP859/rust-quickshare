use anyhow::{Context, Result};
use std::path::Path;

pub async fn compute_file_hash(path: &Path) -> Result<[u8; 32]> {
    let path_buf = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if let Ok(hasher) = blake3::Hasher::new().update_mmap_rayon(&path_buf) {
            return Ok(*hasher.finalize().as_bytes());
        }

        let mut file = std::fs::File::open(&path_buf).with_context(|| {
            format!(
                "Nie udało się otworzyć pliku do wyliczenia haszu: {:?}",
                path_buf
            )
        })?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(*hasher.finalize().as_bytes())
    })
    .await
    .context("Błąd zadania blokującego przy obliczaniu haszu BLAKE3")?
}
