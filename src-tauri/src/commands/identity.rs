//! The device key: where it is kept, and the password-protected device backup.

use super::{CmdResult, err};
use crate::node::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

/// Writes a password-protected backup of this device (its key and paired
/// devices, see [`ferry::node::backup`]) and returns where.
///
/// On the desktop `path` comes from a save dialog; on Android (no `path`) the
/// backup goes to the download folder (`Download/Ferry`), from where it can be
/// moved to the new phone.
///
/// # Errors
///
/// When the password is too short, the key cannot be read or the file cannot
/// be written.
#[tauri::command]
pub async fn create_backup(
    password: String,
    path: Option<String>,
    state: State<'_, AppState>,
) -> CmdResult<String> {
    let config = state.config.lock().await.clone();
    let target = match path.filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => {
            let name = format!(
                "ferry-backup-{}.{}",
                chrono::Local::now().format("%Y-%m-%d"),
                ferry::node::backup::EXTENSION
            );
            ferry::transfer::security::unique_destination(&state.download_dir().await.join(name))
        }
    };
    // PBKDF2 takes a moment on purpose: off the async threads.
    let text = tauri::async_runtime::spawn_blocking(move || {
        let key = ferry::node::identity::load_or_generate_default_key()?;
        ferry::node::backup::create(&key, &config, &password)
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))?;
    std::fs::write(&target, text).map_err(err)?;
    Ok(target.display().to_string())
}

/// The result of [`restore_backup`].
#[derive(Serialize)]
pub struct RestoredBackup {
    /// Node ID of this device from now on.
    node_id: String,
    /// The key changed: Ferry must restart to use it.
    key_changed: bool,
    /// Paired devices added.
    peers_added: usize,
}

/// Restores a backup made by [`create_backup`] (on this or another device):
/// its key becomes this device's key (the previous one is kept as a `.bak`
/// file) and its paired devices are added.
///
/// # Errors
///
/// When the file cannot be read, is not a backup, the password is wrong, or
/// the key or the configuration cannot be written.
#[tauri::command]
pub async fn restore_backup(
    path: String,
    password: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<RestoredBackup> {
    let local = crate::platform::local_path(&app, &path).await?;
    let text = std::fs::read_to_string(&local);
    // A copy of a content:// file (Android) is not kept.
    if path.starts_with("content://") {
        let _ = std::fs::remove_file(&local);
    }
    let text = text.map_err(err)?;
    let contents =
        tauri::async_runtime::spawn_blocking(move || ferry::node::backup::open(&text, &password))
            .await
            .map_err(err)?
            .map_err(|e| format!("{e:#}"))?;
    let mut config = state.config.lock().await;
    let mut next = config.clone();
    let restored = tauri::async_runtime::spawn_blocking(move || {
        let restored = ferry::node::backup::restore(&contents, &mut next)?;
        next.save()?;
        anyhow::Ok((restored, next))
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))?;
    let (restored, next) = restored;
    *config = next;
    drop(config);
    let _ = app.emit("peers-changed", ());
    Ok(RestoredBackup {
        node_id: restored.node_id,
        key_changed: restored.key_changed,
        peers_added: restored.peers_added,
    })
}

/// Where the device key is kept, and whether the system's secret store can
/// be chosen here.
#[derive(Serialize)]
pub struct KeyStorageInfo {
    /// The key is in the system's secret store.
    system: bool,
    /// This platform has a secret store Ferry can use (not Android).
    supported: bool,
}

/// Where the device key is kept.
///
/// # Errors
///
/// Never fails; Tauri requires async commands that borrow the state to return a `Result`.
#[tauri::command]
pub async fn get_key_storage(state: State<'_, AppState>) -> CmdResult<KeyStorageInfo> {
    use ferry::node::keystore::KeyStorage;
    Ok(KeyStorageInfo {
        system: state.config.lock().await.key_storage == KeyStorage::System,
        supported: KeyStorage::system_supported(),
    })
}

/// Moves the device key into the system's secret store (`system`) or back
/// into its file; the Node ID stays the same and the running node is not
/// affected.
///
/// # Errors
///
/// When the secret store cannot be used; the key then stays where it was.
#[tauri::command]
pub async fn set_key_storage(system: bool, state: State<'_, AppState>) -> CmdResult<()> {
    use ferry::node::keystore::KeyStorage;
    let target = if system {
        KeyStorage::System
    } else {
        KeyStorage::File
    };
    let mut config = state.config.lock().await;
    // The move reads and writes config.toml itself, and talks to D-Bus or
    // the Credential Manager: off the async threads.
    tauri::async_runtime::spawn_blocking(move || ferry::node::identity::set_key_storage(target))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    // Keep the in-memory copy in step, or the next save would undo the switch.
    config.key_storage = target;
    Ok(())
}
