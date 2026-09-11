use serde::{Deserialize, Serialize};
use std::process::Command;
use sysinfo::{System, RefreshKind, CpuRefreshKind};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Manager, WindowEvent};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState};

mod history;
mod parse;
mod query;

#[cfg(test)]
mod history_test;

static SYSTEM: Mutex<Option<System>> = Mutex::new(None);

#[derive(Debug, Serialize, Deserialize)]
pub struct CpuInfo {
    pub name: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub total_usage: f32,
    pub per_core_usage: Vec<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub utilization: f32,
    pub memory_used_bytes: u64,
    pub memory_allocated_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiskInfo {
    pub device: String,
    pub name: String,
    pub size_bytes: u64,
    pub smart_status: String,
    pub temperature_celsius: Option<f32>,
    pub power_on_hours: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiskThroughput {
    pub device: String,
    pub mb_per_sec: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub memory_bytes: u64,
    pub cpu_usage: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpu: GpuInfo,
    pub disks: Vec<DiskInfo>,
    pub disk_throughput: Vec<DiskThroughput>,
}

#[tauri::command]
async fn get_processes() -> Result<Vec<ProcessInfo>, String> {
    let mut sys_guard = SYSTEM.lock().map_err(|e| e.to_string())?;
    if sys_guard.is_none() {
        *sys_guard = Some(System::new_with_specifics(
            RefreshKind::new().with_cpu(CpuRefreshKind::everything())
        ));
    }

    let mut processes = Vec::new();
    if let Some(ref mut sys) = *sys_guard {
        sys.refresh_all();
        for (pid, process) in sys.processes() {
            processes.push(ProcessInfo {
                pid: pid.as_u32(),
                name: process.name().to_string(),
                memory_bytes: process.memory(),
                cpu_usage: process.cpu_usage(),
            });
        }
        // Sort by memory usage descending
        processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
        // Take top 50
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
    let start_time = now - duration_secs;

    let db_guard = history::DB.lock().map_err(|e| e.to_string())?;
    if let Some(ref db) = *db_guard {
        let mut stmt = db.conn.prepare(
            "SELECT timestamp, value FROM metric_samples
             WHERE metric_id = ?1 AND object_id = ?2 AND timestamp >= ?3
             ORDER BY timestamp ASC"
        ).map_err(|e| e.to_string())?;

        let rows = stmt.query_map([&metric_id, &object_id, &start_time.to_string()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        }).map_err(|e| e.to_string())?;

        let mut data = Vec::new();
        for row in rows {
            if let Ok((ts, val)) = row {
                data.push((ts, val));
            }
        }
        Ok(data)
    } else {
        Err("Database not initialized".to_string())
    }
}

#[tauri::command]
async fn get_system_info() -> Result<SystemInfo, String> {
    let cpu = get_cpu_info()?;
    let memory = get_memory_info()?;
    let gpu = get_gpu_info()?;
    let disks = get_disk_info()?;
    let disk_throughput = get_disk_throughput()?;

    // Record samples to history
    let _ = history::record_sample("cpu.total_usage", "system", cpu.total_usage as f64, "%");
    let _ = history::record_sample("memory.used_percent", "system", memory.used_percent as f64, "%");
    let _ = history::record_sample("gpu.utilization", "gpu0", gpu.utilization as f64, "%");
    for (idx, usage) in cpu.per_core_usage.iter().enumerate() {
        let _ = history::record_sample("cpu.per_core", &format!("core{}", idx), *usage as f64, "%");
    }
    for disk in &disk_throughput {
        let _ = history::record_sample("disk.throughput", &disk.device, disk.mb_per_sec as f64, "MB/s");
    }

    Ok(SystemInfo {
        cpu,
        memory,
        gpu,
        disks,
        disk_throughput,
    })
}

fn get_cpu_info() -> Result<CpuInfo, String> {
    // Get CPU topology
    let output = Command::new("sysctl")
        .args(&["-n", "machdep.cpu.brand_string"])
        .output()
        .map_err(|e| e.to_string())?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let output = Command::new("sysctl")
        .args(&["-n", "hw.physicalcpu"])
        .output()
        .map_err(|e| e.to_string())?;
    let physical_cores: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    let output = Command::new("sysctl")
        .args(&["-n", "hw.logicalcpu"])
        .output()
        .map_err(|e| e.to_string())?;
    let logical_cores: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    // Get per-core CPU usage via sysinfo
    let mut sys_guard = SYSTEM.lock().map_err(|e| e.to_string())?;
    if sys_guard.is_none() {
        *sys_guard = Some(System::new_with_specifics(
            RefreshKind::new().with_cpu(CpuRefreshKind::everything())
        ));
        // First refresh to initialize
        if let Some(ref mut sys) = *sys_guard {
            sys.refresh_cpu_usage();
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    let mut total_usage = 0.0;
    let mut per_core_usage = Vec::new();

    if let Some(ref mut sys) = *sys_guard {
        sys.refresh_cpu_usage();
        let cpus = sys.cpus();
        for cpu in cpus {
            per_core_usage.push(cpu.cpu_usage());
        }
        if !cpus.is_empty() {
            total_usage = cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32;
        }
    }

    Ok(CpuInfo {
        name,
        physical_cores,
        logical_cores,
        total_usage,
        per_core_usage,
    })
}

fn get_memory_info() -> Result<MemoryInfo, String> {
    let output = Command::new("sysctl")
        .args(&["-n", "hw.memsize"])
        .output()
        .map_err(|e| e.to_string())?;
    let total_bytes: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    // Get memory pressure info
    let output = Command::new("memory_pressure")
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let page_size = 16384u64; // M4 page size

    let free = extract_pages(&stdout, "Pages free:") * page_size;
    let active = extract_pages(&stdout, "Pages active:") * page_size;
    let inactive = extract_pages(&stdout, "Pages inactive:") * page_size;
    let wired = extract_pages(&stdout, "Pages wired down:") * page_size;
    let compressor = extract_pages(&stdout, "Pages used by compressor:") * page_size;
    let speculative = extract_pages(&stdout, "Pages speculative:") * page_size;
    let purgeable = extract_pages(&stdout, "Pages purgeable:") * page_size;

    let used_bytes = active + wired + compressor;
    let available_bytes = free + inactive + speculative + purgeable;
    let used_percent = (used_bytes as f32 / total_bytes as f32) * 100.0;

    Ok(MemoryInfo {
        total_bytes,
        used_bytes,
        available_bytes,
        used_percent,
    })
}

fn extract_pages(text: &str, pattern: &str) -> u64 {
    text.lines()
        .find(|line| line.contains(pattern))
        .and_then(|line| {
            line.split_whitespace()
                .nth(2)
                .and_then(|s| s.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

fn get_gpu_info() -> Result<GpuInfo, String> {
    // Get GPU name
    let output = Command::new("system_profiler")
        .args(&["SPDisplaysDataType", "-json"])
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let name = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
        json["SPDisplaysDataType"][0]["_name"]
            .as_str()
            .unwrap_or("Unknown")
            .to_string()
    } else {
        "Unknown".to_string()
    };

    // Get GPU utilization and memory from ioreg
    let output = Command::new("ioreg")
        .args(&["-l", "-w", "0"])
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut utilization = 0.0;
    let mut memory_used_bytes = 0u64;
    let mut memory_allocated_bytes = 0u64;

    // Parse PerformanceStatistics
    if let Some(start) = stdout.find("\"PerformanceStatistics\" = {") {
        if let Some(end) = stdout[start..].find('}') {
            let stats_str = &stdout[start..start + end];
            utilization = extract_stat_value(stats_str, "Device Utilization %") as f32;
            memory_used_bytes = extract_stat_value(stats_str, "In use system memory");
            memory_allocated_bytes = extract_stat_value(stats_str, "Alloc system memory");
        }
    }

    Ok(GpuInfo {
        name,
        utilization,
        memory_used_bytes,
        memory_allocated_bytes,
    })
}

fn extract_stat_value(text: &str, key: &str) -> u64 {
    text.split(',')
        .find(|part| part.contains(key))
        .and_then(|part| part.split('=').nth(1))
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn get_disk_info() -> Result<Vec<DiskInfo>, String> {
    let output = Command::new("diskutil")
        .args(&["list", "-plist", "physical"])
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut disks = Vec::new();

    if let Ok(plist) = plist::from_bytes::<plist::Value>(stdout.as_bytes()) {
        if let Some(dict) = plist.as_dictionary() {
            if let Some(array) = dict.get("AllDisksAndPartitions").and_then(|v| v.as_array()) {
                for item in array {
                    if let Some(dev_dict) = item.as_dictionary() {
                        if let Some(dev_id) = dev_dict.get("DeviceIdentifier").and_then(|v| v.as_string()) {
                            // Get detailed info for each disk
                            if let Ok(info_output) = Command::new("diskutil")
                                .args(&["info", "-plist", dev_id])
                                .output()
                            {
                                let info_stdout = String::from_utf8_lossy(&info_output.stdout);
                                if let Ok(info_plist) = plist::from_bytes::<plist::Value>(info_stdout.as_bytes()) {
                                    if let Some(info_dict) = info_plist.as_dictionary() {
                                        let temperature_celsius = info_dict.get("SMARTDeviceSpecificKeysMayVaryNotGuaranteed")
                                            .and_then(|v| v.as_dictionary())
                                            .and_then(|dict| dict.get("TEMPERATURE"))
                                            .and_then(|v| v.as_unsigned_integer())
                                            .map(|k| k as f32 - 273.15);

                                        let power_on_hours = info_dict.get("SMARTDeviceSpecificKeysMayVaryNotGuaranteed")
                                            .and_then(|v| v.as_dictionary())
                                            .and_then(|dict| dict.get("POWER_ON_HOURS_0"))
                                            .and_then(|v| v.as_unsigned_integer());

                                        disks.push(DiskInfo {
                                            device: dev_id.to_string(),
                                            name: info_dict.get("MediaName")
                                                .and_then(|v| v.as_string())
                                                .unwrap_or("")
                                                .to_string(),
                                            size_bytes: info_dict.get("TotalSize")
                                                .and_then(|v| v.as_unsigned_integer())
                                                .unwrap_or(0),
                                            smart_status: info_dict.get("SMARTStatus")
                                                .and_then(|v| v.as_string())
                                                .unwrap_or("")
                                                .to_string(),
                                            temperature_celsius,
                                            power_on_hours,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(disks)
}

fn get_disk_throughput() -> Result<Vec<DiskThroughput>, String> {
    let output = Command::new("iostat")
        .args(&["-d", "-c", "2", "-w", "1"])
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let mut throughputs = Vec::new();

    if lines.len() >= 4 {
        let disk_names: Vec<&str> = lines[0].split_whitespace().collect();
        let data_line: Vec<&str> = lines[3].split_whitespace().collect();

        for (i, disk_name) in disk_names.iter().enumerate() {
            let base_idx = i * 3;
            if base_idx + 2 < data_line.len() {
                if let Ok(mb_s) = data_line[base_idx + 2].parse::<f32>() {
                    throughputs.push(DiskThroughput {
                        device: disk_name.to_string(),
                        mb_per_sec: mb_s,
                    });
                }
            }
        }
    }

    Ok(throughputs)
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

      // Start background task for cleanup
      tauri::async_runtime::spawn(async {
        loop {
          tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
          let _ = history::cleanup_old_data();
        }
      });

      Ok(())
    })
    .on_window_event(|window, event| {
      if let WindowEvent::CloseRequested { api, .. } = event {
        // Hide window instead of closing
        window.hide().unwrap();
        api.prevent_close();
      }
    })
    .invoke_handler(tauri::generate_handler![get_system_info, get_processes, get_history])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}