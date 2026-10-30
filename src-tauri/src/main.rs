//! Desktop entry point of the Ferry GUI; see [`ferry_gui_lib`].

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Runs the application.
fn main() {
    ferry_gui_lib::run();
}
