//! Independent collection scheduler (S3).
//!
//! One Rust-owned task samples hardware on a fixed cadence and caches the
//! latest snapshot. IPC handlers only read the cache — they never trigger
//! a collection pass, so a hidden/throttled WebView cannot stop sampling
//! and a busy page cannot distort CPU differential windows (F08, F10).

use crate::history;
use crate::storage;
use serde::{Deserialize, Serialize};
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
    /// Current enumeration address (e.g. "disk0") — volatile, display/live only.
    pub device: String,
    /// R7/A10: stable anonymous identity used as the history object_id, so a
    /// reused `diskN` never joins a previous disk's series. Empty until the
    /// identity registry assigns one; falls back to `device` for display.
    #[serde(default)]
    pub device_uid: String,
    /// Generation of `device_uid`; bumped when the same `device` address is
    /// re-seen as a physically different medium (hot-plug) (A10).
    #[serde(default)]
    pub generation: u32,
    pub name: String,
    pub size_bytes: u64,
    pub smart_status: String,
    pub temperature_celsius: Option<f32>,
    pub power_on_hours: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskThroughput {
    /// Current enumeration address (live label / command arg).
    pub device: String,
    /// R7/A10: stable anonymous identity used as the history object_id.
    #[serde(default)]
    pub device_uid: String,
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
    /// Per-device cumulative MB read from `iostat -d -I`, with the observation
    /// time. Throughput is the delta over the real elapsed window, so the
    /// collection pass no longer pays iostat's fixed 1s sampling delay (R5/A04).
    disk_cumulative: std::collections::HashMap<String, (f64, Instant)>,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys =
            System::new_with_specifics(RefreshKind::new().with_cpu(CpuRefreshKind::everything()));
        sys.refresh_cpu_usage();
        Self {
            sys,
            cpu_static: None,
            last_tick: None,
            disk_cumulative: std::collections::HashMap::new(),
        }
    }

    fn cpu_static(&mut self) -> (String, u32, u32) {
        if let Some(c) = &self.cpu_static {
            return c.clone();
        }
        let name = crate::cmd::run(
            "/usr/sbin/sysctl",
            &["-n", "machdep.cpu.brand_string"],
            crate::cmd::DEFAULT_TIMEOUT,
        )
        .ok()
        .filter(|o| o.status_success)
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
        let mut storage = storage::build_topology().unwrap_or_default();
        // R7/A10: assign stable anonymous identities so history keys off the
        // device_uid, never the volatile `diskN` address.
        crate::device_id::assign_topology(&mut storage);
        let disks: Vec<DiskInfo> = storage
            .iter()
            .map(|d| DiskInfo {
                device: d.device.clone(),
                device_uid: d.device_uid.clone().unwrap_or_default(),
                generation: d.generation,
                name: d.name.clone(),
                size_bytes: d.size_bytes,
                smart_status: d.smart_status.clone(),
                temperature_celsius: d.temperature_celsius,
                power_on_hours: d.power_on_hours,
            })
            .collect();
        let devices: Vec<String> = disks.iter().map(|d| d.device.clone()).collect();
        let mut disk_throughput = self.sample_disk_throughput(&devices)?;
        // R7/A10: tag each throughput with the disk's anonymous uid so history
        // is keyed by device_uid, not the volatile `diskN` address.
        for tp in disk_throughput.iter_mut() {
            if let Some(d) = disks.iter().find(|d| d.device == tp.device) {
                tp.device_uid = d.device_uid.clone();
            }
        }

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

    /// Per-device throughput via cumulative counters (R5/A04). Reads
    /// `iostat -d -I` (cumulative MB since boot — no 1s sampling delay), then
    /// computes MB/s as the delta over the real elapsed window since the last
    /// observation. The first call after start/wake establishes the baseline
    /// and reports no rate (warm-up), so no fabricated spike is produced.
    fn sample_disk_throughput(
        &mut self,
        physical_devices: &[String],
    ) -> Result<Vec<DiskThroughput>, String> {
        if physical_devices.is_empty() {
            return Ok(Vec::new());
        }
        let mut args: Vec<&str> = vec!["-d", "-I"];
        for dev in physical_devices {
            args.push(dev.as_str());
        }
        let out = crate::cmd::run("/usr/sbin/iostat", &args, crate::cmd::DEFAULT_TIMEOUT)?;
        if out.timed_out {
            return Err("iostat timed out".to_string());
        }
        if !out.status_success {
            return Err("iostat exited non-zero".to_string());
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let cumulative = parse_iostat_cumulative_mb(&stdout);

        let now = Instant::now();
        let mut throughputs = Vec::new();
        for dev in physical_devices {
            let cur = match cumulative.get(dev.as_str()) {
                Some(&v) => v,
                None => continue, // device absent from iostat output — skip
            };
            match self.disk_cumulative.get(dev) {
                Some(&(prev_mb, prev_at)) => {
                    let dt = now.duration_since(prev_at).as_secs_f32();
                    if dt > 0.0 && cur >= prev_mb {
                        let mb_s = ((cur - prev_mb) as f32) / dt;
                        throughputs.push(DiskThroughput {
                            device: dev.clone(),
                            device_uid: String::new(),
                            mb_per_sec: mb_s,
                        });
                    }
                    // Counter went backwards (counter reset / disk re-add): drop
                    // the stale baseline; the new value becomes the baseline.
                }
                None => {
                    // First observation: baseline only, no rate this pass.
                }
            }
            self.disk_cumulative.insert(dev.clone(), (cur, now));
        }
        Ok(throughputs)
    }
}

fn read_sysctl_u32(key: &str) -> Option<u32> {
    crate::cmd::run(
        "/usr/sbin/sysctl",
        &["-n", key],
        crate::cmd::DEFAULT_TIMEOUT,
    )
    .ok()
    .filter(|o| o.status_success)
    .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
}

fn sample_memory() -> Result<MemoryInfo, String> {
    let total_bytes = crate::cmd::run(
        "/usr/sbin/sysctl",
        &["-n", "hw.memsize"],
        crate::cmd::DEFAULT_TIMEOUT,
    )?
    .to_result()
    .and_then(|s| {
        s.trim()
            .parse::<u64>()
            .map_err(|_| "hw.memsize not a number".to_string())
    })?;
    if total_bytes == 0 {
        return Err("hw.memsize is zero".to_string());
    }

    let page_size = read_sysctl_u32("hw.pagesize")
        .filter(|&p| p > 0)
        .unwrap_or(16384) as u64;

    let stdout = crate::cmd::run("/usr/bin/memory_pressure", &[], crate::cmd::DEFAULT_TIMEOUT)?
        .to_result()?;

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
    let name = crate::cmd::run(
        "/usr/sbin/system_profiler",
        &["SPDisplaysDataType", "-json"],
        crate::cmd::DEFAULT_TIMEOUT,
    )
    .ok()
    .and_then(|o| o.to_result().ok())
    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    .and_then(|j| {
        j["SPDisplaysDataType"][0]["_name"]
            .as_str()
            .map(|s| s.to_string())
    })
    .unwrap_or_else(|| "Unknown".to_string());

    let stdout = crate::cmd::run(
        "/usr/sbin/ioreg",
        &["-l", "-w", "0"],
        crate::cmd::DEFAULT_TIMEOUT,
    )?
    .to_result()?;

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

/// Parse `iostat -d -I` output into per-device cumulative MB. Pure function
/// (unit-testable with fixtures; no hardware, no shell). The MB column is the
/// cumulative total since boot, used for real differential windows.
///
/// Format (per device, columns KB/t, xfrs, MB):
/// ```text
///               disk0               disk1
///     KB/t xfrs   MB     KB/t xfrs   MB
///     6.89 75957 510.89  5.00 100    20.5
/// ```
fn parse_iostat_cumulative_mb(stdout: &str) -> std::collections::HashMap<String, f64> {
    let mut map = std::collections::HashMap::new();
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() < 3 {
        return map;
    }
    // Header line 0 holds the device names (leading spaces, one per device).
    let names: Vec<&str> = lines[0].split_whitespace().collect();
    // The data line is the last non-empty line; each device contributes 3 cols
    // (KB/t, xfrs, MB) — MB is at base+2.
    let data: Option<&str> = lines.iter().rev().find(|l| !l.trim().is_empty()).copied();
    let data = match data {
        Some(d) => d,
        None => return map,
    };
    let cols: Vec<&str> = data.split_whitespace().collect();
    for (i, name) in names.iter().enumerate() {
        let idx = i * 3 + 2;
        if idx < cols.len() {
            if let Ok(mb) = cols[idx].parse::<f64>() {
                map.insert(name.to_string(), mb);
            }
        }
    }
    map
}

/// Record one snapshot's metrics to history with the snapshot's own
/// observation timestamp, not a fresh per-row timestamp (F11).
///
/// R2/A05: the whole snapshot is one atomic batch write (single transaction).
/// A failure is surfaced via history::health() instead of silently dropping
/// rows one by one with `let _ =`.
pub fn record_snapshot(info: &SystemInfo) {
    let ts = info.observed_at;
    // One row per metric; roughly 3 + cores + disks + disk-temps.
    let mut rows: Vec<(&str, &str, f64, &str)> = Vec::with_capacity(
        3 + info.cpu.per_core_usage.len() + info.disk_throughput.len() + info.disks.len(),
    );
    rows.push((
        "cpu.total_usage",
        "system",
        info.cpu.total_usage as f64,
        "%",
    ));
    rows.push((
        "memory.used_percent",
        "system",
        info.memory.used_percent as f64,
        "%",
    ));
    rows.push(("gpu.utilization", "gpu0", info.gpu.utilization as f64, "%"));
    // per-core object ids must outlive the call — build owned strings.
    let core_ids: Vec<String> = (0..info.cpu.per_core_usage.len())
        .map(|i| format!("core{}", i))
        .collect();
    for (i, usage) in info.cpu.per_core_usage.iter().enumerate() {
        rows.push(("cpu.per_core", core_ids[i].as_str(), *usage as f64, "%"));
    }
    for disk in &info.disk_throughput {
        // R7/A10: history keyed by anonymous device_uid, falling back to the
        // volatile address only if no uid was assigned (degraded path).
        let oid = if disk.device_uid.is_empty() {
            disk.device.as_str()
        } else {
            disk.device_uid.as_str()
        };
        rows.push(("disk.throughput", oid, disk.mb_per_sec as f64, "MB/s"));
    }
    // Disk temperature into history (only real readings; absent stays absent).
    for disk in &info.disks {
        if let Some(t) = disk.temperature_celsius {
            let oid = if disk.device_uid.is_empty() {
                disk.device.as_str()
            } else {
                disk.device_uid.as_str()
            };
            rows.push(("disk.temperature", oid, t as f64, "°C"));
        }
    }
    // Surface the error via health(); do not panic the sampler on a full disk.
    let _ = history::record_snapshot_batch(ts, &rows);
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
///
/// R5/A04: scheduling is anchored to the next absolute deadline, not
/// "sleep(interval) + work". A slow pass shortens the following sleep so the
/// cadence stays close to the configured interval and slow sources never
/// accumulate drift; an overrun simply means the next tick starts on time.
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

        let tick_start = Instant::now();

        // Run the sampling pass. The MutexGuard must NOT be held across an
        // await (it is not Send), so we scope it to a plain sync block that
        // returns the snapshot; the guard is dropped before any sleep.
        let sampled: Option<Option<SystemInfo>> = {
            match SAMPLER.lock() {
                Ok(mut guard) => Some(match guard.as_mut() {
                    Some(s) => {
                        crate::metric_status::note_attempt();
                        match s.sample() {
                            Ok(info) => {
                                crate::metric_status::note_success();
                                Some(info)
                            }
                            Err(e) => {
                                crate::metric_status::note_failure();
                                log::warn!("sample pass failed: {}", e);
                                None
                            }
                        }
                    }
                    None => None,
                }),
                Err(_) => None, // poisoned — skip this tick (sleep below)
            }
        };
        let lock_ok = sampled.is_some();
        let snapshot = sampled.flatten();

        if let Some(info) = snapshot {
            record_snapshot(&info);
            if let Ok(mut latest) = LATEST.lock() {
                *latest = Some(info);
            }
        }

        // Sleep only the remaining time until the next deadline; if the pass
        // overran (or the lock was poisoned so we did no work), start the next
        // tick immediately / after the plain interval respectively.
        let elapsed = tick_start.elapsed();
        let remaining = if !lock_ok {
            interval
        } else if elapsed < interval {
            interval - elapsed
        } else {
            Duration::ZERO
        };
        if remaining > Duration::ZERO {
            tokio::time::sleep(remaining).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_iostat_cumulative_mb;

    const TWO_DISKS: &str = "              disk0               disk1 \n    KB/t xfrs   MB     KB/t xfrs   MB \n    6.89 75957 510.89  5.00 100    20.5 \n";

    #[test]
    fn parses_cumulative_mb_per_device() {
        let m = parse_iostat_cumulative_mb(TWO_DISKS);
        assert_eq!(m.get("disk0"), Some(&510.89));
        assert_eq!(m.get("disk1"), Some(&20.5));
    }

    #[test]
    fn handles_single_disk_and_missing_columns() {
        let one = "              disk0 \n    KB/t xfrs   MB \n    6.89 75957 510.89 \n";
        let m = parse_iostat_cumulative_mb(one);
        assert_eq!(m.get("disk0"), Some(&510.89));
        assert_eq!(m.len(), 1);
        // Empty / malformed input yields an empty map, never a panic.
        assert!(parse_iostat_cumulative_mb("").is_empty());
        assert!(parse_iostat_cumulative_mb("garbage\n").is_empty());
    }
}
