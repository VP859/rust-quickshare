//! The device key in the system's secret store (`ferry::node::keystore`)
//! against the real Secret Service or Credential Manager.
//!
//! Needs a desktop session with an unlocked wallet, so it is ignored by
//! default: `cargo test --test keystore -- --include-ignored`. It uses a
//! temporary configuration folder, so its entry in the wallet has its own name
//! and is removed again.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ferry::config::Config;
use ferry::node::identity::{default_key_path, load_or_generate_default_key, set_key_storage};
use ferry::node::keystore::KeyStorage;

#[test]
#[ignore = "needs a desktop session with the system's secret store"]
fn the_device_key_moves_into_the_system_store_and_back() {
    let dir = tempfile::tempdir().unwrap();
    ferry::config::paths::set_config_dir(dir.path().to_path_buf());
    Config::default().save().unwrap();

    let original = load_or_generate_default_key().unwrap();
    let file = default_key_path().unwrap();
    assert!(file.exists());

    set_key_storage(KeyStorage::System).unwrap();
    assert_eq!(Config::load().unwrap().key_storage, KeyStorage::System);
    assert!(!file.exists(), "no key file any more");
    assert_eq!(
        load_or_generate_default_key().unwrap().public(),
        original.public(),
        "same identity, read from the secret store"
    );

    set_key_storage(KeyStorage::File).unwrap();
    assert_eq!(Config::load().unwrap().key_storage, KeyStorage::File);
    assert!(file.exists());
    assert_eq!(
        load_or_generate_default_key().unwrap().public(),
        original.public()
    );
}
