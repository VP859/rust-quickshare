pub mod commands;
mod node;
mod platform;

use commands::*;
use node::AppState;

#[cfg(desktop)]
use tauri::Manager;

/// Argument przekazywany przez autostart – aplikacja startuje ukryta w zasobniku.
#[cfg(desktop)]
const MINIMIZED_ARG: &str = "--minimized";

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Na Androidzie stdout nie trafia nigdzie – logi kierujemy do logcat
    // (`adb logcat -s RqsCore`).
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
            // Zamknięcie okna chowa je do zasobnika – węzeł nadal odbiera pliki.
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
            open_received,
            discovery_health,
            set_download_dir,
            take_shared_files,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Błąd podczas uruchamiania aplikacji Tauri v2: {e}");
            std::process::exit(1);
        });
}

#[cfg(desktop)]
mod tray {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{App, AppHandle, Manager};

    fn show_main(app: &AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }

    pub fn build(app: &App) -> tauri::Result<()> {
        let show = MenuItem::with_id(app, "show", "Pokaż okno", true, None::<&str>)?;
        let hide = MenuItem::with_id(app, "hide", "Ukryj do zasobnika", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Zakończ RQS", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

        let mut tray = TrayIconBuilder::with_id("rqs-tray")
            .tooltip("RQS P2P Quick Share – nasłuchuje")
            .menu(&menu)
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
}
