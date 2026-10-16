pub mod commands;

use commands::*;
use rqs_p2p::config::Config;
use std::collections::HashMap;
use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Pluginy mobilne rejestrowane wyłącznie na Androidzie/iOS –
        // na desktopie zbiera je osobna gałąź cfg poniżej, dzięki czemu
        // `cargo build` na Linuksie/Windows/macOS nie wymaga tych crate'ów.
        #[cfg(mobile)]
        .plugin(tauri_plugin_notification::init())
        #[cfg(mobile)]
        .plugin(tauri_plugin_barcode_scanner::init())
        #[cfg(mobile)]
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let quit_i = MenuItem::with_id(app, "quit", "Zakończ RQS", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Pokaż Okno", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app_handle, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app_handle.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .manage(AppState {
            config: Mutex::new(Config::load().unwrap_or_default()),
            pending_responses: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            get_node_info,
            send_file_offer,
            respond_to_offer,
            get_trusted_peers,
            add_trusted_peer,
            remove_trusted_peer,
            pick_file_dialog,
            discover_devices,
        ])
        .run(tauri::generate_context!())
        .expect("Błąd podczas uruchamiania aplikacji Tauri v2");
}
