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

/// Runtime permissions on Android (see `FerryPermissions.kt`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionStatus {
    /// Local network access (Android 17+): `granted`, `denied` or `not_needed`.
    #[serde(default)]
    pub local_network: String,
    /// Notifications (Android 13+): `granted`, `denied` or `not_needed`.
    #[serde(default)]
    pub notifications: String,
    /// Saving to Downloads (Android 10 and older): `granted`, `denied` or `not_needed`.
    #[serde(default)]
    pub storage: String,
    /// Android still shows its dialog; otherwise only the app settings help.
    #[serde(default)]
    pub can_ask: bool,
    /// Android's dialog is open right now.
    #[serde(default)]
    pub asking: bool,
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
    use jni::objects::{JClass, JClassLoader, JObject, JObjectArray, JString, LoaderContext};
    use jni::signature::MethodSignature;
    use jni::strings::JNIStr;
    use jni::{Env, EnvUnowned, JValue, JValueOwned, JavaVM, jni_sig, jni_str};

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
        let _ = APP.set(app.clone());
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
                    // Shared files, and copies of folders picked to send.
                    let path = entry.path();
                    let _ = if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
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

    /// Runs `call` on a thread attached to the JVM, with the application
    /// context. A Java exception thrown by `call` is cleared and returned as
    /// an error by `attach_current_thread`.
    ///
    /// The only place that needs `unsafe` for JNI; every block is justified.
    #[allow(unsafe_code)]
    fn with_context<R>(
        call: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<R>,
    ) -> jni::errors::Result<R> {
        let ctx = ndk_context::android_context();
        // SAFETY: tao initializes the ndk context with a valid JavaVM pointer
        // before the app starts and never releases it.
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) };
        vm.attach_current_thread(|env| {
            // SAFETY: the context is the application context, held by tao in
            // a global reference for the whole process lifetime, so it is
            // valid on every thread; `JObject` never deletes the reference.
            let context = unsafe { JObject::from_raw(env, ctx.context().cast()) };
            call(env, &context)
        })
    }

    /// Calls `MediaScannerConnection.scanFile`, so the file shows up in the
    /// gallery and file managers right away (Android 10 and older do not index
    /// files written through paths by themselves).
    fn media_scan(path: &Path) -> jni::errors::Result<()> {
        with_context(|env, context| {
            let file = env.new_string(path.to_string_lossy())?;
            let paths = JObjectArray::<JString>::new(env, 1, &file)?;
            env.call_static_method(
                jni_str!("android/media/MediaScannerConnection"),
                jni_str!("scanFile"),
                jni_sig!(
                    "(Landroid/content/Context;[Ljava/lang/String;[Ljava/lang/String;\
                     Landroid/media/MediaScannerConnection$OnScanCompletedListener;)V"
                ),
                &[
                    JValue::Object(context),
                    JValue::Object(&paths),
                    JValue::Object(&JObject::null()),
                    JValue::Object(&JObject::null()),
                ],
            )?;
            Ok(())
        })
    }

    /// Runs `call` with a JNI environment, the application context and the app
    /// class `name` (e.g. `io.github.vp859.ferry.PowerHelper`).
    ///
    /// App classes are loaded through the context's class loader: `FindClass`
    /// on a thread started by Rust only sees the classes of the system.
    fn with_app_class<R>(
        name: &JNIStr,
        call: impl FnOnce(&mut Env<'_>, &JObject<'_>, &JClass<'_>) -> jni::errors::Result<R>,
    ) -> jni::errors::Result<R> {
        with_context(|env, context| {
            let loader = env
                .call_method(
                    context,
                    jni_str!("getClassLoader"),
                    jni_sig!("()Ljava/lang/ClassLoader;"),
                    &[],
                )?
                .l()?;
            let loader = env.cast_local::<JClassLoader>(loader)?;
            let class = LoaderContext::Loader(&loader).load_class(env, name, false)?;
            call(env, context, &class)
        })
    }

    /// The value after the context in a call of an app method.
    enum Arg<'a> {
        /// A `java.lang.String`.
        Text(&'a str),
        /// A `boolean`.
        Bool(bool),
    }

    /// Calls the static method `method` (signature `sig`) of the app class
    /// `class` with the application context and `args`; `read` turns the
    /// returned value into the result.
    fn call_app<R>(
        class: &JNIStr,
        method: &JNIStr,
        sig: MethodSignature<'_, '_>,
        args: &[Arg<'_>],
        read: impl FnOnce(&mut Env<'_>, JValueOwned<'_>) -> jni::errors::Result<R>,
    ) -> jni::errors::Result<R> {
        with_app_class(class, |env, context, class| {
            // Java strings first, so the values below can borrow them.
            let texts = args
                .iter()
                .map(|arg| match arg {
                    Arg::Text(text) => env.new_string(*text).map(Some),
                    Arg::Bool(_) => Ok(None),
                })
                .collect::<jni::errors::Result<Vec<_>>>()?;
            // Never used: every text has its Java string.
            let null = JObject::null();
            let mut values = vec![JValue::Object(context)];
            for (arg, text) in args.iter().zip(&texts) {
                values.push(match (arg, text) {
                    (Arg::Bool(value), _) => JValue::Bool(*value),
                    (Arg::Text(_), Some(text)) => JValue::Object(text),
                    (Arg::Text(_), None) => JValue::Object(&null),
                });
            }
            let value = env.call_static_method(class, method, &sig, &values)?;
            read(env, value)
        })
    }

    /// [`call_app`] of a method returning nothing.
    fn call_app_void(
        class: &JNIStr,
        method: &JNIStr,
        sig: MethodSignature<'_, '_>,
        args: &[Arg<'_>],
    ) -> jni::errors::Result<()> {
        call_app(class, method, sig, args, |_, _| Ok(()))
    }

    /// [`call_app`] of a method returning a `String`.
    fn call_app_string(
        class: &JNIStr,
        method: &JNIStr,
        sig: MethodSignature<'_, '_>,
        args: &[Arg<'_>],
    ) -> jni::errors::Result<String> {
        call_app(class, method, sig, args, |env, value| {
            env.cast_local::<JString>(value.l()?)?.try_to_string(env)
        })
    }

    /// [`call_app`] of a method returning a `boolean`.
    fn call_app_bool(
        class: &JNIStr,
        method: &JNIStr,
        sig: MethodSignature<'_, '_>,
        args: &[Arg<'_>],
    ) -> jni::errors::Result<bool> {
        call_app(class, method, sig, args, |_, value| value.z())
    }

    /// Background limits reported by `PowerHelper.status`.
    pub fn background_status() -> Option<BackgroundStatus> {
        let json = call_app_string(
            jni_str!("io.github.vp859.ferry.PowerHelper"),
            jni_str!("status"),
            jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
            &[],
        )
        .inspect_err(|e| warn!("Could not read the background limits: {e}"))
        .ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Opens a received file in another app (`FileOpener.open`): a
    /// `content://` URI of the app's FileProvider with `ACTION_VIEW`.
    pub fn open_file(path: &Path) -> Result<(), String> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let outcome = call_app_string(
            jni_str!("io.github.vp859.ferry.FileOpener"),
            jni_str!("open"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;"),
            &[Arg::Text(&path.to_string_lossy())],
        )
        .map_err(|e| {
            // The Kotlin side could not be called at all (not "file missing").
            warn!("Could not open {name}: {e}");
            t!("gui.open_failed", name = &name, error = e.to_string()).into_owned()
        })?;
        match outcome.as_str() {
            "" => Ok(()),
            "no_app" => Err(t!("gui.open_no_app", name = &name).into_owned()),
            _ => Err(t!("gui.open_unavailable", name = &name).into_owned()),
        }
    }

    /// The runtime permissions, from `FerryPermissions.status`.
    pub fn permission_status() -> Option<super::PermissionStatus> {
        let json = call_app_string(
            jni_str!("io.github.vp859.ferry.FerryPermissions"),
            jni_str!("status"),
            jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
            &[],
        )
        .inspect_err(|e| warn!("Could not read the permissions: {e}"))
        .ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Shows Android's dialog for the missing permissions, or only for the
    /// local network (`FerryPermissions.request`); the answer arrives as the
    /// `ferry-permissions` window event.
    pub fn request_permissions(only_local_network: bool) {
        let only = if only_local_network {
            "local_network"
        } else {
            ""
        };
        let result = call_app_void(
            jni_str!("io.github.vp859.ferry.FerryPermissions"),
            jni_str!("request"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[Arg::Text(only)],
        );
        if let Err(e) = result {
            warn!("Could not ask for the permissions: {e}");
        }
    }

    /// The app, for answers from the buttons of a notification (set in [`init`]).
    static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

    /// Shows the question `id` as a notification with "Accept" and "Decline"
    /// (`OfferActions.show`); `false` when it could not be shown.
    pub fn show_question(id: &str, title: &str, text: &str, accept: &str, decline: &str) -> bool {
        call_app_bool(
            jni_str!("io.github.vp859.ferry.OfferActions"),
            jni_str!("show"),
            jni_sig!(
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;\
                 Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z"
            ),
            &[
                Arg::Text(id),
                Arg::Text(title),
                Arg::Text(text),
                Arg::Text(accept),
                Arg::Text(decline),
            ],
        )
        .inspect_err(|e| warn!("Could not show the question: {e}"))
        .unwrap_or(false)
    }

    /// Removes the notification of the question `id` (`OfferActions.dismiss`).
    pub fn dismiss_question(id: &str) {
        if let Err(e) = call_app_void(
            jni_str!("io.github.vp859.ferry.OfferActions"),
            jni_str!("dismiss"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[Arg::Text(id)],
        ) {
            warn!("Could not remove the question: {e}");
        }
    }

    /// `OfferActions.answer`: a button of a question's notification was
    /// pressed; the answer goes where an answer in the app would.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_vp859_ferry_OfferActions_answer<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        id: JString<'caller>,
        accept: jni::sys::jboolean,
    ) {
        env.with_env(|env| -> jni::errors::Result<()> {
            let id = id.try_to_string(env)?;
            if let Some(app) = APP.get() {
                crate::node::answer_from_notification(app, id, accept);
            }
            Ok(())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
    }

    /// Starts Ferry again in a new process (`Restarter.restart`).
    pub fn restart_app() {
        let result = call_app_void(
            jni_str!("io.github.vp859.ferry.Restarter"),
            jni_str!("restart"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[],
        );
        if let Err(e) = result {
            warn!("Could not restart: {e}");
        }
    }

    /// Opens the system app info screen (battery usage, permissions).
    pub fn open_app_settings() {
        let result = call_app_void(
            jni_str!("io.github.vp859.ferry.PowerHelper"),
            jni_str!("openAppSettings"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[],
        );
        if let Err(e) = result {
            warn!("Could not open the app settings: {e}");
        }
    }

    /// Holds or releases the wake and Wi-Fi locks of `FerryService`.
    pub fn set_transfers_active(active: bool) {
        let result = call_app_void(
            jni_str!("io.github.vp859.ferry.FerryService"),
            jni_str!("setTransfersActive"),
            jni_sig!("(Landroid/content/Context;Z)V"),
            &[Arg::Bool(active)],
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
        // Parsing a FilePath cannot fail (its error type is `Infallible`).
        let Ok(path) = FilePath::from_str(input);
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

    /// Shows the system folder picker (`FolderIntake.pick`); the copied folder
    /// is announced by the `ferry-folder` window event.
    pub fn pick_folder() {
        let result = call_app_void(
            jni_str!("io.github.vp859.ferry.FolderIntake"),
            jni_str!("pick"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[],
        );
        if let Err(e) = result {
            warn!("Could not show the folder picker: {e}");
        }
    }

    /// Takes (and forgets) the copy of the folder picked to send: a folder
    /// inside the share cache, or nothing (cancelled, or the copy failed).
    pub fn take_picked_folder(app: &AppHandle) -> Option<String> {
        let dir = share_dir(app)?;
        let file = dir.join("pending-folder.txt");
        let path = std::fs::read_to_string(&file).ok();
        let _ = std::fs::remove_file(&file);
        let path = PathBuf::from(path?.trim());
        (path.is_dir() && path.starts_with(&dir)).then(|| path.display().to_string())
    }

    /// Takes (and forgets) the paired device chosen in the "Share" menu
    /// (Direct Share, `ShareTargets.storeTarget`).
    pub fn take_share_target(app: &AppHandle) -> Option<String> {
        let file = share_dir(app)?.join("pending-target.txt");
        let target = std::fs::read_to_string(&file).ok();
        let _ = std::fs::remove_file(&file);
        target
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// The share targets last given to Android, so an unchanged list is not
    /// sent again (the frontend reads the devices often).
    static SHARE_TARGETS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

    /// Makes the paired devices (`(Node ID, alias)`) the Direct Share targets
    /// of the "Share" menu (`ShareTargets.update`), the most recently used
    /// first (Android shows only the first few).
    pub fn set_share_targets(devices: &[(String, String)]) {
        let devices = super::share_target_order(devices, &ferry::history::last_used());
        let json = serde_json::Value::Array(
            devices
                .iter()
                .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
                .collect(),
        )
        .to_string();
        {
            let Ok(mut last) = SHARE_TARGETS.lock() else {
                return;
            };
            if *last == json {
                return;
            }
            last.clone_from(&json);
        }
        let result = call_app_void(
            jni_str!("io.github.vp859.ferry.ShareTargets"),
            jni_str!("update"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[Arg::Text(&json)],
        );
        if let Err(e) = result {
            warn!("Could not update the share targets: {e}");
            if let Ok(mut last) = SHARE_TARGETS.lock() {
                last.clear();
            }
        }
    }
}

/// Desktop: nothing to set up, paths are used as they are and there is no
/// "Share" menu.
#[cfg(not(target_os = "android"))]
mod imp {
    use ferry::transfer::service::TransferEvent;
    use std::path::{Path, PathBuf};
    use tauri::AppHandle;

    /// The default directories of the core are used.
    pub fn init(_app: &AppHandle) {}

    /// Received files stay where they were saved.
    pub fn finalize_download(_app: &AppHandle, _event: &mut TransferEvent) {}

    /// Desktop file pickers return regular paths.
    pub async fn local_path(_app: &AppHandle, input: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(input))
    }

    /// Files from the file manager ("Send with Ferry"), see `open_files`.
    pub fn take_shared_files(_app: &AppHandle) -> Vec<String> {
        crate::open_files::take()
    }

    /// There is no "Share" menu on desktop.
    pub fn take_shared_text(_app: &AppHandle) -> Option<String> {
        None
    }

    /// There is no "Share" menu on desktop.
    pub fn take_share_target(_app: &AppHandle) -> Option<String> {
        None
    }

    /// Desktop folders are picked by the dialog plugin.
    pub fn pick_folder() {}

    /// Desktop folders are picked by the dialog plugin.
    pub fn take_picked_folder(_app: &AppHandle) -> Option<String> {
        None
    }

    /// There is no "Share" menu on desktop.
    pub fn set_share_targets(_devices: &[(String, String)]) {}

    /// Desktop notifications have no buttons here: the dialog asks.
    pub fn show_question(
        _id: &str,
        _title: &str,
        _text: &str,
        _accept: &str,
        _decline: &str,
    ) -> bool {
        false
    }

    /// See [`show_question`].
    pub fn dismiss_question(_id: &str) {}

    /// Desktop systems do not limit the app in the background.
    pub fn background_status() -> Option<super::BackgroundStatus> {
        None
    }

    /// Desktop systems grant what Ferry needs without asking.
    pub fn permission_status() -> Option<super::PermissionStatus> {
        None
    }

    /// The file opens in the default app for its type.
    pub fn open_file(path: &Path) -> Result<(), String> {
        ferry::system::open_path(path).map_err(|e| e.to_string())
    }

    /// Nothing to ask for on desktop.
    pub fn request_permissions(_only_local_network: bool) {}

    /// Only Android has the app info screen with battery settings.
    pub fn open_app_settings() {}

    /// Desktop systems do not suspend a running transfer; after the last one
    /// the memory of its buffers is given back (see [`super::release_memory_later`]).
    pub fn set_transfers_active(active: bool) {
        if !active {
            super::release_memory_later();
        }
    }
}

/// Sets platform-specific directories; called before the configuration is loaded.
pub fn init(app: &AppHandle) {
    imp::init(app);
}

/// Shows the Android folder picker; the picked folder is copied into the
/// app cache and announced by the `ferry-folder` window event (no-op on desktop).
pub fn pick_folder() {
    imp::pick_folder();
}

/// The copy of the folder picked on Android (see [`pick_folder`]); `None` when
/// the picker was cancelled or on desktop.
pub fn take_picked_folder(app: &AppHandle) -> Option<String> {
    imp::take_picked_folder(app)
}

/// The paired device chosen in the Android "Share" menu for the shared
/// files or text (Direct Share); `None` when Ferry itself was chosen.
pub fn take_share_target(app: &AppHandle) -> Option<String> {
    imp::take_share_target(app)
}

/// Shows the question `id` (an offered file or text) as a notification with
/// "Accept" and "Decline" that answer it without opening Ferry (Android);
/// `false` when there is none (desktop, or notifications not allowed).
pub fn show_question(id: &str, title: &str, text: &str, accept: &str, decline: &str) -> bool {
    imp::show_question(id, title, text, accept, decline)
}

/// Removes the notification of the question `id` once it is answered or expired.
pub fn dismiss_question(id: &str) {
    imp::dismiss_question(id);
}

/// Makes the paired devices (`(Node ID, alias)`) the Direct Share targets of
/// the Android "Share" menu, the most recently used first (no-op on desktop).
pub fn set_share_targets(devices: &[(String, String)]) {
    imp::set_share_targets(devices);
}

/// `devices` (`(Node ID, alias)`) for the share menu: those used most recently
/// first (`last_used`, from the history), the others after them by name.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn share_target_order(
    devices: &[(String, String)],
    last_used: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
) -> Vec<(String, String)> {
    let mut ordered = devices.to_vec();
    ordered.sort_by(|(a_id, a_name), (b_id, b_name)| {
        // Newer first; never used (None) after every used one.
        last_used
            .get(b_id)
            .cmp(&last_used.get(a_id))
            .then_with(|| a_name.to_lowercase().cmp(&b_name.to_lowercase()))
    });
    ordered
}

/// Opens a received file in the app for its type (on Android through a
/// `content://` URI, so the other app sees only this file).
///
/// # Errors
///
/// The reason as a message for the user: no app for the type, or the file is gone.
pub fn open_file(path: &std::path::Path) -> Result<(), String> {
    imp::open_file(path)
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

/// The runtime permissions (Android); `None` on desktop.
pub fn permission_status() -> Option<PermissionStatus> {
    imp::permission_status()
}

/// Shows Android's dialog for the missing permissions, or only for the local
/// network (no-op on desktop).
pub fn request_permissions(only_local_network: bool) {
    imp::request_permissions(only_local_network);
}

/// Starts Ferry again in a new process (Android; the desktop uses Tauri's restart).
#[cfg(target_os = "android")]
pub fn restart_app() {
    imp::restart_app();
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

/// Gives memory freed by Ferry back to the system a few seconds from now
/// (after the window was unloaded or the last transfer ended, once their
/// memory is actually freed). glibc keeps freed memory in its arenas for
/// reuse, so without this the process would not shrink; other platforms and
/// C libraries return it themselves.
#[cfg(desktop)]
pub fn release_memory_later() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        trim::malloc_trim();
    });
}

/// `malloc_trim` of glibc.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod trim {
    /// Returns the free memory of all glibc arenas to the system.
    #[allow(unsafe_code)]
    pub fn malloc_trim() {
        // SAFETY: malloc_trim only releases free pages of the allocator's own
        // arenas; it takes no pointers and is thread-safe.
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

/// Turns a path or a `content://` URI from the frontend into a local file path.
pub async fn local_path(app: &AppHandle, input: &str) -> Result<std::path::PathBuf, String> {
    imp::local_path(app, input).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap;

    fn device(id: &str, name: &str) -> (String, String) {
        (id.to_string(), name.to_string())
    }

    #[test]
    fn the_share_menu_lists_the_most_recently_used_devices_first() {
        let devices = [
            device("a", "Laptop"),
            device("b", "tablet"),
            device("c", "Phone"),
            device("d", "Desktop"),
        ];
        let at = |hour| Utc.with_ymd_and_hms(2026, 11, 15, hour, 0, 0).unwrap();
        let used = HashMap::from([("c".to_string(), at(9)), ("b".to_string(), at(12))]);
        let names: Vec<_> = share_target_order(&devices, &used)
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        // Used: the tablet (12:00), then the phone (9:00); the rest by name.
        assert_eq!(names, ["tablet", "Phone", "Desktop", "Laptop"]);
        // Without a history (turned off) the order is by name.
        let names: Vec<_> = share_target_order(&devices, &HashMap::new())
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        assert_eq!(names, ["Desktop", "Laptop", "Phone", "tablet"]);
    }
}
