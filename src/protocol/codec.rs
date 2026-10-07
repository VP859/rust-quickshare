use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn write_msg<W: AsyncWriteExt + Unpin, T: Serialize>(
    mut writer: W,
    msg: &T,
) -> Result<()> {
    let bytes = postcard::to_allocvec(msg).context("Błąd serializacji wiadomości postcard")?;
    let len = bytes.len() as u32;
    writer
        .write_u32(len)
        .await
        .context("Błąd zapisu prefiksu długości ramki")?;
    writer
        .write_all(&bytes)
        .await
        .context("Błąd zapisu zawartości ramki")?;
    writer
        .flush()
        .await
        .context("Błąd opróżniania bufora zapisu")?;
    Ok(())
}

pub async fn read_msg<R: AsyncReadExt + Unpin, T: DeserializeOwned>(mut reader: R) -> Result<T> {
    let len = reader
        .read_u32()
        .await
        .context("Błąd odczytu prefiksu długości ramki")?;
    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .context("Błąd odczytu ramki z sieci")?;
    let msg = postcard::from_bytes(&buf).context("Błąd deserializacji struktury postcard")?;
    Ok(msg)
}
