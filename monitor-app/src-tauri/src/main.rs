// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "windows")]
    if app_lib::enhanced::storage_service::entry() {
        return;
    }
    app_lib::run();
}
