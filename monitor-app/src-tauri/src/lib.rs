use tauri::{Manager, WindowEvent};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState};

mod cmd;
mod data_paths;
mod history;
mod instance;
mod loginitem;
mod metric_status;
mod parse;
mod processes;
mod query;
mod sampler;
mod settings;
mod storage;
mod termination;

#[cfg(test)]
mod history_test;

pub use sampler::SystemInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessSort {
    Memory,
    Cpu,
    DiskRead,
    DiskWrite,
}

/// Scan, then filter/sort/paginate on the FULL readable set — never
/// truncate before filtering, which previously hid low-memory high-CPU
/// processes from a CPU sort (F13).
#[tauri::command]
async fn get_processes(
    search: Option<String>,
    sort: Option<ProcessSort>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<processes::ProcessPage, String> {
    let mut page = processes::scan_processes()?;

    // Filter by name or PID substring over the full set.
    if let Some(q) = search.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
        page.processes.retain(|p| {
            p.name.to_lowercase().contains(&q) || p.pid.to_string().contains(&q)
        });
    }

    // Sort the filtered full set. R6: unknown rates sort AFTER known ones
    // (None is never treated as 0 and never ranks above a real reading).
    let none_last = |a: &Option<f64>, b: &Option<f64>| match (a, b) {
        (Some(x), Some(y)) => y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal),
        (None, Some(_)) => std::cmp::Ordering::Greater, // None sinks to the end
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    };
    match sort.unwrap_or(ProcessSort::Memory) {
        ProcessSort::Memory => page.processes.sort_by(|a, b| {
            b.memory_bytes.cmp(&a.memory_bytes).then(a.pid.cmp(&b.pid))
        }),
        ProcessSort::Cpu => page.processes.sort_by(|a, b| {
            b.cpu_usage.partial_cmp(&a.cpu_usage).unwrap_or(std::cmp::Ordering::Equal).then(a.pid.cmp(&b.pid))
        }),
        ProcessSort::DiskRead => page.processes.sort_by(|a, b| {
            none_last(&a.disk_read_bps, &b.disk_read_bps).then(a.pid.cmp(&b.pid))
        }),
        ProcessSort::DiskWrite => page.processes.sort_by(|a, b| {
            none_last(&a.disk_write_bps, &b.disk_write_bps).then(a.pid.cmp(&b.pid))
        }),
    }

    // R6: record how many rows matched the filter BEFORE pagination so the UI
    // can render real page controls ("page X of N") instead of a fixed window.
    page.matched_total = page.processes.len();

    // Paginate after sort.
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(50).min(500);
    page.offset = offset;
    page.limit = limit;
    page.processes = page.processes.into_iter().skip(offset).take(limit).collect();

    Ok(page)
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

/// R4: combined health view. IPC success is NOT collection success — this DTO
/// carries sampling freshness and history-write health so the frontend can
/// show "数据可能过期" instead of a stale value labeled "实时采集中" (A03).
#[derive(Debug, serde::Serialize)]
struct SystemStatus {
    sampling: metric_status::SamplingHealth,
    history_health: &'static str,
}

#[tauri::command]
async fn get_system_status() -> Result<SystemStatus, String> {
    let hh = match history::health() {
        history::HistoryHealth::Ok => "ok",
        history::HistoryHealth::OverBudget => "over_budget",
        history::HistoryHealth::WriteError => "write_error",
        history::HistoryHealth::Unavailable => "unavailable",
    };
    Ok(SystemStatus {
        sampling: metric_status::health(),
        history_health: hh,
    })
}

#[tauri::command]
async fn get_settings() -> Result<settings::Settings, String> {
    Ok(settings::get())
}

#[tauri::command]
async fn set_settings(new_settings: settings::Settings) -> Result<(), String> {
    settings::set(new_settings)
}

/// Opt-in login item toggle. Returns the resulting registered state.
#[tauri::command]
async fn set_launch_at_login(enable: bool, app: tauri::AppHandle) -> Result<bool, String> {
    let app_path = std::env::current_exe()
        .ok()
        .and_then(|p| {
            // exe is at monitor.app/Contents/MacOS/monitor; the bundle root
            // is three levels up.
            p.ancestors().nth(3).map(|a| a.to_path_buf())
        })
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .ok_or("could not resolve .app bundle path")?;
    loginitem::set_launch_at_login(enable, &app_path)?;
    let registered = loginitem::is_registered(&app_path).unwrap_or(enable);
    // Persist the user's intent in settings regardless of query accuracy.
    let mut s = settings::get();
    s.launch_at_login = registered;
    let _ = settings::set(s);
    let _ = app; // app handle reserved for future native registration APIs
    Ok(registered)
}

#[tauri::command]
async fn terminate_process(pid: u32, start_marker: u64) -> Result<(), String> {
    termination::request(pid, start_marker)
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

      // Resolve the single data root once (R3/A07): MONITOR_DATA_DIR isolation
      // applies to BOTH the DB and settings; the real app_data_dir is not
      // touched when the override is set.
      let paths = match app.path().app_data_dir() {
        Ok(dir) => match data_paths::DataPaths::resolve(dir) {
          Ok(p) => Some(p),
          Err(e) => {
            log::error!("failed to resolve data dir: {:?}", e);
            None
          }
        },
        Err(e) => {
          log::error!("no app data dir: {:?}", e);
          None
        }
      };

      // Single-instance guard (R3/A08): keyed by the canonical data dir. A
      // second instance on the SAME dir does not sample; a different
      // MONITOR_DATA_DIR gets an independent lock and may run in parallel.
      let acquired = paths.as_ref().and_then(|p| {
        match instance::try_acquire(&p.lock_path()) {
          Ok(instance::Acquire::Acquired(lock)) => Some(lock),
          Ok(instance::Acquire::AlreadyRunning) => {
            log::warn!("another instance owns this data dir; sampling disabled");
            None
          }
          Err(e) => {
            log::error!("instance lock error: {:?}", e);
            None
          }
        }
      });
      let is_primary = acquired.is_some() || paths.is_none();

      // Initialize history DB + settings from the shared DataPaths. A DB
      // failure degrades to "realtime ok, history unavailable" (R2/A05): the
      // app keeps sampling; it does not panic or create a substitute empty DB.
      if let Some(p) = &paths {
        if is_primary {
          if let Err(e) = history::init_db_at(p.db_path()) {
            log::error!("history DB init failed; continuing without history: {:?}", e);
          }
        }
        settings::init(p.settings_dir());
      } else {
        log::error!("no data dir; history and settings persistence disabled");
      }

      // Create menu bar tray icon
      let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
      let show_item = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
      let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

      // Dedicated template icon so the tray renders in the menu bar on both
      // light and dark themes. The colored app icon renders as an invisible
      // glyph when macOS templates it.
      let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
        .expect("failed to load tray icon");
      let _tray = TrayIconBuilder::new()
        .icon(tray_icon)
        .icon_as_template(true)
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
      // Only the primary instance (holding the data-dir lock) samples; a
      // second instance on the same dir shows a window but does not double-write.
      if is_primary {
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
      }

      // Keep the lock alive for the process lifetime by leaking it into a
      // static holder; dropping it would release the lock prematurely.
      if let Some(lock) = acquired {
        std::mem::forget(lock);
      }

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
    .invoke_handler(tauri::generate_handler![
      get_system_info,
      get_system_status,
      get_processes,
      terminate_process,
      get_history,
      get_settings,
      set_settings,
      set_launch_at_login,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
