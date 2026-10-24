//! Platform differences: app directories and files from the Android "Share" menu.
//!
//! On Android, `ShareIntake.kt` copies the `content://` streams of
//! `ACTION_SEND`/`ACTION_SEND_MULTIPLE` intents into `cacheDir/share-in/` and
//! appends their paths to `pending.txt`; the frontend fetches them with
//! `take_shared_files`.

use tauri::AppHandle;

#[cfg(target_os = "android")]
mod imp {
    use rust_i18n::t;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};
    use tauri::{AppHandle, Manager};

    use crate::node::AppState;

    /// Shared files older than a day are deleted at startup.
    const SHARE_RETENTION: Duration = Duration::from_secs(24 * 3600);

    fn share_dir(app: &AppHandle) -> Option<PathBuf> {
        app.path().app_cache_dir().ok().map(|d| d.join("share-in"))
    }

    /// Sets the configuration directory, picks the download folder
    /// (public `Download/Ferry`, falling back to the app's private download folder)
    /// and removes stale shared files.
    pub fn init(app: &AppHandle) {
        if let Ok(data) = app.path().app_data_dir() {
            rqs_p2p::config::paths::set_config_dir(data.join("rqs"));
        }

        let public = app
            .path()
            .home_dir()
            .ok()
            .map(|h| h.join("Download").join("Ferry"))
            .filter(|d| std::fs::create_dir_all(d).is_ok());
        let dir = public
            .or_else(|| app.path().download_dir().ok())
            .unwrap_or_else(std::env::temp_dir);
        let _ = app.state::<AppState>().platform_download_dir.set(dir);

        if let Some(entries) = share_dir(app).and_then(|d| std::fs::read_dir(d).ok()) {
            for entry in entries.flatten() {
                let stale = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > SHARE_RETENTION);
                if stale {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Copies a `content://` URI (from the file picker) into the app cache,
    /// because the transfer engine needs a regular file (size, seeking on resume).
    pub async fn local_path(app: &AppHandle, input: &str) -> Result<PathBuf, String> {
        if !input.starts_with("content://") {
            return Ok(PathBuf::from(
                input.strip_prefix("file://").unwrap_or(input),
            ));
        }
        use std::str::FromStr;
        use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};

        let dir = share_dir(app).ok_or_else(|| t!("gui.no_cache_dir").into_owned())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name = app
            .path()
            .file_name(input)
            .and_then(|n| rqs_p2p::transfer::security::sanitize_filename(&n).ok())
            .unwrap_or_else(|| "file".to_string());
        let dest = rqs_p2p::transfer::security::unique_destination(&dir.join(name));

        let mut opts = OpenOptions::new();
        opts.read(true);
        let path = FilePath::from_str(input).map_err(|e| e.to_string())?;
        let mut src = app.fs().open(path, opts).map_err(|e| e.to_string())?;
        let target = dest.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut src, &mut out)?;
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        Ok(dest)
    }

    /// Takes (and forgets) the paths of files received from the "Share" menu.
    pub fn take_shared_files(app: &AppHandle) -> Vec<String> {
        let Some(list) = share_dir(app).map(|d| d.join("pending.txt")) else {
            return Vec::new();
        };
        let content = std::fs::read_to_string(&list).unwrap_or_default();
        let _ = std::fs::remove_file(&list);
        content
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && std::path::Path::new(l).is_file())
            .map(String::from)
            .collect()
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    use std::path::PathBuf;
    use tauri::AppHandle;

    pub fn init(_app: &AppHandle) {}

    pub async fn local_path(_app: &AppHandle, input: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(input))
    }

    pub fn take_shared_files(_app: &AppHandle) -> Vec<String> {
        Vec::new()
    }
}

/// Sets platform-specific directories; called before the configuration is loaded.
pub fn init(app: &AppHandle) {
    imp::init(app);
}

/// Paths of files shared to the app from the Android "Share" menu (empty on desktop).
pub fn take_shared_files(app: &AppHandle) -> Vec<String> {
    imp::take_shared_files(app)
}

/// Turns a path or a `content://` URI from the frontend into a local file path.
pub async fn local_path(app: &AppHandle, input: &str) -> Result<std::path::PathBuf, String> {
    imp::local_path(app, input).await
}
