//! Tauri v2 desktop and Android application of Ferry.
//!
//! The Rust side runs the P2P node ([`rqs_p2p`]) and exposes [`commands`] to the
//! web frontend in `ui/index.html`. On desktop the app keeps running in the
//! system tray when its window is closed.

#![warn(missing_docs)]

rust_i18n::i18n!("../locales", fallback = "en");

pub mod commands;
mod node;
mod platform;

use commands::*;
use node::AppState;

#[cfg(desktop)]
use tauri::Manager;

/// Argument passed by autostart: the app starts hidden in the tray.
#[cfg(desktop)]
const MINIMIZED_ARG: &str = "--minimized";

/// Logs go to stdout on desktop and to logcat on Android (`adb logcat -s RqsCore`),
/// where stdout is discarded.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        if let Ok(layer) = tracing_android::layer("RqsCore") {
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

    let mut builder = tauri::Builder::default()
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
                    api.prevent_close();
                    let _ = window.hide();
                }
            });
    }

    builder
        .manage(AppState::new())
        .setup(|app| {
            platform::init(app.handle());
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
            inspect_ticket,
            receive_from_ticket,
            respond_to_offer,
            list_nearby,
            refresh_nearby,
            pair_code,
            pair_device,
            get_device_name,
            set_device_name,
            get_language,
            set_language,
            open_received,
            discovery_health,
            set_download_dir,
            take_shared_files,
            app_info,
            open_repository,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Failed to start the application: {e}");
            std::process::exit(1);
        });
}

/// System tray icon and its menu (desktop only).
#[cfg(desktop)]
pub(crate) mod tray {
    use rust_i18n::t;
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{App, AppHandle, Manager, Runtime};

    const TRAY_ID: &str = "rqs-tray";

    fn show_main(app: &AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }

    fn menu<R: Runtime, M: Manager<R>>(manager: &M) -> tauri::Result<Menu<R>> {
        let show = MenuItem::with_id(manager, "show", t!("gui.tray_show"), true, None::<&str>)?;
        let hide = MenuItem::with_id(manager, "hide", t!("gui.tray_hide"), true, None::<&str>)?;
        let quit = MenuItem::with_id(manager, "quit", t!("gui.tray_quit"), true, None::<&str>)?;
        Menu::with_items(manager, &[&show, &hide, &quit])
    }

    /// Creates the tray icon.
    pub fn build(app: &App) -> tauri::Result<()> {
        let mut tray = TrayIconBuilder::with_id(TRAY_ID)
            .tooltip(t!("gui.tray_tooltip"))
            .menu(&menu(app)?)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "show" => show_main(app),
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
