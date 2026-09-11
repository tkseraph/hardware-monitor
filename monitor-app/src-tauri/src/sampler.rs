//! Independent collection scheduler (S3).
//!
//! One Rust-owned task samples hardware on a fixed cadence and caches the
//! latest snapshot. IPC handlers only read the cache — they never trigger
//! a collection pass, so a hidden/throttled WebView cannot stop sampling
//! and a busy page cannot distort CPU differential windows (F08, F10).

use crate::history;
use crate::storage;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{CpuRefreshKind, RefreshKind, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub name: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub total_usage: f32,
    pub per_core_usage: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub utilization: f32,
    pub memory_used_bytes: u64,
    pub memory_allocated_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub device: String,
    pub name: String,
    pub size_bytes: u64,
    pub smart_status: String,
    pub temperature_celsius: Option<f32>,
    pub power_on_hours: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskThroughput {
    pub device: String,
    pub mb_per_sec: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpu: GpuInfo,
    pub disks: Vec<DiskInfo>,
    pub disk_throughput: Vec<DiskThroughput>,
    /// Full physical-disk → container → volume topology (S5).
    pub storage: Vec<storage::PhysicalDisk>,
    /// Unix seconds when this snapshot was sampled — not when it was read.
    pub observed_at: i64,
}

/// The single owner of CPU differential state and the latest snapshot.
pub struct Sampler {
    sys: System,
    cpu_static: Option<(String, u32, u32)>,
    /// When the sampler last produced a snapshot; used to compute real
    /// differential windows instead of assuming a fixed interval.
    last_tick: Option<Instant>,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::new().with_cpu(CpuRefreshKind::everything()),
        );
        sys.refresh_cpu_usage();
        Self {
            sys,
            cpu_static: None,
            last_tick: None,
        }
    }

    fn cpu_static(&mut self) -> (String, u32, u32) {
        if let Some(c) = &self.cpu_static {
            return c.clone();
        }
        let name = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string());
        let physical = read_sysctl_u32("hw.physicalcpu").unwrap_or(0);
        let logical = read_sysctl_u32("hw.logicalcpu").unwrap_or(0);
        let c = (name, physical, logical);
        self.cpu_static = Some(c.clone());
        c
    }

    /// Produce one snapshot. CPU usage is differential against the previous
    /// tick; the first tick after start/wake returns whatever sysinfo has
    /// since its internal baseline, which callers treat as warm-up.
    pub fn sample(&mut self) -> Result<SystemInfo, String> {
        let (name, physical, logical) = self.cpu_static();

        self.sys.refresh_cpu_usage();
        let cpus = self.sys.cpus();
        let per_core: Vec<f32> = cpus.iter().map(|c| c.cpu_usage()).collect();
        let total = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
        };
        self.last_tick = Some(Instant::now());

        let cpu = CpuInfo {
            name,
            physical_cores: physical,
            logical_cores: logical,
            total_usage: total,
            per_core_usage: per_core,
        };

        let memory = sample_memory()?;
        let gpu = sample_gpu()?;
        // Build the storage topology once, then derive the flat disk list
        // and throughput device set from it so they can never disagree (F05).
        let storage = storage::build_topology().unwrap_or_default();
        let disks: Vec<DiskInfo> = storage.iter().map(|d| DiskInfo {
            device: d.device.clone(),
            name: d.name.clone(),
            size_bytes: d.size_bytes,
            smart_status: d.smart_status.clone(),
            temperature_celsius: d.temperature_celsius,
            power_on_hours: d.power_on_hours,
        }).collect();
        let devices: Vec<String> = disks.iter().map(|d| d.device.clone()).collect();
        let disk_throughput = sample_disk_throughput(&devices)?;

        let observed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;

        Ok(SystemInfo {
            cpu,
            memory,
            gpu,
            disks,
            disk_throughput,
            storage,
            observed_at,
        })
    }
}

fn read_sysctl_u32(key: &str) -> Option<u32> {
    Command::new("sysctl")
        .args(["-n", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
}

fn sample_memory() -> Result<MemoryInfo, String> {
    let total_bytes = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .map_err(|e| e.to_string())
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "hw.memsize not a number".to_string())
            } else {
                Err("sysctl hw.memsize failed".to_string())
            }
        })?;
    if total_bytes == 0 {
        return Err("hw.memsize is zero".to_string());
    }

    let page_size = read_sysctl_u32("hw.pagesize")
        .filter(|&p| p > 0)
        .unwrap_or(16384) as u64;

    let output = Command::new("memory_pressure")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("memory_pressure failed".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    let pages = |key: &str| -> Result<u64, String> {
        crate::parse::parse_labeled_number(&stdout, key)
            .ok_or_else(|| format!("memory_pressure missing '{}'", key))
    };

    let free = pages("Pages free:")? * page_size;
    let active = pages("Pages active:")? * page_size;
    let inactive = pages("Pages inactive:")? * page_size;
    let wired = pages("Pages wired down:")? * page_size;
    let compressor = pages("Pages used by compressor:")? * page_size;
    let speculative = pages("Pages speculative:")? * page_size;
    let purgeable = pages("Pages purgeable:")? * page_size;

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

fn sample_gpu() -> Result<GpuInfo, String> {
    let name = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&o.stdout)).ok())
        .and_then(|j| j["SPDisplaysDataType"][0]["_name"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown".to_string());

    let output = Command::new("ioreg")
        .args(["-l", "-w", "0"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("ioreg failed".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    let stats_start = stdout
        .find("\"PerformanceStatistics\" = {")
        .ok_or("no PerformanceStatistics block")?;
    let stats_end = stdout[stats_start..]
        .find('}')
        .map(|e| stats_start + e)
        .ok_or("unterminated PerformanceStatistics block")?;
    let stats_str = &stdout[stats_start..=stats_end];

    let utilization = crate::parse::parse_ioreg_stat(stats_str, "Device Utilization %")
        .ok_or("missing Device Utilization %")? as f32;
    let memory_used_bytes = crate::parse::parse_ioreg_stat(stats_str, "In use system memory")
        .ok_or("missing In use system memory")?;
    let memory_allocated_bytes = crate::parse::parse_ioreg_stat(stats_str, "Alloc system memory")
        .ok_or("missing Alloc system memory")?;

    Ok(GpuInfo {
        name,
        utilization,
        memory_used_bytes,
        memory_allocated_bytes,
    })
}

fn sample_disk_throughput(physical_devices: &[String]) -> Result<Vec<DiskThroughput>, String> {
    if physical_devices.is_empty() {
        return Ok(Vec::new());
    }
    let mut args: Vec<String> = vec!["-d".into(), "-c".into(), "2".into(), "-w".into(), "1".into()];
    for dev in physical_devices {
        args.push(dev.clone());
    }
    let output = Command::new("iostat")
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("iostat exited with {}", output.status));
    }

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

/// Record one snapshot's metrics to history with the snapshot's own
/// observation timestamp, not a fresh per-row timestamp (F11).
pub fn record_snapshot(info: &SystemInfo) {
    let ts = info.observed_at;
    let _ = history::record_sample_at("cpu.total_usage", "system", info.cpu.total_usage as f64, "%", ts);
    let _ = history::record_sample_at("memory.used_percent", "system", info.memory.used_percent as f64, "%", ts);
    let _ = history::record_sample_at("gpu.utilization", "gpu0", info.gpu.utilization as f64, "%", ts);
    for (idx, usage) in info.cpu.per_core_usage.iter().enumerate() {
        let _ = history::record_sample_at("cpu.per_core", &format!("core{}", idx), *usage as f64, "%", ts);
    }
    for disk in &info.disk_throughput {
        let _ = history::record_sample_at("disk.throughput", &disk.device, disk.mb_per_sec as f64, "MB/s", ts);
    }
}

// ---------- Shared scheduler state ----------

static SAMPLER: Mutex<Option<Sampler>> = Mutex::new(None);
static LATEST: Mutex<Option<SystemInfo>> = Mutex::new(None);
/// True while the main window is visible → foreground cadence.
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

pub fn set_window_visible(visible: bool) {
    WINDOW_VISIBLE.store(visible, Ordering::SeqCst);
}

/// Latest cached snapshot, if the scheduler has produced one yet.
pub fn latest_snapshot() -> Option<SystemInfo> {
    LATEST.lock().ok()?.clone()
}

/// Run the collection loop forever. Intended to be spawned once at setup.
/// The interval switches between the configured foreground/background
/// cadence based on window visibility; sampling continues regardless so
/// history has no WebView-throttling gap (F08). Settings are re-read each
/// cycle so changes take effect without a restart (S8).
pub async fn run_scheduler() {
    {
        let mut guard = SAMPLER.lock().expect("sampler lock poisoned");
        *guard = Some(Sampler::new());
    }
    loop {
        let s = crate::settings::get();
        let interval = if WINDOW_VISIBLE.load(Ordering::SeqCst) {
            Duration::from_millis(s.foreground_interval_ms)
        } else {
            Duration::from_millis(s.background_interval_ms)
        };
        tokio::time::sleep(interval).await;

        let snapshot = {
            let mut guard = match SAMPLER.lock() {
                Ok(g) => g,
                Err(_) => continue,
            };
            match guard.as_mut() {
                Some(s) => s.sample().ok(),
                None => None,
            }
        };

        if let Some(info) = snapshot {
            record_snapshot(&info);
            if let Ok(mut latest) = LATEST.lock() {
                *latest = Some(info);
            }
        }
    }
}
