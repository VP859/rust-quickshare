//! Length-prefixed framing of protocol messages.
//!
//! A frame is a 4-byte big-endian length followed by a [`postcard`] payload of at
//! most [`MAX_FRAME_LEN`] bytes.

use anyhow::{Context, Result};
use rust_i18n::t;
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::messages::MAX_FRAME_LEN;

/// Serializes `msg` and writes it as a single frame, then flushes `writer`.
///
/// # Errors
///
/// Fails when serialization fails, the frame exceeds [`MAX_FRAME_LEN`] or the
/// underlying writer returns an I/O error.
pub async fn write_msg<W, T>(writer: &mut W, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let bytes = postcard::to_allocvec(msg).context(t!("codec.serialize"))?;
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME_LEN)
        .context(t!("codec.frame_too_large"))?;

    writer
        .write_all(&len.to_be_bytes())
        .await
        .context(t!("codec.write"))?;
    writer.write_all(&bytes).await.context(t!("codec.write"))?;
    writer.flush().await.context(t!("codec.write"))?;
    Ok(())
}

/// Reads one frame written by [`write_msg`] and deserializes it.
///
/// Frames longer than [`MAX_FRAME_LEN`] are rejected before any buffer is
/// allocated, so a malicious peer cannot force a large allocation.
///
/// # Errors
///
/// Fails on I/O errors, oversized frames and payloads that do not decode as `T`.
pub async fn read_msg<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .context(t!("codec.read"))?;

    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        anyhow::bail!(t!("codec.frame_rejected", len = len, max = MAX_FRAME_LEN));
    }

    let mut buf = vec![0u8; len as usize];
    reader
        .read_exact(&mut buf)
        .await
        .context(t!("codec.read"))?;
    postcard::from_bytes(&buf).context(t!("codec.deserialize"))
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
