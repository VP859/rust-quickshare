use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn write_msg<W, T>(writer: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let bytes = postcard::to_allocvec(msg).context("Błąd serializacji wiadomości postcard")?;
    let len = bytes.len() as u32;

    writer
        .write_all(&len.to_be_bytes())
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

pub async fn read_msg<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .context("Błąd odczytu prefiksu długości ramki")?;

    let len = u32::from_be_bytes(len_buf);

    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .context("Błąd odczytu ramki z sieci")?;

    let msg = postcard::from_bytes(&buf).context("Błąd deserializacji struktury postcard")?;
    Ok(msg)
}