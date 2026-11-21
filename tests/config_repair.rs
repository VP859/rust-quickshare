//! Repairing a damaged `config.toml` (`ferry::config::check::repair`) on a
//! real file, as the GUI does when the file could not be read at start. Its
//! own test binary: the configuration folder and blocking saves apply to the
//! whole process.

use ferry::config::Config;
use ferry::config::check::{self, BEFORE_REPAIR};

#[test]
fn a_damaged_file_is_repaired_keeping_the_trusted_devices() {
    let dir = tempfile::tempdir().unwrap();
    ferry::config::paths::set_config_dir(dir.path().to_path_buf());
    let path = Config::file_path().unwrap();
    let id = iroh::SecretKey::generate().public().to_string();
    let broken = format!(
        "device_name = \"Desk\"\npaired_only = tru\n\n[peers.{id}]\nalias = \"Laptop\"\nauto_accept = true\nadded_at = \"2026-11-01T10:00:00Z\"\n"
    );
    std::fs::write(&path, &broken).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();

    // As the GUI starts: the defaults, saving blocked.
    let (_, error) = Config::load_or_defaults();
    assert!(error.is_some());

    let report = check::inspect_file().unwrap();
    assert!(report.repairable(), "{:?}", report.problems);
    assert_eq!(report.problems[0].line, Some(2));

    let repaired = check::repair().unwrap();
    assert!(
        repaired.paired_only,
        "the safer choice for the lost setting"
    );
    assert_eq!(repaired.device_name.as_deref(), Some("Desk"));
    assert!(
        repaired.peers[&id].auto_accept,
        "a valid setting is kept as it was"
    );

    // The file reads again, the old one is kept, and saving works again.
    let loaded = Config::load().unwrap();
    assert_eq!(loaded.peers[&id].alias, "Laptop");
    let before = path.with_extension(BEFORE_REPAIR);
    assert_eq!(std::fs::read_to_string(before).unwrap(), broken);
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions())
            & 0o777,
        0o600,
        "only the owner reads the repaired file"
    );
    loaded.save().unwrap();

    // Nothing is left to repair.
    let again = check::inspect_file().unwrap();
    assert!(!again.repairable(), "{:?}", again.problems);
    assert!(check::repair().is_err());
}
