//! Property-based tests of everything that parses data from other devices:
//! frames, offers, file and folder names, TICKETs and pairing codes. Each
//! property is checked on hundreds of generated inputs (`PROPTEST_CASES`
//! raises the number).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferry::node::NodeTicket;
use ferry::pairing::pairing_code;
use ferry::protocol::codec::{decode_frame, encode_frame};
use ferry::protocol::messages::{
    BatchOffer, ChunkHeader, Hello, MAX_CHUNK_SIZE, MAX_SEND_CHUNK_SIZE, MIN_CHUNK_SIZE,
    MIN_SEND_CHUNK_SIZE, TransferDecision, TransferOffer, TransferResult, chunk_size_for,
    chunk_window,
};
use ferry::transfer::security::{
    resolve_secure_folder, resolve_secure_target_path, sanitize_filename,
};
use proptest::prelude::*;

/// Names with the characters that matter for paths: separators, dots,
/// spaces, control characters, Windows-reserved stems and non-ASCII letters.
fn tricky_name() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[./\\\\ a-zA-Z0-9:*?\"<>|\u{0}\n\tżółćCONcon]{0,40}",
        Just("..".to_string()),
        Just("CON.txt".to_string()),
    ]
}

fn offer() -> impl Strategy<Value = TransferOffer> {
    (
        any::<String>(),
        any::<u64>(),
        any::<u32>(),
        any::<[u8; 32]>(),
        prop::collection::vec(any::<String>(), 0..20),
    )
        .prop_map(
            |(file_name, file_size, chunk_size, file_hash, folder)| TransferOffer {
                file_name,
                file_size,
                chunk_size,
                file_hash,
                folder,
            },
        )
}

proptest! {
    /// The frame parser never panics, whatever a peer sends.
    #[test]
    fn decoding_arbitrary_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_frame::<Hello>(&bytes);
        let _ = decode_frame::<TransferOffer>(&bytes);
        let _ = decode_frame::<TransferDecision>(&bytes);
        let _ = decode_frame::<ChunkHeader>(&bytes);
        let _ = decode_frame::<TransferResult>(&bytes);
        let _ = decode_frame::<BatchOffer>(&bytes);
    }

    /// Every frame decodes to the message it was made from, using exactly its
    /// length; a truncated frame never decodes.
    #[test]
    fn frames_round_trip(offer in offer(), cut in any::<prop::sample::Index>()) {
        let frame = match encode_frame(&offer) {
            Ok(frame) => frame,
            Err(_) => return Ok(()), // larger than a frame: refused when sending
        };
        let (back, used) = decode_frame::<TransferOffer>(&frame).unwrap();
        prop_assert_eq!(&back, &offer);
        prop_assert_eq!(used, frame.len());
        let short = cut.index(frame.len());
        prop_assert!(decode_frame::<TransferOffer>(&frame[..short]).is_err());
    }

    /// An offer that passes validation has sizes the receiver can work with.
    #[test]
    fn valid_offers_are_within_limits(offer in offer()) {
        if offer.validate().is_ok() {
            prop_assert!((MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE).contains(&offer.chunk_size));
            prop_assert!(!offer.file_name.is_empty() && offer.file_name.len() <= 255);
            prop_assert!(offer.folder.len() <= 16);
        }
    }

    /// The chunks of a file cover it exactly, without gaps or overlaps.
    #[test]
    fn chunks_cover_the_file(chunks in 0u64..2000, extra in 0u64..MIN_CHUNK_SIZE as u64,
                             chunk_size in MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE) {
        let file_size = (chunks * chunk_size as u64).saturating_sub(extra);
        let offer = TransferOffer {
            file_name: "f".into(),
            file_size,
            chunk_size,
            file_hash: [0; 32],
            folder: Vec::new(),
        };
        let total: u64 = (0..offer.total_chunks())
            .map(|i| u64::from(offer.chunk_len(i).unwrap()))
            .sum();
        prop_assert_eq!(total, file_size);
        prop_assert_eq!(offer.chunk_len(offer.total_chunks()), None);
    }

    /// Senders pick power-of-two chunks within their range that grow with the
    /// file, and the window keeps the buffered data bounded.
    #[test]
    fn chunk_policy(a in any::<u64>(), b in any::<u64>(), peer_chunk in any::<u32>()) {
        let (small, large) = (a.min(b), a.max(b));
        let size = chunk_size_for(small);
        prop_assert!(size.is_power_of_two());
        prop_assert!((MIN_SEND_CHUNK_SIZE..=MAX_SEND_CHUNK_SIZE).contains(&size));
        prop_assert!(chunk_size_for(large) >= size);
        if (MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE).contains(&peer_chunk) {
            prop_assert!(chunk_window(peer_chunk) as u64 * u64::from(peer_chunk) <= 64 << 20);
        }
    }

    /// A sanitized name is a single, visible file name and stays the same when
    /// sanitized again.
    #[test]
    fn sanitized_names_are_single_safe_names(raw in tricky_name()) {
        if let Ok(name) = sanitize_filename(&raw) {
            prop_assert!(!name.is_empty());
            prop_assert!(!name.contains(['/', '\\', '\0']));
            prop_assert!(!name.contains(".."));
            prop_assert!(!name.chars().any(char::is_control));
            prop_assert!(!name.starts_with('.') && !name.ends_with(['.', ' ']));
            prop_assert_eq!(sanitize_filename(&name).unwrap(), name);
        }
    }

    /// Whatever a peer names a file or its folders, the result is inside the
    /// download folder.
    #[test]
    fn received_paths_stay_in_the_download_folder(name in tricky_name(),
                                                   folder in prop::collection::vec(tricky_name(), 0..4)) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        if let Ok(path) = resolve_secure_target_path(&base, &name) {
            prop_assert!(path.starts_with(&base) && path != base);
        }
        if let Ok(path) = resolve_secure_folder(&base, &folder) {
            prop_assert!(path.starts_with(&base));
        }
    }

    /// Parsing a TICKET never panics, and a TICKET survives a round trip,
    /// also when wrapped by a terminal.
    #[test]
    fn tickets(garbage in any::<String>(), seed in any::<[u8; 32]>(), token in any::<Option<[u8; 16]>>()) {
        let _ = NodeTicket::deserialize_from_str(&garbage);
        let _ = NodeTicket::parse_target(&garbage);
        let id = iroh::SecretKey::from_bytes(&seed).public();
        let ticket = match token {
            Some(t) => NodeTicket::with_share(id.into(), t),
            None => NodeTicket::new(id.into()),
        };
        let text = ticket.serialize_to_string().unwrap();
        let wrapped: String = text.chars().enumerate()
            .flat_map(|(i, c)| (i % 40 == 39).then_some('\n').into_iter().chain([c]))
            .collect();
        let back = NodeTicket::deserialize_from_str(&wrapped).unwrap();
        prop_assert_eq!(back.addr.id, id);
        prop_assert_eq!(back.share, token);
    }

    /// Pairing codes are always six digits split by a space.
    #[test]
    fn pairing_codes(a in any::<[u8; 32]>(), b in any::<[u8; 32]>(),
                     na in any::<[u8; 32]>(), nb in any::<[u8; 32]>()) {
        let (a, b) = (iroh::SecretKey::from_bytes(&a).public(), iroh::SecretKey::from_bytes(&b).public());
        let code = pairing_code(&a, &b, &na, &nb);
        let well_formed = code.len() == 7
            && code.char_indices().all(|(i, c)| (i == 3 && c == ' ') || (i != 3 && c.is_ascii_digit()));
        prop_assert!(well_formed, "malformed code {:?}", code);
    }
}
