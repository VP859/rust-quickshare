//! Length-prefixed framing of protocol messages.
//!
//! A frame is a 4-byte big-endian length followed by a [`postcard`] payload of at
//! most [`MAX_FRAME_LEN`] bytes.

use anyhow::{Context, Result};
use rust_i18n::t;

use crate::error::ProtocolViolation;
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::messages::MAX_FRAME_LEN;

/// Size of the big-endian length prefix of every frame.
const LEN_PREFIX: usize = 4;

/// Serializes `msg` into one frame: the length prefix followed by the payload.
///
/// # Errors
///
/// Fails when serialization fails or the payload exceeds [`MAX_FRAME_LEN`].
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    let mut frame =
        postcard::to_extend(msg, vec![0u8; LEN_PREFIX]).with_context(|| t!("codec.serialize"))?;
    let len = u32::try_from(frame.len() - LEN_PREFIX)
        .ok()
        .filter(|l| *l <= MAX_FRAME_LEN)
        .with_context(|| t!("codec.frame_too_large"))?;
    frame[..LEN_PREFIX].copy_from_slice(&len.to_be_bytes());
    Ok(frame)
}

/// Length of the payload announced by a frame's prefix.
///
/// # Errors
///
/// Fails with a [`ProtocolViolation`] when it exceeds [`MAX_FRAME_LEN`].
fn payload_len(prefix: [u8; LEN_PREFIX]) -> Result<usize> {
    let len = u32::from_be_bytes(prefix);
    if len > MAX_FRAME_LEN {
        return Err(ProtocolViolation(
            t!("codec.frame_rejected", len = len, max = MAX_FRAME_LEN).into_owned(),
        )
        .into());
    }
    // MAX_FRAME_LEN fits in usize on every supported platform.
    Ok(len as usize)
}

/// Decodes the frame at the start of `bytes`; returns the message and the
/// number of bytes it took. This is the parser for everything a peer sends on
/// a control stream, so it must handle any input without panicking.
///
/// # Errors
///
/// Fails when `bytes` holds less than a whole frame, the frame is longer than
/// [`MAX_FRAME_LEN`] or its payload does not decode as `T`.
pub fn decode_frame<T: DeserializeOwned>(bytes: &[u8]) -> Result<(T, usize)> {
    let prefix: [u8; LEN_PREFIX] = bytes
        .get(..LEN_PREFIX)
        .and_then(|p| p.try_into().ok())
        .with_context(|| t!("codec.read"))?;
    let len = payload_len(prefix)?;
    let payload = bytes
        .get(LEN_PREFIX..LEN_PREFIX + len)
        .with_context(|| t!("codec.read"))?;
    let msg = postcard::from_bytes(payload).with_context(|| t!("codec.deserialize"))?;
    Ok((msg, LEN_PREFIX + len))
}

/// Serializes `msg` and writes it as a single frame ([`encode_frame`]), then
/// flushes `writer`.
///
/// The length prefix and the payload are written with one call, so a frame
/// costs a single write on QUIC streams.
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
    let frame = encode_frame(msg)?;
    writer
        .write_all(&frame)
        .await
        .with_context(|| t!("codec.write"))?;
    writer.flush().await.with_context(|| t!("codec.write"))?;
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
    let mut prefix = [0u8; LEN_PREFIX];
    reader
        .read_exact(&mut prefix)
        .await
        .with_context(|| t!("codec.read"))?;
    let len = payload_len(prefix)?;
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .with_context(|| t!("codec.read"))?;
    postcard::from_bytes(&buf).with_context(|| t!("codec.deserialize"))
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
