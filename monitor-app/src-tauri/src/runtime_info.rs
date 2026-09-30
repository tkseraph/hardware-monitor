//! Capabilities come from the backend, never from navigator.userAgent.
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

static PRIMARY: AtomicBool = AtomicBool::new(true);
pub fn set_primary(value: bool) {
    PRIMARY.store(value, Ordering::SeqCst);
}
pub fn is_primary() -> bool {
    PRIMARY.load(Ordering::SeqCst)
}

#[derive(Serialize)]
pub struct RuntimeInfo {
    pub platform: &'static str,
    pub terminate_process: bool,
    pub launch_at_login: bool,
    pub process_disk_io: bool,
    pub primary_instance: bool,
    pub effective_interval_ms: u64,
}

#[tauri::command]
pub fn get_runtime_info() -> RuntimeInfo {
    #[cfg(target_os = "macos")]
    let settings = crate::settings::get();
    RuntimeInfo {
        platform: std::env::consts::OS,
        terminate_process: cfg!(target_os = "macos"),
        launch_at_login: cfg!(target_os = "macos"),
        process_disk_io: cfg!(any(target_os = "macos", target_os = "windows")),
        primary_instance: is_primary(),
        // Conservative freshness limit for either foreground/background mode.
        #[cfg(target_os = "macos")]
        effective_interval_ms: settings
            .foreground_interval_ms
            .max(settings.background_interval_ms),
        #[cfg(target_os = "windows")]
        effective_interval_ms: crate::sampler::effective_interval_ms(),
    }
}
