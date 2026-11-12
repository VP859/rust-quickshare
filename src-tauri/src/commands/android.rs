//! Android: what limits Ferry in the background, its runtime permissions and
//! the app settings (the commands answer `null` or do nothing on desktop).

/// What limits Ferry in the background on Android; `null` on desktop.
#[tauri::command]
pub fn background_status() -> Option<crate::platform::BackgroundStatus> {
    crate::platform::background_status()
}

/// The runtime permissions on Android (local network, notifications);
/// `null` on desktop.
#[tauri::command]
pub fn permission_status() -> Option<crate::platform::PermissionStatus> {
    crate::platform::permission_status()
}

/// Shows Android's dialog for the missing permissions (`only:
/// "local_network"` – just that one, when a search starts); the answer arrives
/// as the `ferry-permissions` event.
#[tauri::command]
pub fn request_permissions(only: Option<String>) {
    crate::platform::request_permissions(only.as_deref() == Some("local_network"));
}

/// Opens the system app info screen of Ferry (Android: battery usage).
#[tauri::command]
pub fn open_app_settings() {
    crate::platform::open_app_settings();
}
