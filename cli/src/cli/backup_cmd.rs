//! `ferry backup create|restore`: the password-protected device backup
//! ([`ferry::node::backup`]) – the same file the GUI makes and restores, so a
//! backup from a phone can be restored on a server running the daemon, and
//! the other way round.

use anyhow::{Context, Result};
use rust_i18n::t;
use std::io::{IsTerminal, Write};
use std::path::Path;

use crate::cli::args::BackupCommands;
use ferry::config::Config;
use ferry::node::backup;

/// Runs a `ferry backup` subcommand.
///
/// # Errors
///
/// Fails when the password is too short or wrong, the file cannot be read or
/// written, or the key or configuration cannot be saved.
pub fn run(command: BackupCommands) -> Result<()> {
    match command {
        BackupCommands::Create { file } => create(&file),
        BackupCommands::Restore { file } => restore(&file),
    }
}

/// Writes a backup of this device to `file`, which must not exist yet.
fn create(file: &Path) -> Result<()> {
    let password = read_password(&t!("backup_cli.password_new"))?;
    anyhow::ensure!(
        password.chars().count() >= backup::MIN_PASSWORD_LEN,
        t!("backup.password_short", min = backup::MIN_PASSWORD_LEN)
    );
    if std::io::stdin().is_terminal() {
        let again = read_password(&t!("backup_cli.password_again"))?;
        anyhow::ensure!(password == again, t!("backup_cli.password_mismatch"));
    }
    let key = ferry::node::identity::load_or_generate_default_key()?;
    let config = Config::load()?;
    eprintln!("{}", t!("backup_cli.working"));
    let text = backup::create(&key, &config, &password)?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .with_context(|| t!("identity.write", path = file.display()))?;
    out.write_all(text.as_bytes())
        .with_context(|| t!("identity.write", path = file.display()))?;
    println!(
        "{}",
        t!(
            "backup_cli.created",
            path = file.display(),
            id = key.public(),
            count = config.peers.len()
        )
    );
    println!("{}", t!("backup_cli.keep_apart"));
    Ok(())
}

/// Restores the backup in `file`: its key and its paired devices.
fn restore(file: &Path) -> Result<()> {
    let text = std::fs::read_to_string(file)
        .with_context(|| t!("identity.read", path = file.display()))?;
    let password = read_password(&t!("backup_cli.password"))?;
    eprintln!("{}", t!("backup_cli.working"));
    let contents = backup::open(&text, &password)?;
    let mut config = Config::load()?;
    let restored = backup::restore(&contents, &mut config)?;
    config.save()?;
    println!(
        "{}",
        t!(
            "backup_cli.restored",
            id = &restored.node_id,
            count = restored.peers_added
        )
    );
    if let Some(previous) = &restored.previous_key {
        println!("{}", t!("identity.backup", path = previous.display()));
    }
    if restored.key_changed {
        println!("{}", t!("settings.restart_running"));
    }
    Ok(())
}

/// Reads a password: typed in the terminal without echo, or the first line of
/// standard input when it is not a terminal (for scripts:
/// `ferry backup restore FILE < password.txt`).
fn read_password(prompt: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let typed = read_hidden();
    crossterm::terminal::disable_raw_mode()?;
    eprintln!();
    typed
}

/// The characters typed until Enter, shown as nothing (raw mode is on).
fn read_hidden() -> Result<String> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    let mut typed = String::new();
    loop {
        let Event::Key(key) = read()? else {
            continue;
        };
        // Windows also reports key releases.
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Enter => return Ok(typed),
            KeyCode::Backspace => {
                typed.pop();
            }
            KeyCode::Esc => anyhow::bail!(t!("backup_cli.cancelled")),
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                anyhow::bail!(t!("backup_cli.cancelled"))
            }
            KeyCode::Char(c) => typed.push(c),
            _ => {}
        }
    }
}
