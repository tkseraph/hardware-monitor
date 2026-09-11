use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use sysinfo::System;
use tauri::{Manager, WindowEvent};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState};

mod history;
mod parse;
mod query;
mod sampler;
mod storage;

#[cfg(test)]
mod history_test;

pub use sampler::SystemInfo;

/// Process enumeration uses its own System instance so refreshing it never
/// perturbs the CPU differential baseline owned by the sampler (F10).
static PROCESS_SYSTEM: Mutex<Option<System>> = Mutex::new(None);

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub memory_bytes: u64,
    pub cpu_usage: f32,
}

#[tauri::command]
async fn get_processes() -> Result<Vec<ProcessInfo>, String> {
    let mut sys_guard = PROCESS_SYSTEM.lock().map_err(|e| e.to_string())?;
    if sys_guard.is_none() {
        *sys_guard = Some(System::new());
    }

    let mut processes = Vec::new();
    if let Some(ref mut sys) = *sys_guard {
        // Refresh only processes, not CPU — the sampler owns CPU state.
        sys.refresh_processes();
        for (pid, process) in sys.processes() {
            processes.push(ProcessInfo {
                pid: pid.as_u32(),
                name: process.name().to_string(),
                memory_bytes: process.memory(),
                cpu_usage: process.cpu_usage(),
            });
        }
        // Sort by memory descending, then truncate to top 50.
        processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
        processes.truncate(50);
    }
    Ok(processes)
}

#[tauri::command]
async fn get_history(metric_id: String, object_id: String, duration_secs: i64) -> Result<Vec<(i64, f64)>, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Validate the query before touching the DB (F14).
    let q = query::validate_query(&metric_id, &object_id, duration_secs, now)
        .map_err(|e| format!("invalid history query: {:?}", e))?;

    history::query_range(&q.metric_id, &q.object_id, q.start_secs, q.end_secs, q.max_points)
        .map_err(|e| e.to_string())
}

/// IPC returns the scheduler's cached snapshot. It never triggers a
/// collection pass, so page load / polling cannot distort sampling (F08).
#[tauri::command]
async fn get_system_info() -> Result<SystemInfo, String> {
    sampler::latest_snapshot()
        .ok_or_else(|| "collector is still warming up".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }

      // Initialize history database
      let app_data_dir = app.path().app_data_dir().expect("failed to get app data dir");
      std::fs::create_dir_all(&app_data_dir).expect("failed to create app data dir");
      history::init_db(app_data_dir).expect("failed to init database");

      // Create menu bar tray icon
      let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
      let show_item = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
      let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

      let _tray = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Hardware Monitor")
        .on_menu_event(|app, event| match event.id.as_ref() {
          "quit" => {
            app.exit(0);
          }
          "show" => {
            if let Some(window) = app.get_webview_window("main") {
              let _ = window.show();
              let _ = window.set_focus();
            }
          }
          _ => {}
        })
        .on_tray_icon_event(|tray, event| {
          if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
          } = event
          {
            let app = tray.app_handle();
            if let Some(window) = app.get_webview_window("main") {
              let _ = window.show();
              let _ = window.set_focus();
            }
          }
        })
        .build(app)?;

      // Independent Rust scheduler: samples hardware on a cadence even when
      // the WebView is hidden/throttled, and is the only writer of history.
      tauri::async_runtime::spawn(async {
        sampler::run_scheduler().await;
      });

      // Aggregation + retention pass every 60s. Aggregation into buckets is
      // verified inside a transaction before any source row is deleted (F01).
      tauri::async_runtime::spawn(async {
        loop {
          tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
          let _ = history::aggregate_and_prune();
        }
      });

      Ok(())
    })
    .on_window_event(|window, event| {
      match event {
        WindowEvent::CloseRequested { api, .. } => {
          // Hide window instead of closing; sampling continues in background.
          window.hide().unwrap();
          sampler::set_window_visible(false);
          api.prevent_close();
        }
        WindowEvent::Focused(true) => {
          sampler::set_window_visible(true);
        }
        WindowEvent::Focused(false) => {
          // Only drop to background cadence if the window is also hidden.
          if !window.is_visible().unwrap_or(true) {
            sampler::set_window_visible(false);
          }
        }
        _ => {}
      }
    })
    .invoke_handler(tauri::generate_handler![get_system_info, get_processes, get_history])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
