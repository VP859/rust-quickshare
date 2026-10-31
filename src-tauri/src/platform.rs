//! Platform differences: app directories, received media in the Android
//! gallery and files or texts from the Android "Share" menu.
//!
//! On Android, `ShareIntake.kt` copies the `content://` streams of
//! `ACTION_SEND`/`ACTION_SEND_MULTIPLE` intents into `cacheDir/share-in/` and
//! appends their paths to `pending.txt`; a shared text goes to
//! `pending-text.txt`. The frontend fetches them with `take_shared_files` and
//! `take_shared_text`.
//!
//! Files are received into `Download/Ferry`; completed photos and videos are
//! then moved to `Pictures/Ferry`, which galleries show as an album.
//!
//! Android also reports what limits Ferry in the background
//! ([`background_status`], from `PowerHelper.kt`) and keeps the phone awake
//! while transfers run ([`set_transfers_active`], `FerryService.kt`).

use ferry::transfer::service::TransferEvent;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// What limits Ferry in the background on Android (see `PowerHelper.kt`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackgroundStatus {
    /// Battery optimization applies: Doze may pause the network for minutes.
    #[serde(default)]
    pub battery_optimized: bool,
    /// Battery saver is on.
    #[serde(default)]
    pub power_save: bool,
    /// The user restricted the app in the background; no foreground service.
    #[serde(default)]
    pub background_restricted: bool,
    /// Data saver blocks background traffic on metered networks.
    #[serde(default)]
    pub data_saver: bool,
}

/// Android: public folders, gallery integration and the "Share" menu.
#[cfg(target_os = "android")]
mod imp {
    use ferry::transfer::service::{Direction, TransferEvent, TransferStatus};
    use rust_i18n::t;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};
    use tauri::{AppHandle, Manager};
    use tracing::warn;

    use super::BackgroundStatus;
    use crate::node::AppState;
    use jni::objects::{JClass, JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};

    /// Shared files older than a day are deleted at startup.
    const SHARE_RETENTION: Duration = Duration::from_secs(24 * 3600);
    /// Folder name inside the public `Download` and `Pictures` folders.
    const APP_FOLDER: &str = "Ferry";
    /// Extensions of photos and videos moved to the gallery folder.
    const MEDIA_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "bmp", "dng", "mp4", "m4v",
        "mov", "3gp", "webm", "mkv",
    ];

    /// Folder for files and texts received from the "Share" menu.
    fn share_dir(app: &AppHandle) -> Option<PathBuf> {
        app.path().app_cache_dir().ok().map(|d| d.join("share-in"))
    }

    /// `<storage>/<name>/Ferry` (e.g. `Download/Ferry`), created when missing;
    /// `None` when the public storage is not writable.
    fn public_dir(app: &AppHandle, name: &str) -> Option<PathBuf> {
        app.path()
            .home_dir()
            .ok()
            .map(|h| h.join(name).join(APP_FOLDER))
            .filter(|d| std::fs::create_dir_all(d).is_ok())
    }

    /// Sets the configuration directory, picks the download folder (public
    /// `Download/Ferry`, falling back to the app's private download folder) and
    /// the gallery folder `Pictures/Ferry`, and removes stale shared files.
    pub fn init(app: &AppHandle) {
        if let Ok(data) = app.path().app_data_dir() {
            ferry::config::paths::set_config_dir(data.join(ferry::config::paths::APP_DIR));
        }

        let state = app.state::<AppState>();
        let dir = public_dir(app, "Download")
            .or_else(|| app.path().download_dir().ok())
            .unwrap_or_else(std::env::temp_dir);
        let _ = state.platform_download_dir.set(dir);
        if let Some(media) = public_dir(app, "Pictures") {
            let _ = state.platform_media_dir.set(media);
        }

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

    /// Whether the file is a photo or video, judged by its extension.
    fn is_media(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
    }

    /// Moves `src` into `dir`, copying when a rename is not possible.
    fn move_into(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
        let name = src.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
        let dest = ferry::transfer::security::unique_destination(&dir.join(name));
        if std::fs::rename(src, &dest).is_err() {
            std::fs::copy(src, &dest)?;
            std::fs::remove_file(src)?;
        }
        Ok(dest)
    }

    /// After a completed download: moves photos and videos to the gallery
    /// folder, updates the path in `event` and registers the file in the
    /// Android media store.
    pub fn finalize_download(app: &AppHandle, event: &mut TransferEvent) {
        if event.direction != Direction::Download {
            return;
        }
        let TransferStatus::Completed { path: Some(path) } = &mut event.status else {
            return;
        };
        let mut file = PathBuf::from(&*path);
        if is_media(&file)
            && let Some(media_dir) = app.state::<AppState>().platform_media_dir.get()
        {
            match move_into(&file, media_dir) {
                Ok(moved) => {
                    file = moved;
                    *path = file.display().to_string();
                }
                Err(e) => warn!("Could not move {} to the gallery: {e}", file.display()),
            }
        }
        if let Err(e) = media_scan(&file) {
            warn!("Media scan of {} failed: {e}", file.display());
        }
    }

    /// Calls `MediaScannerConnection.scanFile`, so the file shows up in the
    /// gallery and file managers right away (Android 10 and older do not index
    /// files written through paths by themselves).
    ///
    /// The other place that needs `unsafe` (JNI); every block is justified.
    #[allow(unsafe_code)]
    fn media_scan(path: &Path) -> jni::errors::Result<()> {
        use jni::objects::{JObject, JValue};

        let ctx = ndk_context::android_context();
        // SAFETY: tao initializes the ndk context with a valid JavaVM pointer
        // before the app starts and never releases it.
        let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        // SAFETY: the context is the application context, kept alive by tao in
        // a global reference for the whole process lifetime.
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        let mut scan = || -> jni::errors::Result<()> {
            let file = env.new_string(path.to_string_lossy())?;
            let paths = env.new_object_array(1, "java/lang/String", &file)?;
            env.call_static_method(
                "android/media/MediaScannerConnection",
                "scanFile",
                "(Landroid/content/Context;[Ljava/lang/String;[Ljava/lang/String;\
                 Landroid/media/MediaScannerConnection$OnScanCompletedListener;)V",
                &[
                    JValue::Object(&context),
                    JValue::Object(&paths),
                    JValue::Object(&JObject::null()),
                    JValue::Object(&JObject::null()),
                ],
            )?;
            Ok(())
        };
        let result = scan();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result
    }

    /// Runs `call` with a JNI environment, the application context and the app
    /// class `name` (e.g. `io.github.vp859.ferry.PowerHelper`).
    ///
    /// App classes are loaded through the context's class loader: `FindClass`
    /// on a thread started by Rust only sees the classes of the system.
    #[allow(unsafe_code)]
    fn with_app_class<R>(
        name: &str,
        call: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>, &JClass<'_>) -> jni::errors::Result<R>,
    ) -> jni::errors::Result<R> {
        let ctx = ndk_context::android_context();
        // SAFETY: tao initializes the ndk context with a valid JavaVM pointer
        // before the app starts and never releases it.
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        // SAFETY: the context is the application context, kept alive by tao in
        // a global reference for the whole process lifetime.
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        let run = || {
            let loader = env
                .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
                .l()?;
            let class_name = env.new_string(name)?;
            let class = env
                .call_method(
                    &loader,
                    "loadClass",
                    "(Ljava/lang/String;)Ljava/lang/Class;",
                    &[JValue::Object(&class_name)],
                )?
                .l()?;
            call(&mut env, &context, &JClass::from(class))
        };
        let result = run();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result
    }

    /// Background limits reported by `PowerHelper.status`.
    pub fn background_status() -> Option<BackgroundStatus> {
        let json = with_app_class(
            "io.github.vp859.ferry.PowerHelper",
            |env, context, class| {
                let value = env
                    .call_static_method(
                        class,
                        "status",
                        "(Landroid/content/Context;)Ljava/lang/String;",
                        &[JValue::Object(context)],
                    )?
                    .l()?;
                env.get_string(&JString::from(value)).map(String::from)
            },
        )
        .inspect_err(|e| warn!("Could not read the background limits: {e}"))
        .ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Opens the system app info screen (battery usage, permissions).
    pub fn open_app_settings() {
        let result = with_app_class(
            "io.github.vp859.ferry.PowerHelper",
            |env, context, class| {
                env.call_static_method(
                    class,
                    "openAppSettings",
                    "(Landroid/content/Context;)V",
                    &[JValue::Object(context)],
                )
                .map(|_| ())
            },
        );
        if let Err(e) = result {
            warn!("Could not open the app settings: {e}");
        }
    }

    /// Holds or releases the wake and Wi-Fi locks of `FerryService`.
    pub fn set_transfers_active(active: bool) {
        let result = with_app_class(
            "io.github.vp859.ferry.FerryService",
            |env, context, class| {
                env.call_static_method(
                    class,
                    "setTransfersActive",
                    "(Landroid/content/Context;Z)V",
                    &[JValue::Object(context), JValue::Bool(u8::from(active))],
                )
                .map(|_| ())
            },
        );
        if let Err(e) = result {
            warn!("Could not update the transfer locks: {e}");
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
            .and_then(|n| ferry::transfer::security::sanitize_filename(&n).ok())
            .unwrap_or_else(|| "file".to_string());
        let dest = ferry::transfer::security::unique_destination(&dir.join(name));

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

    /// Takes (and forgets) a text received from the "Share" menu.
    pub fn take_shared_text(app: &AppHandle) -> Option<String> {
        let file = share_dir(app)?.join("pending-text.txt");
        let text = std::fs::read_to_string(&file).ok();
        let _ = std::fs::remove_file(&file);
        text.filter(|t| !t.trim().is_empty())
    }
}

/// Desktop: nothing to set up, paths are used as they are and there is no
/// "Share" menu.
#[cfg(not(target_os = "android"))]
mod imp {
    use ferry::transfer::service::TransferEvent;
    use std::path::PathBuf;
    use tauri::AppHandle;

    /// The default directories of the core are used.
    pub fn init(_app: &AppHandle) {}

    /// Received files stay where they were saved.
    pub fn finalize_download(_app: &AppHandle, _event: &mut TransferEvent) {}

    /// Desktop file pickers return regular paths.
    pub async fn local_path(_app: &AppHandle, input: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(input))
    }

    /// There is no "Share" menu on desktop.
    pub fn take_shared_files(_app: &AppHandle) -> Vec<String> {
        Vec::new()
    }

    /// There is no "Share" menu on desktop.
    pub fn take_shared_text(_app: &AppHandle) -> Option<String> {
        None
    }

    /// Desktop systems do not limit the app in the background.
    pub fn background_status() -> Option<super::BackgroundStatus> {
        None
    }

    /// Only Android has the app info screen with battery settings.
    pub fn open_app_settings() {}

    /// Desktop systems do not suspend a running transfer.
    pub fn set_transfers_active(_active: bool) {}
}

/// Sets platform-specific directories; called before the configuration is loaded.
pub fn init(app: &AppHandle) {
    imp::init(app);
}

/// Post-processes a transfer event before it reaches the frontend: on Android
/// a completed photo or video is moved to the gallery folder and indexed.
pub fn finalize_download(app: &AppHandle, event: &mut TransferEvent) {
    imp::finalize_download(app, event);
}

/// Paths of files shared to the app from the Android "Share" menu (empty on desktop).
pub fn take_shared_files(app: &AppHandle) -> Vec<String> {
    imp::take_shared_files(app)
}

/// A text shared to the app from the Android "Share" menu (none on desktop).
pub fn take_shared_text(app: &AppHandle) -> Option<String> {
    imp::take_shared_text(app)
}

/// What limits Ferry in the background (Android); `None` on desktop.
pub fn background_status() -> Option<BackgroundStatus> {
    imp::background_status()
}

/// Opens the system app info screen of Ferry (Android).
pub fn open_app_settings() {
    imp::open_app_settings();
}

/// Tells Android whether transfers are running, so the phone stays awake
/// while they do (no-op on desktop).
pub fn set_transfers_active(active: bool) {
    imp::set_transfers_active(active);
}

/// Turns a path or a `content://` URI from the frontend into a local file path.
pub async fn local_path(app: &AppHandle, input: &str) -> Result<std::path::PathBuf, String> {
    imp::local_path(app, input).await
}
