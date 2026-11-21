//! A `config.toml` that cannot be read must never be replaced by the defaults
//! an app falls back to (`Config::load_or_defaults`). Its own test binary:
//! blocking saves applies to the whole process.

use ferry::config::Config;

#[test]
fn an_unreadable_file_is_kept_and_not_overwritten_until_a_backup_is_restored() {
    let dir = tempfile::tempdir().unwrap();
    ferry::config::paths::set_config_dir(dir.path().to_path_buf());
    let path = Config::file_path().unwrap();
    // A typo made by hand in a file with a trusted device.
    let broken = format!(
        "paired_only = tru\n\n[peers.{}]\nalias = \"Laptop\"\nauto_accept = false\nadded_at = \"2026-11-01T10:00:00Z\"\n",
        "b".repeat(64)
    );
    std::fs::write(&path, &broken).unwrap();

    let (config, error) = Config::load_or_defaults();
    assert!(error.is_some(), "the typo is reported");
    assert!(config.peers.is_empty(), "the app runs with the defaults");
    let copy = path.with_extension("toml.broken");
    assert_eq!(
        std::fs::read_to_string(&copy).unwrap(),
        broken,
        "a copy is kept"
    );

    // Changing a setting must not replace the user's file with the defaults.
    let mut changed = config.clone();
    changed.device_name = Some("Desk".into());
    let refused = changed.save().unwrap_err();
    assert!(
        format!("{refused:#}").contains("config.toml"),
        "{refused:#}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);

    // A restored backup completes the configuration: it is saved, and
    // saving works again afterwards.
    let mut restored = config;
    restored.device_name = Some("Restored".into());
    restored.save_over_unreadable().unwrap();
    assert_eq!(
        Config::load().unwrap().device_name.as_deref(),
        Some("Restored")
    );
    changed.save().unwrap();
    assert_eq!(Config::load().unwrap().device_name.as_deref(), Some("Desk"));
    assert_eq!(
        std::fs::read_to_string(&copy).unwrap(),
        broken,
        "the copy stays"
    );
}
