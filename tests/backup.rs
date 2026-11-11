//! A device backup (`ferry::node::backup`) restored on a "new" device: the
//! key replaces the new one (which is kept as a `.bak` file) and the paired
//! devices are added, without touching devices already paired there.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ferry::config::Config;
use ferry::config::peers::PeerInfo;
use ferry::node::backup;
use ferry::node::identity::{default_key_path, load_or_generate_default_key};
use iroh::SecretKey;

fn peer(alias: &str) -> PeerInfo {
    PeerInfo {
        alias: alias.into(),
        auto_accept: false,
        added_at: chrono::Utc::now(),
        verified: true,
        ticket: None,
        discovery_key: Some("cd".repeat(32)),
    }
}

#[test]
fn a_new_device_takes_over_the_key_and_the_pairings() {
    // The old phone.
    let old_key = SecretKey::generate();
    let mut old = Config {
        device_name: Some("Kate's phone".into()),
        ..Config::default()
    };
    old.peers.insert("b".repeat(64), peer("Laptop"));
    old.peers
        .insert("c".repeat(64), peer("Old name of the tablet"));
    let file = backup::create(&old_key, &old, "a good password").unwrap();

    // The new phone: its own fresh key, one device paired already.
    let dir = tempfile::tempdir().unwrap();
    ferry::config::paths::set_config_dir(dir.path().to_path_buf());
    let mut new = Config::default();
    new.peers.insert("c".repeat(64), peer("Tablet"));
    new.save().unwrap();
    let fresh = load_or_generate_default_key().unwrap();

    let contents = backup::open(&file, "a good password").unwrap();
    let restored = backup::restore(&contents, &mut new).unwrap();
    new.save().unwrap();

    assert!(restored.key_changed);
    assert_eq!(restored.node_id, old_key.public().to_string());
    assert_eq!(restored.peers_added, 1, "the tablet was paired already");
    assert_eq!(
        load_or_generate_default_key().unwrap().public(),
        old_key.public()
    );
    // The fresh key is not lost.
    let previous = restored.previous_key.expect("previous key kept");
    assert_eq!(std::fs::read(&previous).unwrap(), fresh.to_bytes());
    assert!(default_key_path().unwrap().exists());

    let saved = Config::load().unwrap();
    assert_eq!(saved.peers.len(), 2);
    assert_eq!(saved.peers[&"b".repeat(64)].alias, "Laptop");
    assert_eq!(
        saved.peers[&"c".repeat(64)].alias,
        "Tablet",
        "kept as it was"
    );
    assert_eq!(saved.device_name.as_deref(), Some("Kate's phone"));

    // Restoring the same backup again changes nothing.
    let again = backup::restore(&contents, &mut new).unwrap();
    assert!(!again.key_changed);
    assert_eq!(again.peers_added, 0);
    assert_eq!(again.previous_key, None);
}
