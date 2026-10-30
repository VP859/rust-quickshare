//! Build script: generates the Tauri context (configuration, icons, frontend).

/// Runs the Tauri build steps.
fn main() {
    tauri_build::build();
}
