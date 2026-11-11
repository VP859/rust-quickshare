//! Tauri v2 desktop and Android application of Ferry.
//!
//! The Rust side runs the P2P node ([`ferry`]) and exposes [`commands`] to the
//! web frontend in `ui/index.html`. On desktop only one instance runs: starting
//! Ferry again shows the existing window. Closing the window keeps Ferry running
//! in the system tray, or quits it when `quit_on_close` is set.

rust_i18n::i18n!("../locales", fallback = "en");

pub mod commands;
mod node;
#[cfg(desktop)]
mod open_files;
mod platform;

use commands::*;
use node::AppState;

#[cfg(desktop)]
use tauri::{Emitter, Manager};

/// Argument passed by autostart: the app starts hidden in the tray.
#[cfg(desktop)]
const MINIMIZED_ARG: &str = "--minimized";

/// Logs go to stdout on desktop and to logcat on Android (`adb logcat -s FerryCore`),
/// where stdout is discarded.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        if let Ok(layer) = tracing_android::layer("FerryCore") {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(layer)
                .try_init();
            return;
        }
    }

    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Builds and runs the application; also the mobile entry point.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    let mut builder = tauri::Builder::default();

    // Must be the first plugin: a second start hands over to the running
    // instance and exits before anything else is initialized.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            tray::show_main(app);
            // "Send with Ferry" while Ferry runs: the new start hands its files over.
            if open_files::add(open_files::paths_from_args(
                &args,
                std::path::Path::new(&cwd),
            )) {
                let _ = app.emit("share-files", ());
            }
        }));
    }

    builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init());

    #[cfg(mobile)]
    {
        builder = builder
            .plugin(tauri_plugin_barcode_scanner::init())
            .plugin(tauri_plugin_fs::init());
    }

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                Some(vec![MINIMIZED_ARG]),
            ))
            .on_window_event(|window, event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    on_close_requested(window, api);
                }
            });
    }

    builder
        .manage(AppState::new())
        .setup(|app| {
            platform::init(app.handle());
            #[cfg(desktop)]
            {
                // Files from the file manager that started Ferry.
                let args: Vec<String> = std::env::args().collect();
                let cwd = std::env::current_dir().unwrap_or_default();
                open_files::add(open_files::paths_from_args(&args, &cwd));
            }
            commands::load_config(app.handle());
            node::start(app.handle().clone());

            #[cfg(desktop)]
            {
                tray::build(app)?;
                if std::env::args().any(|a| a == MINIMIZED_ARG)
                    && let Some(window) = app.get_webview_window("main")
                {
                    window.hide()?;
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_node_info,
            list_peers,
            add_peer,
            update_peer,
            remove_peer,
            peer_online,
            send_files,
            share_file,
            start_web_share,
            stop_web_share,
            inspect_ticket,
            receive_from_ticket,
            respond_to_offer,
            list_nearby,
            refresh_nearby,
            pair_device,
            cancel_transfer,
            pause_transfer,
            get_history,
            clear_history,
            get_speed_limit,
            get_key_storage,
            set_key_storage,
            set_speed_limit,
            safety_number,
            set_verified,
            background_status,
            open_app_settings,
            permission_status,
            request_permissions,
            get_quit_on_close,
            set_quit_on_close,
            get_privacy,
            set_privacy,
            set_pairing_open,
            get_welcome,
            dismiss_welcome,
            web_share_status,
            list_localsend,
            scan_localsend,
            send_to_localsend,
            send_text_to_localsend,
            get_device_name,
            set_device_name,
            get_language,
            get_network_mode,
            set_network_mode,
            check_relays,
            create_backup,
            restore_backup,
            restart_app,
            set_language,
            open_received,
            discovery_health,
            set_download_dir,
            take_shared_files,
            take_shared_text,
            take_share_target,
            send_text,
            open_link,
            app_info,
            open_repository,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Failed to start the application: {e}");
            std::process::exit(1);
        });
}

/// Closing the main window: quits Ferry when the user chose so, otherwise hides
/// the window and keeps receiving in the background. The first time in a
/// session a notification says where Ferry went, so it does not seem to have
/// quit (on desktops without a tray, starting Ferry again shows the window).
#[cfg(desktop)]
fn on_close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    use std::sync::atomic::Ordering;
    use tauri_plugin_notification::NotificationExt;

    let app = window.app_handle();
    let state = app.state::<AppState>();
    if state.quit_on_close.load(Ordering::Relaxed) {
        app.exit(0);
        return;
    }
    api.prevent_close();
    let _ = window.hide();
    if !state.tray_hint_shown.swap(true, Ordering::Relaxed) {
        let _ = app
            .notification()
            .builder()
            .title(rust_i18n::t!("gui.notify_tray_title"))
            .body(rust_i18n::t!("gui.notify_tray"))
            .show();
    }
}

/// System tray icon and its menu (desktop only).
#[cfg(desktop)]
pub(crate) mod tray {
    use rust_i18n::t;
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{App, AppHandle, Manager, Runtime};

    /// ID of the tray icon, used to find it again in [`refresh`].
    const TRAY_ID: &str = "ferry-tray";

    /// Shows, restores and focuses the main window.
    pub fn show_main(app: &AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }

    /// Tray menu (show, send the clipboard, hide, quit) in the current language.
    fn menu<R: Runtime, M: Manager<R>>(manager: &M) -> tauri::Result<Menu<R>> {
        let show = MenuItem::with_id(manager, "show", t!("gui.tray_show"), true, None::<&str>)?;
        let clipboard = MenuItem::with_id(
            manager,
            "send_clipboard",
            t!("gui.tray_send_clipboard"),
            true,
            None::<&str>,
        )?;
        let hide = MenuItem::with_id(manager, "hide", t!("gui.tray_hide"), true, None::<&str>)?;
        let quit = MenuItem::with_id(manager, "quit", t!("gui.tray_quit"), true, None::<&str>)?;
        Menu::with_items(manager, &[&show, &clipboard, &hide, &quit])
    }

    /// Opens the window with the text of the clipboard ready to be sent: the
    /// user picks the device (nothing is sent without that choice).
    fn send_clipboard(app: &AppHandle) {
        use tauri::Emitter;
        use tauri_plugin_clipboard_manager::ClipboardExt;
        let text = app
            .clipboard()
            .read_text()
            .ok()
            .filter(|t| !t.trim().is_empty());
        show_main(app);
        let _ = app.emit("send-clipboard", text);
    }

    /// Creates the tray icon.
    pub fn build(app: &App) -> tauri::Result<()> {
        let mut tray = TrayIconBuilder::with_id(TRAY_ID)
            .tooltip(t!("gui.tray_tooltip"))
            .menu(&menu(app)?)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "show" => show_main(app),
                "send_clipboard" => send_clipboard(app),
                "hide" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.hide();
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.build(app)?;
        Ok(())
    }

    /// Re-creates the menu and tooltip in the current language.
    pub fn refresh(app: &AppHandle) {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            if let Ok(menu) = menu(app) {
                let _ = tray.set_menu(Some(menu));
            }
            let _ = tray.set_tooltip(Some(t!("gui.tray_tooltip")));
        }
    }
}
