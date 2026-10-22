use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::messages::MAX_FRAME_LEN;

/// Zapisuje ramkę postcard poprzedzoną 4-bajtowym prefiksem długości (big-endian).
pub async fn write_msg<W, T>(writer: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let bytes = postcard::to_allocvec(msg).context("Błąd serializacji wiadomości postcard")?;
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME_LEN)
        .context("Ramka przekracza maksymalny dozwolony rozmiar")?;

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

/// Odczytuje ramkę zapisaną przez [`write_msg`]. Ramki dłuższe niż
/// [`MAX_FRAME_LEN`] są odrzucane przed alokacją bufora.
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
    if len > MAX_FRAME_LEN {
        anyhow::bail!(
            "Odrzucono ramkę o rozmiarze {} B (limit {} B)",
            len,
            MAX_FRAME_LEN
        );
    }

    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .context("Błąd odczytu ramki z sieci")?;

    let msg = postcard::from_bytes(&buf).context("Błąd deserializacji struktury postcard")?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::messages::Hello;

    #[tokio::test]
    async fn roundtrip() -> Result<()> {
        let mut buf = Vec::new();
        write_msg(&mut buf, &Hello::Pull { token: [7; 16] }).await?;
        let msg: Hello = read_msg(&mut buf.as_slice()).await?;
        assert_eq!(msg, Hello::Pull { token: [7; 16] });
        Ok(())
    }

    #[tokio::test]
    async fn rejects_oversized_frame() {
        let mut buf = u32::MAX.to_be_bytes().to_vec();
        buf.extend_from_slice(&[0; 16]);
        let res: Result<Hello> = read_msg(&mut buf.as_slice()).await;
        assert!(res.is_err());
    }
}
