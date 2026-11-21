//! Checking `config.toml` for damage and for settings worth a look, and
//! repairing it.
//!
//! The check reads the file statement by statement, so one broken line does
//! not lose the rest: every statement that can be read is kept, every one
//! that cannot is reported with its line. The values are then checked one by
//! one (each setting, each trusted device), and what is left forms the
//! repaired configuration.
//!
//! A repair never makes Ferry less safe than the file meant it to be: a
//! [`Config::paired_only`] that cannot be read becomes "on", an Auto-Accept
//! that cannot be read becomes "off", and nothing is guessed in favour of a
//! device. Settings that may have been changed by someone else (programs
//! from Auto-Accept devices, the download folder in a startup folder) are
//! reported, never changed.

use super::peers::fallback_alias;
use super::{Config, PeerInfo};
use crate::node::NodeTicket;
use crate::node::keystore::KeyStorage;
use anyhow::{Context, Result};
use iroh::EndpointId;
use jiff::Timestamp;
use rust_i18n::t;
use serde::Serialize;
use std::path::Path;
use toml::{Table, Value};

/// Largest file that is checked statement by statement: the file is read
/// again for every statement, and a configuration has a few kilobytes.
const MAX_SIZE: usize = 1 << 20;

/// Extension of the copy of the file kept before a repair.
pub const BEFORE_REPAIR: &str = "toml.before-repair";

/// The settings of `config.toml` (the fields of [`Config`]); others are
/// reported as unknown. Kept equal to the fields by a test.
const KNOWN_KEYS: [&str; 23] = [
    "peers",
    "default_download_dir",
    "device_name",
    "language",
    "network_mode",
    "relay_urls",
    "disable_history",
    "key_storage",
    "speed_limit",
    "hidden",
    "strip_metadata",
    "announce_platform",
    "random_port",
    "pending_unpair",
    "paired_only",
    "pairing_until",
    "auto_accept_texts",
    "auto_accept_executables",
    "welcome_dismissed",
    "quit_on_close",
    "keep_window_loaded",
    "minimal_background",
    "localsend",
];

/// How serious a [`Problem`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Damage: something in the file cannot be used.
    Error,
    /// A setting worth checking: it may not be what the user chose.
    Warning,
    /// For information (e.g. which devices send files without asking).
    Info,
}

/// One finding of the check.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    /// How serious it is.
    pub severity: Severity,
    /// Line of the file it is about, from 1.
    pub line: Option<usize>,
    /// What was found and what a repair does about it, in the user's language.
    pub message: String,
    /// Whether a repair changes it (warnings about choices the user may have
    /// made are only reported).
    pub repaired: bool,
}

/// Result of checking the file.
#[derive(Debug, Default)]
pub struct Report {
    /// What was found, in the order of the file.
    pub problems: Vec<Problem>,
    /// The configuration a repair saves: everything that could be read.
    /// `None` when not even that forms a configuration.
    pub fixed: Option<Config>,
}

impl Report {
    /// Whether a repair would change something.
    pub fn repairable(&self) -> bool {
        self.fixed.is_some() && self.problems.iter().any(|p| p.repaired)
    }

    /// Adds a finding.
    fn push(&mut self, severity: Severity, line: Option<usize>, message: impl Into<String>) {
        self.problems.push(Problem {
            severity,
            line,
            message: message.into(),
            repaired: severity == Severity::Error,
        });
    }
}

/// Checks the text of a `config.toml`; `key_file` tells whether the device
/// key is kept in `secret.key` (which [`Config::key_storage`] that cannot be
/// read is set to).
pub fn inspect(text: &str, key_file: bool) -> Report {
    let mut report = Report::default();
    if text.len() > MAX_SIZE {
        report.problems.push(Problem {
            severity: Severity::Error,
            line: None,
            message: t!("check.too_large", size = text.len()).into_owned(),
            repaired: false,
        });
        return report;
    }
    let (table, lost) = salvage(text, &mut report);
    let fixed = settings(table, &lost, key_file, &mut report);
    if let Some(config) = &fixed {
        notable(config, &mut report);
    }
    report.fixed = fixed;
    report
}

/// Checks the `config.toml` in use, including who may read it.
///
/// # Errors
///
/// When the configuration folder cannot be found or the file cannot be read.
pub fn inspect_file() -> Result<Report> {
    let path = Config::file_path()?;
    if !path.exists() {
        return Ok(Report::default());
    }
    let bytes = std::fs::read(&path).with_context(|| t!("config.read"))?;
    let text = String::from_utf8_lossy(&bytes);
    let key_file = crate::node::identity::default_key_path().is_ok_and(|p| p.exists());
    let mut report = inspect(&text, key_file);
    if let std::borrow::Cow::Owned(_) = text {
        report.problems.insert(
            0,
            Problem {
                severity: Severity::Error,
                line: None,
                message: t!("check.not_text").into_owned(),
                repaired: true,
            },
        );
    }
    permissions(&path, &mut report);
    Ok(report)
}

/// Repairs the `config.toml` in use: keeps a copy of it next to it
/// ([`BEFORE_REPAIR`]) and saves what [`inspect_file`] could read, also when
/// saving was blocked because the file could not be read at start.
///
/// # Errors
///
/// When there is nothing to repair, the file cannot be repaired, or saving fails.
pub fn repair() -> Result<Config> {
    let report = inspect_file()?;
    if !report.repairable() {
        anyhow::bail!(t!("check.nothing_to_repair"));
    }
    let config = report
        .fixed
        .with_context(|| t!("check.nothing_to_repair"))?;
    let path = Config::file_path()?;
    let copy = path.with_extension(BEFORE_REPAIR);
    std::fs::copy(&path, &copy).with_context(|| t!("config.write"))?;
    config.save_over_unreadable()?;
    tracing::info!(
        "Repaired {}; the previous file is in {}",
        path.display(),
        copy.display()
    );
    Ok(config)
}

/// A statement of the file: a section header or a `key = value`, with the
/// lines that continue it (a value over several lines, comments).
struct Statement {
    /// Its first line, from 1.
    line: usize,
    /// Its text with the line ends.
    text: String,
}

impl Statement {
    /// Whether it is a section header (`[peers.<id>]`).
    fn is_header(&self) -> bool {
        self.text.trim_start().starts_with('[')
    }

    /// The key it sets, as written (`paired_only`).
    fn key(&self) -> Option<String> {
        let (key, _) = self.text.trim_start().split_once('=')?;
        Some(key.trim().trim_matches(['"', '\'']).to_owned())
    }
}

/// Whether `line` starts a new statement: a section header, or a key
/// followed by `=` (a line of a value over several lines does not look so).
fn starts_statement(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with('[') {
        return true;
    }
    line.split_once('=').is_some_and(|(key, _)| {
        let key = key.trim();
        !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-.\"' ".contains(c))
    })
}

/// Splits the file into statements.
fn statements(text: &str) -> Vec<Statement> {
    let mut list: Vec<Statement> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        match list.last_mut() {
            Some(last) if !starts_statement(line) => {
                last.text.push_str(line);
                last.text.push('\n');
            }
            _ => list.push(Statement {
                line: i + 1,
                text: format!("{line}\n"),
            }),
        }
    }
    list
}

/// Reads every statement that can be read; the others are reported. Returns
/// what was read and the keys of top-level settings that were lost.
fn salvage(text: &str, report: &mut Report) -> (Table, Vec<String>) {
    let mut kept = String::new();
    let mut lost = Vec::new();
    let mut in_section = false;
    let mut skipping = false;
    for statement in statements(text) {
        let header = statement.is_header();
        if header {
            in_section = true;
            skipping = false;
        } else if skipping {
            continue;
        }
        let candidate = format!("{kept}{}", statement.text);
        match toml::from_str::<Table>(&candidate) {
            Ok(_) => kept = candidate,
            Err(e) if header => {
                skipping = true;
                report.push(
                    Severity::Error,
                    Some(statement.line),
                    t!(
                        "check.section",
                        line = statement.line,
                        section = statement.text.trim(),
                        error = e.message().trim()
                    ),
                );
            }
            Err(e) => {
                if !in_section && let Some(key) = statement.key() {
                    lost.push(key);
                }
                report.push(
                    Severity::Error,
                    Some(statement.line),
                    t!(
                        "check.line",
                        line = statement.line,
                        error = e.message().trim()
                    ),
                );
            }
        }
    }
    (toml::from_str(&kept).unwrap_or_default(), lost)
}

/// Checks every setting and every trusted device of `table`; returns the
/// configuration of what is valid.
fn settings(
    mut table: Table,
    lost: &[String],
    key_file: bool,
    report: &mut Report,
) -> Option<Config> {
    let peers = table.remove("peers");
    let mut lost = lost.to_vec();
    let mut fixed = Table::new();
    for (key, value) in table {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            report.push(Severity::Error, None, t!("check.unknown_key", key = &key));
            continue;
        }
        // Checked alone, with the list of devices every configuration has.
        let alone = Table::from_iter([
            (key.clone(), value.clone()),
            ("peers".to_owned(), Value::Table(Table::new())),
        ]);
        match Value::Table(alone).try_into::<Config>() {
            Ok(_) => {
                fixed.insert(key, value);
            }
            Err(e) => {
                report.push(
                    Severity::Error,
                    None,
                    t!("check.bad_value", key = &key, error = e.message().trim()),
                );
                lost.push(key);
            }
        }
    }
    // Lost settings get their defaults, except where the default is less safe
    // or would lose the device key.
    if lost.iter().any(|k| k == "paired_only") {
        fixed.insert("paired_only".into(), Value::Boolean(true));
        report.push(Severity::Error, None, t!("check.paired_only"));
    }
    if lost.iter().any(|k| k == "key_storage") {
        let storage = if key_file || !KeyStorage::system_supported() {
            "file"
        } else {
            "system"
        };
        fixed.insert("key_storage".into(), Value::String(storage.into()));
        report.push(
            Severity::Error,
            None,
            t!("check.key_storage", value = storage),
        );
    }

    let mut devices = Table::new();
    match peers {
        None => {}
        Some(Value::Table(peers)) => {
            for (id, value) in peers {
                if let Some(peer) = peer(&id, value, report) {
                    devices.insert(id, peer);
                }
            }
        }
        Some(_) => report.push(Severity::Error, None, t!("check.peers_not_table")),
    }
    fixed.insert("peers".into(), Value::Table(devices));

    match Value::Table(fixed).try_into::<Config>() {
        Ok(config) => Some(config),
        Err(e) => {
            report.push(
                Severity::Error,
                None,
                t!("check.unrepairable", error = e.message().trim()),
            );
            None
        }
    }
}

/// Checks the trusted device `id`; what cannot be read is reset (Auto-Accept
/// off) or removed. `None` when it is left out.
fn peer(id: &str, value: Value, report: &mut Report) -> Option<Value> {
    if id.parse::<EndpointId>().is_err() {
        report.push(Severity::Error, None, t!("check.peer_bad_id", id = id));
        return None;
    }
    let short = &id[..8];
    let Value::Table(mut fields) = value else {
        report.push(
            Severity::Error,
            None,
            t!("check.peer_unreadable", name = short),
        );
        return None;
    };
    let alias = match fields.get("alias") {
        Some(Value::String(alias)) if !alias.trim().is_empty() => alias.clone(),
        _ => {
            let alias = fallback_alias(id);
            report.push(
                Severity::Error,
                None,
                t!("check.peer_alias", id = short, alias = &alias),
            );
            fields.insert("alias".into(), Value::String(alias.clone()));
            alias
        }
    };
    if !matches!(fields.get("auto_accept"), Some(Value::Boolean(_))) {
        fields.insert("auto_accept".into(), Value::Boolean(false));
        report.push(
            Severity::Error,
            None,
            t!("check.peer_auto_accept", name = &alias),
        );
    }
    if fields
        .get("added_at")
        .is_none_or(|v| v.clone().try_into::<Timestamp>().is_err())
    {
        fields.insert(
            "added_at".into(),
            Value::String(Timestamp::now().to_string()),
        );
        report.push(
            Severity::Error,
            None,
            t!("check.peer_added_at", name = &alias),
        );
    }
    let valid = |field: &str, value: &Value| match field {
        "verified" => value.is_bool(),
        "ticket" => value
            .as_str()
            .is_some_and(|s| NodeTicket::parse_target(s).is_ok()),
        "discovery_key" => value
            .as_str()
            .is_some_and(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())),
        _ => true,
    };
    for field in ["verified", "ticket", "discovery_key"] {
        if fields.get(field).is_some_and(|v| !valid(field, v)) {
            fields.remove(field);
            report.push(
                Severity::Error,
                None,
                t!("check.peer_field", name = &alias, field = field),
            );
        }
    }
    let peer = Value::Table(fields);
    if let Err(e) = peer.clone().try_into::<PeerInfo>() {
        report.push(
            Severity::Error,
            None,
            t!("check.peer_unreadable", name = &alias).into_owned()
                + &format!(" ({})", e.message().trim()),
        );
        return None;
    }
    Some(peer)
}

/// Settings worth a look although they are valid: someone else may have
/// changed them. Reported, never changed by a repair.
fn notable(config: &Config, report: &mut Report) {
    let trusted: Vec<&str> = config
        .peers
        .values()
        .filter(|p| p.auto_accept)
        .map(|p| p.alias.as_str())
        .collect();
    if config.auto_accept_executables && !trusted.is_empty() {
        report.push(Severity::Warning, None, t!("check.executables"));
    }
    if let Some(dir) = &config.default_download_dir
        && runs_programs(dir)
    {
        report.push(
            Severity::Warning,
            None,
            t!("check.download_dir", path = dir.display()),
        );
    }
    if !trusted.is_empty() {
        report.push(
            Severity::Info,
            None,
            t!("check.auto_accept", names = trusted.join(", ")),
        );
    }
}

/// Whether files saved in `dir` may be run without the user opening them: a
/// folder of programs started at login, or a folder of the system.
fn runs_programs(dir: &Path) -> bool {
    let text = dir.to_string_lossy().replace('\\', "/").to_lowercase();
    let startup = [
        "/autostart",
        "/startup",
        "/launchagents",
        "/launchdaemons",
        "/.bashrc",
        "/.profile",
    ];
    let system = [
        "/etc",
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/boot",
        "c:/windows",
        "c:/program files",
    ];
    startup.iter().any(|s| text.contains(s))
        || system
            .iter()
            .any(|s| text == *s || text.starts_with(&format!("{s}/")))
}

/// Reports a file other users may read: it holds the secrets shared with
/// paired devices. A repair saves it readable by the owner only.
fn permissions(path: &Path, report: &mut Report) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                report.problems.push(Problem {
                    severity: Severity::Warning,
                    line: None,
                    message: t!("check.permissions", mode = format!("{mode:o}")).into_owned(),
                    repaired: true,
                });
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (path, report);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Node ID of a new device.
    fn id() -> String {
        iroh::SecretKey::generate().public().to_string()
    }

    fn errors(report: &Report) -> usize {
        report
            .problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
            .count()
    }

    #[test]
    fn a_valid_file_has_nothing_to_repair() {
        let id = id();
        let text = format!(
            "paired_only = true\nrelay_urls = [\n    \"https://relay.example.org/\",\n]\n\n[peers.{id}]\nalias = \"Laptop\"\nauto_accept = false\nadded_at = \"2026-11-01T10:00:00Z\"\n"
        );
        let report = inspect(&text, true);
        assert_eq!(errors(&report), 0, "{:?}", report.problems);
        assert!(!report.repairable());
        let config = report.fixed.unwrap();
        assert!(config.paired_only);
        assert_eq!(
            config.relay_urls.len(),
            1,
            "a value over several lines is kept"
        );
        assert_eq!(config.peers[&id].alias, "Laptop");
    }

    /// A typo loses only its line; `paired_only` comes back "on", the safer choice.
    #[test]
    fn a_broken_line_is_left_out_and_paired_only_stays_safe() {
        let id = id();
        let text = format!(
            "device_name = \"Desk\"\npaired_only = tru\n\n[peers.{id}]\nalias = \"Laptop\"\nauto_accept = false\nadded_at = \"2026-11-01T10:00:00Z\"\n"
        );
        let report = inspect(&text, true);
        assert!(report.repairable());
        assert_eq!(report.problems[0].line, Some(2));
        let config = report.fixed.unwrap();
        assert!(config.paired_only);
        assert_eq!(config.device_name.as_deref(), Some("Desk"));
        assert_eq!(
            config.peers.len(),
            1,
            "the device after the broken line is kept"
        );
    }

    /// Auto-Accept that cannot be read is turned off, never on; a device
    /// with a bad Node ID or a broken header is left out.
    #[test]
    fn broken_devices_are_fixed_towards_safety_or_left_out() {
        let (good, fixed) = (id(), id());
        let text = format!(
            "[peers.{good}]\nalias = \"Laptop\"\nauto_accept = true\nadded_at = \"2026-11-01T10:00:00Z\"\n\
             [peers.{fixed}]\nalias = \"\"\nauto_accept = \"yes\"\nadded_at = \"soon\"\ndiscovery_key = \"zz\"\n\
             [peers.not-an-id]\nalias = \"X\"\nauto_accept = true\nadded_at = \"2026-11-01T10:00:00Z\"\n\
             [peers.{ID}\nalias = \"Broken header\"\nauto_accept = true\n"
        );
        let report = inspect(&text, true);
        let config = report.fixed.clone().unwrap();
        assert_eq!(config.peers.len(), 2, "{:?}", report.problems);
        assert!(config.peers[&good].auto_accept);
        let repaired = &config.peers[&fixed];
        assert!(!repaired.auto_accept);
        assert!(!repaired.alias.is_empty());
        assert!(repaired.discovery_key.is_none());
        assert!(errors(&report) >= 6, "{:?}", report.problems);
    }

    /// A lost `key_storage` points where the device key is found, so the
    /// device keeps its Node ID.
    #[test]
    fn a_lost_key_storage_points_where_the_key_is() {
        let text = "key_storage = \"systm\"\n";
        let in_file = inspect(text, true).fixed.unwrap();
        assert_eq!(in_file.key_storage, KeyStorage::File);
        if KeyStorage::system_supported() {
            let in_system = inspect(text, false).fixed.unwrap();
            assert_eq!(in_system.key_storage, KeyStorage::System);
        }
    }

    /// Unknown settings and text that is not a configuration at all.
    #[test]
    fn garbage_and_unknown_settings_are_reported() {
        let report = inspect("\u{0}\u{1}garbage\npaired_onyl = true\n", true);
        assert_eq!(report.problems[0].line, Some(1));
        assert!(report.repairable());
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.message.contains("paired_onyl"))
        );
        assert!(report.fixed.unwrap().peers.is_empty());
    }

    /// Settings someone else may have changed are reported, not changed.
    #[test]
    fn notable_settings_are_reported_but_kept() {
        let id = id();
        let text = format!(
            "auto_accept_executables = true\ndefault_download_dir = \"/home/u/.config/autostart\"\n\n[peers.{id}]\nalias = \"Laptop\"\nauto_accept = true\nadded_at = \"2026-11-01T10:00:00Z\"\n"
        );
        let report = inspect(&text, true);
        assert_eq!(errors(&report), 0);
        assert!(!report.repairable(), "nothing for a repair to do");
        let warnings = report
            .problems
            .iter()
            .filter(|p| p.severity == Severity::Warning)
            .count();
        assert_eq!(warnings, 2, "{:?}", report.problems);
        assert!(report.fixed.unwrap().auto_accept_executables);
        assert!(!runs_programs(Path::new("/home/u/Downloads")));
        assert!(runs_programs(Path::new(
            "C:\\Users\\u\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup"
        )));
    }

    /// [`KNOWN_KEYS`] are exactly the settings a configuration can have.
    #[test]
    fn the_known_keys_are_the_fields_of_the_configuration() {
        let id = id();
        let mut c = Config::default();
        c.add_paired(id.parse().unwrap(), "Laptop", None, false, None)
            .unwrap();
        c.default_download_dir = Some("/tmp".into());
        c.device_name = Some("Desk".into());
        c.language = Some("pl".into());
        c.network_mode = crate::node::NetworkMode::Relay;
        c.relay_urls = vec!["https://relay.example.org/".into()];
        c.disable_history = true;
        c.key_storage = KeyStorage::System;
        c.speed_limit = Some(1);
        c.hidden = true;
        c.strip_metadata = true;
        c.announce_platform = true;
        c.random_port = true;
        c.pending_unpair.insert(id, Timestamp::now());
        c.paired_only = true;
        c.pairing_until = Some(Timestamp::now());
        c.auto_accept_texts = true;
        c.auto_accept_executables = true;
        c.welcome_dismissed = true;
        c.quit_on_close = true;
        c.keep_window_loaded = true;
        c.minimal_background = true;
        c.localsend = true;
        let table: Table = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        let mut keys: Vec<_> = table.keys().map(String::as_str).collect();
        let mut known = KNOWN_KEYS.to_vec();
        keys.sort_unstable();
        known.sort_unstable();
        assert_eq!(keys, known);
    }
}
