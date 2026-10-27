//! Desktop entry point of the Ferry GUI; see [`rqs_gui_lib`].

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Runs the application.
fn main() {
    rqs_gui_lib::run();
}
