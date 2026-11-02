//! Test vectors of the Ferry protocol (`docs/protocol-vectors.json`).
//!
//! The file lists, for fixed keys and inputs, the exact bytes of every message,
//! the pairing commitment and code, TICKETs, a signed discovery announcement and
//! the sanitized file names and chunk sizes. Another implementation of the
//! protocol can check itself against the file; this test checks that the file
//! and this implementation agree, so an accidental change of the wire format
//! (a reordered enum variant, a different hash input) fails here.
//!
//! After an *intended*, versioned change run
//! `FERRY_UPDATE_VECTORS=1 cargo test --test protocol_vectors` and review the diff.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferry::discovery::{Announcement, sign_announcement, verify_announcement};
use ferry::node::NodeTicket;
use ferry::pairing::{pairing_code, pairing_commitment};
use ferry::protocol::codec::{decode_frame, encode_frame};
use ferry::protocol::messages::{
    BatchDecision, BatchOffer, ChunkHeader, Hello, PairChallenge, PairResponse, PairReveal,
    TextDecision, TransferDecision, TransferOffer, TransferResult, chunk_size_for, chunk_window,
};
use ferry::protocol::version::ProtocolVersion;
use ferry::transfer::security::sanitize_filename;
use iroh::{EndpointAddr, SecretKey, TransportAddr};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::fmt::Debug;
use std::net::SocketAddr;

/// Path of the vectors, relative to the repository root.
const FILE: &str = "docs/protocol-vectors.json";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Fixed key: 32 bytes of `seed`. Never use such keys outside tests.
fn key(seed: u8) -> SecretKey {
    SecretKey::from_bytes(&[seed; 32])
}

/// A frame (length prefix and postcard payload) of `message` as a vector entry;
/// also checks that it decodes back to the same message.
fn frame<T>(name: &str, message: &T) -> Value
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let bytes = encode_frame(message).unwrap();
    let (decoded, used) = decode_frame::<T>(&bytes).unwrap();
    assert_eq!(&decoded, message, "{name} round trip");
    assert_eq!(used, bytes.len());
    json!({
        "name": name,
        "frame": hex(&bytes),
        "payload_length": bytes.len() - 4,
    })
}

fn addr(seed: u8) -> EndpointAddr {
    EndpointAddr::from_parts(
        key(seed).public(),
        [TransportAddr::Ip(SocketAddr::from((
            [192, 168, 1, seed],
            47824,
        )))],
    )
}

fn frames() -> Vec<Value> {
    let hash = *blake3::hash(b"ferry").as_bytes();
    vec![
        frame("Hello::Push", &Hello::Push),
        frame("Hello::Pull", &Hello::Pull { token: [0x11; 16] }),
        frame(
            "Hello::Pair",
            &Hello::Pair {
                name: "Alice's laptop".into(),
                addr: addr(1),
                commitment: pairing_commitment(&key(1).public(), &[0xa1; 32]),
            },
        ),
        frame(
            "Hello::Text",
            &Hello::Text {
                text: "https://example.org/zażółć".into(),
            },
        ),
        frame(
            "Hello::Batch",
            &Hello::Batch(BatchOffer {
                count: 3,
                total_size: 3_145_728,
                names: vec!["a.jpg".into(), "Holiday/b.jpg".into(), "c.mp4".into()],
            }),
        ),
        frame(
            "TransferOffer",
            &TransferOffer {
                file_name: "photo.jpg".into(),
                file_size: 5_242_880,
                chunk_size: 1_048_576,
                file_hash: hash,
                folder: vec!["Holiday".into(), "day 1".into()],
            },
        ),
        frame(
            "TransferDecision::Accept",
            &TransferDecision::Accept {
                start_chunk_index: 2,
            },
        ),
        frame(
            "TransferDecision::Reject",
            &TransferDecision::Reject {
                reason: "Not now".into(),
            },
        ),
        frame(
            "ChunkHeader",
            &ChunkHeader {
                chunk_index: 4,
                payload_len: 1_048_576,
                chunk_hash: hash,
            },
        ),
        frame(
            "TransferResult::Progress",
            &TransferResult::Progress {
                received: 2_097_152,
            },
        ),
        frame("TransferResult::Verified", &TransferResult::Verified),
        frame(
            "TransferResult::Failed",
            &TransferResult::Failed {
                reason: "Hash mismatch".into(),
            },
        ),
        frame("BatchDecision::Accepted", &BatchDecision::Accepted),
        frame("BatchDecision::AskEach", &BatchDecision::AskEach),
        frame(
            "BatchDecision::Rejected",
            &BatchDecision::Rejected {
                reason: "No".into(),
            },
        ),
        frame("TextDecision::Accepted", &TextDecision::Accepted),
        frame(
            "TextDecision::Rejected",
            &TextDecision::Rejected {
                reason: "No".into(),
            },
        ),
        frame("PairChallenge", &PairChallenge { nonce: [0xb2; 32] }),
        frame("PairReveal", &PairReveal { nonce: [0xa1; 32] }),
        frame(
            "PairResponse (accepted)",
            &PairResponse {
                accepted: true,
                name: "Bob's phone".into(),
                addr: Some(addr(2)),
            },
        ),
        frame(
            "PairResponse (declined)",
            &PairResponse {
                accepted: false,
                name: "Bob's phone".into(),
                addr: None,
            },
        ),
    ]
}

fn pairing() -> Value {
    let (initiator, responder) = (key(1).public(), key(2).public());
    let (nonce_a, nonce_b) = ([0xa1; 32], [0xb2; 32]);
    json!({
        "initiator_node_id": initiator.to_string(),
        "responder_node_id": responder.to_string(),
        "initiator_nonce": hex(&nonce_a),
        "responder_nonce": hex(&nonce_b),
        "commitment": hex(&pairing_commitment(&initiator, &nonce_a)),
        "code": pairing_code(&initiator, &responder, &nonce_a, &nonce_b),
    })
}

fn tickets() -> Vec<Value> {
    let id = key(1).public();
    let entry = |name: &str, ticket: NodeTicket| {
        let text = ticket.serialize_to_string().unwrap();
        let back = NodeTicket::deserialize_from_str(&text).unwrap();
        assert_eq!(back.addr, ticket.addr, "{name}");
        assert_eq!(back.share, ticket.share, "{name}");
        json!({ "name": name, "ticket": text })
    };
    vec![
        entry("pairing, Node ID only", NodeTicket::new(id.into())),
        entry("pairing, one IP address", NodeTicket::new(addr(1))),
        entry(
            "shared file, one IP address",
            NodeTicket::with_share(addr(1), [0x22; 16]),
        ),
    ]
}

fn announcement() -> Value {
    let alice = key(1);
    let announcement = Announcement {
        timestamp: 1_800_000_000,
        name: "Alice's laptop".into(),
        platform: "linux".into(),
        addr: addr(1),
        quic_ports: vec![47824],
        reply: false,
    };
    let datagram = sign_announcement(&alice, &announcement).unwrap();
    assert_eq!(
        verify_announcement(&datagram, announcement.timestamp + 300),
        Some(announcement.clone()),
        "valid within 300 s"
    );
    assert!(verify_announcement(&datagram, announcement.timestamp + 301).is_none());
    assert!(verify_announcement(&datagram, announcement.timestamp - 301).is_none());
    json!({
        "signing_key_seed": hex(&[1; 32]),
        "node_id": alice.public().to_string(),
        "timestamp": announcement.timestamp,
        "datagram": hex(&datagram),
        "valid_from": announcement.timestamp - 300,
        "valid_until": announcement.timestamp + 300,
    })
}

fn chunk_policy() -> Vec<Value> {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    [0, 1, 5 * MIB, GIB, GIB + 1, 4 * GIB, 5 * GIB, 100 * GIB]
        .into_iter()
        .map(|size| {
            let chunk = chunk_size_for(size);
            json!({ "file_size": size, "chunk_size": chunk, "window": chunk_window(chunk) })
        })
        .collect()
}

fn file_names() -> Vec<Value> {
    [
        "report.pdf",
        "zdjęcie.jpg",
        "CON.txt",
        ".bashrc",
        ". A",
        "a\nb:c.txt ",
        "../../etc/passwd",
        "/etc/passwd",
        "a/b",
        "..",
        "",
    ]
    .into_iter()
    .map(|input| {
        let result = sanitize_filename(input);
        json!({ "input": input, "sanitized": result.ok() })
    })
    .collect()
}

fn vectors() -> Value {
    json!({
        "about": "Test vectors of the Ferry protocol. Generated by tests/protocol_vectors.rs; see docs/PROTOCOL.md.",
        "protocol": ProtocolVersion::CURRENT.to_string(),
        "encoding": "Frames are a 4-byte big-endian length followed by the postcard payload (hex).",
        "keys": {
            "note": "Ed25519 secret keys of 32 bytes of the seed; Node ID = public key (hex).",
            "1": { "seed": hex(&[1; 32]), "node_id": key(1).public().to_string() },
            "2": { "seed": hex(&[2; 32]), "node_id": key(2).public().to_string() },
        },
        "frames": frames(),
        "pairing": pairing(),
        "tickets": tickets(),
        "announcement": announcement(),
        "chunk_policy": chunk_policy(),
        "file_names": file_names(),
    })
}

#[test]
fn the_vectors_document_matches_the_implementation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE);
    let mut current = serde_json::to_string_pretty(&vectors()).unwrap();
    current.push('\n');
    if std::env::var_os("FERRY_UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &current).unwrap();
        return;
    }
    let saved = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        saved == current,
        "{FILE} is out of date. If the protocol change is intended (and versioned), run\n  \
         FERRY_UPDATE_VECTORS=1 cargo test --test protocol_vectors\nand review the diff."
    );
}

#[test]
fn encoding_is_deterministic() {
    assert_eq!(vectors(), vectors());
}
