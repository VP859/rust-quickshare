//! End-to-end encryption of browser sessions.
//!
//! The link of a session carries a random 256-bit key after `#`
//! (`http://…/<token>/#k=<key>`). Browsers never send that part to the
//! server, so the key only travels inside the QR code or the copied link,
//! never over the network. Everything with content – the list of shared
//! files, the files themselves, the names of the files the browser offers and
//! their content – is encrypted with it; someone who watches the network sees
//! only sizes and timing. The page does the same with
//! [`@noble/ciphers`](https://github.com/paulmillr/noble-ciphers) (`page/seal.js`):
//! browsers offer WebCrypto only to HTTPS pages.
//!
//! **Limit.** The page itself comes over plain HTTP: an *active* attacker on
//! the network who can change it can also make it send the key away. This
//! protects against listening, not against tampering.
//!
//! # Format
//!
//! A message (a file, or a JSON text) is a **stream**: an 8-byte random
//! prefix, then records of [`RECORD`] bytes of plaintext (the last one
//! shorter, possibly empty), each encrypted with ChaCha20-Poly1305 (RFC 8439)
//! and followed by its 16-byte tag. The nonce of record `i` is
//! `prefix ‖ big-endian u32 (i | 0x8000_0000 for the last record)`, so records
//! cannot be reordered, dropped, repeated or cut off without the tag check
//! failing (the STREAM construction). The associated data names what the
//! stream is (`ferry-web-v1 download 3`, …), so one stream cannot stand in
//! for another.

use anyhow::{Context, Result};

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};

/// Plaintext bytes per record.
pub const RECORD: usize = 64 * 1024;
/// Length of the authentication tag of every record.
pub const TAG: usize = 16;
/// Length of the random prefix at the start of a stream.
pub const PREFIX: usize = 8;
/// Bit of the counter that marks the last record.
const LAST: u32 = 0x8000_0000;

/// What a stream is: its associated data.
pub mod purpose {
    /// The session info (`api/info`).
    pub const INFO: &str = "ferry-web-v1 info";
    /// The files the browser offers (`api/offer`).
    pub const OFFER: &str = "ferry-web-v1 offer";

    /// Shared file `index` (`api/files/<index>`).
    pub fn download(index: usize) -> String {
        format!("ferry-web-v1 download {index}")
    }

    /// File `index` of the accepted offer `offer` (`api/upload/<offer>/<index>`).
    pub fn upload(offer: &str, index: usize) -> String {
        format!("ferry-web-v1 upload {offer} {index}")
    }
}

/// The key of a session.
#[derive(Clone)]
pub struct SessionKey([u8; 32]);

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKey(…)")
    }
}

impl SessionKey {
    /// A new random key.
    ///
    /// # Errors
    ///
    /// Fails only when the system has no random numbers.
    pub fn generate() -> Result<Self> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
        Ok(Self(key))
    }

    /// The key from its bytes (tests, and the other side of a session).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The key in the part of a link after `#` (`k=…`, see [`SessionKey::fragment`]).
    pub fn from_fragment(fragment: &str) -> Option<Self> {
        use base64::Engine as _;
        let encoded = fragment
            .trim_start_matches('#')
            .split('&')
            .find_map(|part| part.strip_prefix("k="))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    /// The part of the link after `#`: `k=` and the key in URL-safe base64.
    pub fn fragment(&self) -> String {
        use base64::Engine as _;
        format!(
            "k={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0)
        )
    }

    /// The ChaCha20-Poly1305 key.
    fn aead(&self) -> LessSafeKey {
        // A 32-byte key is always accepted.
        LessSafeKey::new(
            UnboundKey::new(&CHACHA20_POLY1305, &self.0)
                .unwrap_or_else(|_| unreachable!("ChaCha20-Poly1305 takes 32-byte keys")),
        )
    }
}

/// Number of records of a stream with `len` bytes of plaintext.
fn records(len: u64) -> u64 {
    len.div_ceil(RECORD as u64).max(1)
}

/// Length of the stream of `len` bytes of plaintext.
pub fn sealed_len(len: u64) -> u64 {
    PREFIX as u64 + len + TAG as u64 * records(len)
}

/// Length of the plaintext in a stream of `len` bytes, if there can be one.
pub fn plain_len(len: u64) -> Option<u64> {
    let body = len.checked_sub(PREFIX as u64)?;
    let records = body.div_ceil((RECORD + TAG) as u64).max(1);
    let plain = body.checked_sub(records * TAG as u64)?;
    (sealed_len(plain) == len).then_some(plain)
}

/// The nonce of record `index` of the stream with `prefix`.
fn nonce(prefix: &[u8; PREFIX], index: u64, last: bool) -> Result<Nonce> {
    let counter = u32::try_from(index)
        .ok()
        .filter(|c| c & LAST == 0)
        .context("stream too long")?;
    let mut bytes = [0u8; 12];
    bytes[..PREFIX].copy_from_slice(prefix);
    bytes[PREFIX..].copy_from_slice(&(counter | if last { LAST } else { 0 }).to_be_bytes());
    Ok(Nonce::assume_unique_for_key(bytes))
}

/// Encrypts a stream record by record.
pub struct Sealer {
    /// The key.
    key: LessSafeKey,
    /// What the stream is.
    aad: String,
    /// Its random prefix.
    prefix: [u8; PREFIX],
    /// Index of the next record.
    index: u64,
}

impl Sealer {
    /// A new stream with a random prefix; [`Sealer::prefix`] goes first.
    ///
    /// # Errors
    ///
    /// Fails only when the system has no random numbers.
    pub fn new(key: &SessionKey, aad: &str) -> Result<Self> {
        let mut prefix = [0u8; PREFIX];
        getrandom::fill(&mut prefix).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
        Ok(Self::with_prefix(key, aad, prefix))
    }

    /// A stream with a chosen prefix (test vectors).
    pub fn with_prefix(key: &SessionKey, aad: &str, prefix: [u8; PREFIX]) -> Self {
        Self {
            key: key.aead(),
            aad: aad.to_string(),
            prefix,
            index: 0,
        }
    }

    /// The first bytes of the stream.
    pub fn prefix(&self) -> [u8; PREFIX] {
        self.prefix
    }

    /// Encrypts the next record (at most [`RECORD`] bytes; only the last one
    /// may be shorter) and returns it with its tag.
    ///
    /// # Errors
    ///
    /// Fails for a record that is too long or a stream of more than 2³¹ records.
    pub fn seal(&mut self, plain: &[u8], last: bool) -> Result<Vec<u8>> {
        anyhow::ensure!(plain.len() <= RECORD, "record too long");
        let mut record = plain.to_vec();
        self.key
            .seal_in_place_append_tag(
                nonce(&self.prefix, self.index, last)?,
                Aad::from(self.aad.as_bytes()),
                &mut record,
            )
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        self.index += 1;
        Ok(record)
    }
}

/// Encrypts a whole message (a JSON text) as one stream.
///
/// # Errors
///
/// Fails only when the system has no random numbers.
pub fn seal_all(key: &SessionKey, aad: &str, plain: &[u8]) -> Result<Vec<u8>> {
    seal_with(&mut Sealer::new(key, aad)?, plain)
}

/// [`seal_all`] with a given sealer (test vectors use a fixed prefix).
///
/// # Errors
///
/// Fails for a message of more than 2³¹ records.
pub fn seal_with(sealer: &mut Sealer, plain: &[u8]) -> Result<Vec<u8>> {
    let mut out = sealer.prefix().to_vec();
    let count = records(plain.len() as u64);
    let mut chunks = plain.chunks(RECORD);
    for index in 0..count {
        let chunk = chunks.next().unwrap_or_default();
        out.extend(sealer.seal(chunk, index + 1 == count)?);
    }
    Ok(out)
}

/// Decrypts a stream of a known plaintext length as its bytes arrive.
pub struct Opener {
    /// The key.
    key: LessSafeKey,
    /// What the stream must be.
    aad: String,
    /// Its prefix, once read.
    prefix: Option<[u8; PREFIX]>,
    /// Bytes that do not make a whole record yet.
    pending: Vec<u8>,
    /// Plaintext bytes still expected.
    left: u64,
    /// Index of the next record.
    index: u64,
    /// The last record has been read.
    done: bool,
}

impl Opener {
    /// Expects a stream of `plain_len` bytes of plaintext.
    pub fn new(key: &SessionKey, aad: &str, plain_len: u64) -> Self {
        Self {
            key: key.aead(),
            aad: aad.to_string(),
            prefix: None,
            pending: Vec::new(),
            left: plain_len,
            index: 0,
            done: false,
        }
    }

    /// Takes the next bytes of the stream and returns the plaintext of every
    /// record they complete.
    ///
    /// # Errors
    ///
    /// Fails when a record does not decrypt (wrong key, changed, reordered or
    /// foreign data) or data follows the last record.
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.pending.extend_from_slice(data);
        let mut out = Vec::new();
        if self.prefix.is_none() {
            if self.pending.len() < PREFIX {
                return Ok(out);
            }
            let prefix: [u8; PREFIX] = self.pending[..PREFIX].try_into()?;
            self.prefix = Some(prefix);
            self.pending.drain(..PREFIX);
        }
        loop {
            if self.done {
                anyhow::ensure!(self.pending.is_empty(), "data after the end of the stream");
                return Ok(out);
            }
            let plain = usize::try_from(self.left.min(RECORD as u64))?;
            let last = self.left <= RECORD as u64;
            if self.pending.len() < plain + TAG {
                return Ok(out);
            }
            let mut record: Vec<u8> = self.pending.drain(..plain + TAG).collect();
            let prefix = self.prefix.context("no prefix")?;
            let opened = self
                .key
                .open_in_place(
                    nonce(&prefix, self.index, last)?,
                    Aad::from(self.aad.as_bytes()),
                    &mut record,
                )
                .map_err(|_| anyhow::anyhow!("the data does not decrypt with the session key"))?;
            out.extend_from_slice(opened);
            self.index += 1;
            self.left -= plain as u64;
            self.done = last;
        }
    }

    /// Checks that the whole stream arrived.
    ///
    /// # Errors
    ///
    /// Fails when it ended early.
    pub fn finish(&self) -> Result<()> {
        anyhow::ensure!(
            self.done && self.pending.is_empty(),
            "the encrypted data ended early"
        );
        Ok(())
    }
}

/// Decrypts a whole message made by [`seal_all`].
///
/// # Errors
///
/// Fails when it does not decrypt or is cut off.
pub fn open_all(key: &SessionKey, aad: &str, sealed: &[u8]) -> Result<Vec<u8>> {
    let len = plain_len(sealed.len() as u64).context("not an encrypted message")?;
    let mut opener = Opener::new(key, aad, len);
    let plain = opener.push(sealed)?;
    opener.finish()?;
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SessionKey {
        SessionKey::from_bytes([7; 32])
    }

    #[test]
    fn messages_of_every_size_go_and_come_back() {
        for len in [0, 1, RECORD - 1, RECORD, RECORD + 1, 3 * RECORD + 5] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let sealed = seal_all(&key(), "x", &plain).unwrap();
            assert_eq!(
                sealed.len() as u64,
                sealed_len(len as u64),
                "length of {len}"
            );
            assert_eq!(plain_len(sealed.len() as u64), Some(len as u64));
            assert_eq!(open_all(&key(), "x", &sealed).unwrap(), plain);
            // In pieces of any size, as the network delivers them.
            let mut opener = Opener::new(&key(), "x", len as u64);
            let mut back = Vec::new();
            for piece in sealed.chunks(7777) {
                back.extend(opener.push(piece).unwrap());
            }
            opener.finish().unwrap();
            assert_eq!(back, plain);
        }
    }

    #[test]
    fn anything_changed_cut_reordered_or_foreign_is_refused() {
        let plain = vec![42u8; 2 * RECORD + 10];
        let sealed = seal_all(&key(), "file 1", &plain).unwrap();
        assert!(
            open_all(&SessionKey::from_bytes([8; 32]), "file 1", &sealed).is_err(),
            "other key"
        );
        assert!(open_all(&key(), "file 2", &sealed).is_err(), "another file");
        let mut changed = sealed.clone();
        changed[PREFIX + 5] ^= 1;
        assert!(
            open_all(&key(), "file 1", &changed).is_err(),
            "changed byte"
        );
        // Cut after whole records: the last-record flag is missing.
        let cut = &sealed[..PREFIX + 2 * (RECORD + TAG)];
        let mut opener = Opener::new(&key(), "file 1", 2 * RECORD as u64);
        assert!(
            opener.push(cut).is_err(),
            "a middle record is not a last one"
        );
        // Records swapped.
        let r = RECORD + TAG;
        let mut swapped = sealed[..PREFIX].to_vec();
        swapped.extend_from_slice(&sealed[PREFIX + r..PREFIX + 2 * r]);
        swapped.extend_from_slice(&sealed[PREFIX..PREFIX + r]);
        swapped.extend_from_slice(&sealed[PREFIX + 2 * r..]);
        assert!(
            open_all(&key(), "file 1", &swapped).is_err(),
            "swapped records"
        );
        // Data after the end, and an early end.
        let mut longer = sealed.clone();
        longer.extend_from_slice(&[0; 17]);
        let mut opener = Opener::new(&key(), "file 1", plain.len() as u64);
        assert!(opener.push(&longer).is_err(), "data after the end");
        let mut opener = Opener::new(&key(), "file 1", plain.len() as u64);
        opener.push(&sealed[..sealed.len() - 1]).unwrap();
        assert!(opener.finish().is_err(), "ended early");
    }

    #[test]
    fn streams_never_repeat_their_prefix() {
        let a = seal_all(&key(), "x", b"same").unwrap();
        let b = seal_all(&key(), "x", b"same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn the_key_goes_into_the_link_fragment() {
        let fragment = SessionKey::from_bytes([0xFB; 32]).fragment();
        assert_eq!(fragment, "k=-_v7-_v7-_v7-_v7-_v7-_v7-_v7-_v7-_v7-_v7-_s");
        let back = SessionKey::from_fragment(&format!("#{fragment}")).unwrap();
        assert_eq!(back.0, [0xFB; 32]);
        assert!(SessionKey::from_fragment("#k=short").is_none());
        assert_eq!(format!("{:?}", key()), "SessionKey(…)", "never printed");
    }

    /// The same bytes as `tests/gui/web-crypto.spec.js` expects from the page's
    /// implementation: both sides agree.
    #[test]
    fn test_vector_shared_with_the_page() {
        let key = SessionKey::from_bytes(std::array::from_fn(|i| i as u8));
        let mut sealer = Sealer::with_prefix(&key, purpose::INFO, [1, 2, 3, 4, 5, 6, 7, 8]);
        let sealed = seal_with(&mut sealer, b"{\"device\":\"Kate's laptop\"}").unwrap();
        let hex: String = sealed.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, VECTOR);
    }

    /// See [`test_vector_shared_with_the_page`].
    const VECTOR: &str = "0102030405060708da7530383449e3150ef4fe615522431dede1f5dd248679cf1e0399bebb9ce65897d6bf024c428baa4a07";
}
