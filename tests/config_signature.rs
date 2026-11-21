//! Signing `config.toml` (`ferry::config::signature`) against the real Secret
//! Service or Credential Manager.
//!
//! Needs a desktop session with an unlocked wallet, so it is ignored by
//! default: `cargo test --test config_signature -- --include-ignored`. It uses
//! a temporary configuration folder, so its entry in the wallet has its own
//! name and is removed again.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ferry::config::Config;
use ferry::config::check::{self, Severity};
use ferry::config::signature::{self, Status};

/// Removes the signing key from the secret store when the test ends, also
/// when it fails.
struct ForgetKey;

impl Drop for ForgetKey {
    fn drop(&mut self) {
        if let Err(e) = signature::forget() {
            eprintln!("The test's signing key stays in the secret store: {e:#}");
        }
    }
}

#[test]
#[ignore = "needs a desktop session with the system's secret store"]
fn a_change_made_outside_ferry_is_found() {
    let dir = tempfile::tempdir().unwrap();
    ferry::config::paths::set_config_dir(dir.path().to_path_buf());
    let path = Config::file_path().unwrap();

    signature::prepare().unwrap();
    let _forget = ForgetKey;
    let config = Config {
        sign_config: true,
        ..Config::default()
    };
    config.save().unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(signature::status(&path, &saved), Status::Unchanged);
    let report = check::inspect_file().unwrap();
    assert_eq!(
        report.problems[0].severity,
        Severity::Info,
        "{:?}",
        report.problems
    );

    // Another program turns on receiving programs without asking (at the
    // top: a line at the end would belong to the last section).
    let tampered = format!(
        "auto_accept_executables = true\n{}",
        String::from_utf8(saved).unwrap()
    );
    std::fs::write(&path, &tampered).unwrap();
    assert_eq!(
        signature::status(&path, tampered.as_bytes()),
        Status::Changed
    );
    let report = check::inspect_file().unwrap();
    assert_eq!(
        report.problems[0].severity,
        Severity::Warning,
        "{:?}",
        report.problems
    );

    // Saving in Ferry signs it again; turning signing off removes both.
    let mut loaded = Config::load().unwrap();
    loaded.save().unwrap();
    assert_eq!(
        signature::status(&path, &std::fs::read(&path).unwrap()),
        Status::Unchanged
    );
    assert!(loaded.auto_accept_executables, "the change itself is kept");
    loaded.sign_config = false;
    loaded.save().unwrap();
    assert!(!path.with_extension(signature::EXTENSION).exists());
}
