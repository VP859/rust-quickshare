//! `ferry identity`: shows, backs up and restores the Node ID key.
//!
//! The key is what paired devices recognize; a backup lets a reinstalled
//! system keep its pairings. It is stored as a file readable only by the user,
//! or – optionally, on the desktop – in the system's secret store
//! ([`ferry::node::keystore`]).

use anyhow::{Context, Result};
use rust_i18n::t;
use std::path::Path;

use crate::cli::args::IdentityCommands;
use ferry::config::Config;
use ferry::node::identity::set_key_storage;
use ferry::node::identity::{
    default_key_path, load_or_generate_default_key, parse_identity, replace_secret_key,
    write_identity_export,
};
use ferry::node::keystore::KeyStorage;

/// `ferry identity [show|export|import]`.
///
/// # Errors
///
/// Fails when the key cannot be read or written, the backup is invalid, or an
/// import would replace a different key without `--force`.
pub fn run_identity(command: Option<IdentityCommands>) -> Result<()> {
    match command.unwrap_or(IdentityCommands::Show) {
        IdentityCommands::Show => {
            let key = load_or_generate_default_key()?;
            let place = match Config::load().unwrap_or_default().key_storage {
                KeyStorage::File => default_key_path()?.display().to_string(),
                KeyStorage::System => t!("identity.in_system_store").into_owned(),
            };
            println!("{}", t!("identity.show", id = key.public(), path = place));
        }
        IdentityCommands::Storage { storage } => {
            if let Some(storage) = storage {
                let target: KeyStorage = storage.parse()?;
                set_key_storage(target)?;
                println!("{}", t!("identity.storage_moved", storage = target));
            }
            let current = Config::load().unwrap_or_default().key_storage;
            println!("{}", t!("identity.storage", storage = current));
        }
        IdentityCommands::Export { file } => {
            let key = load_or_generate_default_key()?;
            write_identity_export(&file, &key)?;
            println!("{}", t!("identity.exported", path = file.display()));
        }
        IdentityCommands::Import { file, force } => import(&file, force)?,
    }
    Ok(())
}

/// Replaces the stored key with the one in `file`.
fn import(file: &Path, force: bool) -> Result<()> {
    let text = std::fs::read_to_string(file)
        .with_context(|| t!("identity.read", path = file.display()))?;
    let key = parse_identity(&text)?;
    if Config::load().unwrap_or_default().key_storage == KeyStorage::System {
        anyhow::bail!(t!("identity.import_system"));
    }
    let path = default_key_path()?;
    if path.exists() {
        let current = load_or_generate_default_key()?;
        if current.public() == key.public() {
            println!("{}", t!("identity.import_same", id = key.public()));
            return Ok(());
        }
        if !force {
            anyhow::bail!(t!(
                "identity.import_needs_force",
                old = current.public(),
                new = key.public()
            ));
        }
    }
    let backup = replace_secret_key(&path, &key)?;
    println!("{}", t!("identity.imported", id = key.public()));
    if let Some(backup) = backup {
        println!("{}", t!("identity.backup", path = backup.display()));
    }
    println!("{}", t!("settings.restart_running"));
    Ok(())
}
