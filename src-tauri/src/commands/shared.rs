//! What other apps and system pickers hand over: files and texts from the
//! Android "Share" menu or a file manager ("Send with Ferry"), the device
//! chosen in the share menu, and a folder picked on Android.

use tauri::AppHandle;

/// Paths of files shared from the Android "Share" menu (empty on desktop).
#[tauri::command]
pub fn take_shared_files(app: AppHandle) -> Vec<String> {
    crate::platform::take_shared_files(&app)
}

/// A text or link shared from the Android "Share" menu (none on desktop).
#[tauri::command]
pub fn take_shared_text(app: AppHandle) -> Option<String> {
    crate::platform::take_shared_text(&app)
}

/// Shows the Android folder picker (the dialog plugin has none on mobile);
/// the frontend waits for the `ferry-folder` window event.
#[tauri::command]
pub fn pick_folder() {
    crate::platform::pick_folder();
}

/// The copy of the folder picked with [`pick_folder`], ready to send; `None`
/// when the picker was cancelled.
#[tauri::command]
pub fn take_picked_folder(app: AppHandle) -> Option<String> {
    crate::platform::take_picked_folder(&app)
}

/// The paired device chosen in the Android "Share" menu (Direct Share) for
/// what `take_shared_files` / `take_shared_text` return; `None` otherwise.
#[tauri::command]
pub fn take_share_target(app: AppHandle) -> Option<String> {
    crate::platform::take_share_target(&app)
}
