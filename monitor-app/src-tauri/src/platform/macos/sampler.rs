//! Existing macOS sources, isolated from Windows compilation.
use crate::model::*;
use crate::storage;
use std::time::Instant;
use sysinfo::{CpuRefreshKind, RefreshKind, System};

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
            physical_cores: (physical > 0).then_some(physical),
            logical_cores: logical,
            total_usage: total,
            per_core_usage: per_core,
        };

        let memory_result = sample_memory();
        let gpu_result = sample_gpu();
        let mut source_states = std::collections::BTreeMap::from([
            ("cpu".into(), SourceState::Ok),
            (
                "memory".into(),
                if memory_result.is_ok() {
                    SourceState::Ok
                } else {
                    SourceState::Error
                },
            ),
            (
                "gpu".into(),
                if gpu_result.is_ok() {
                    SourceState::Ok
                } else {
                    SourceState::Error
                },
            ),
        ]);
        let memory = memory_result.ok();
        let gpus = gpu_result.ok().into_iter().collect();
        // Build the storage topology once, then derive the flat disk list
        // and throughput device set from it so they can never disagree (F05).
        let storage_result = storage::build_topology();
        source_states.insert(
            "storage".into(),
            if storage_result.is_ok() {
                SourceState::Ok
            } else {
                SourceState::Error
            },
        );
        let mut storage = storage_result.unwrap_or_default();
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
        let throughput_result = self.sample_disk_throughput(&devices);
        source_states.insert(
            "disk_throughput".into(),
            if throughput_result.is_ok() {
                SourceState::Ok
            } else {
                SourceState::Error
            },
        );
        let mut disk_throughput = throughput_result.unwrap_or_default();
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
            cpu: Some(cpu),
            memory,
            gpus,
            source_states,
            source_meta: std::collections::BTreeMap::new(),
            windows_storage: None,
            windows_disk_throughput: None,
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
        // Query only GPU accelerator nodes. The full IORegistry can exceed
        // the command adapter's 4 MiB cap before PerformanceStatistics appears.
        &["-r", "-c", "IOAccelerator", "-l", "-w", "0"],
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
        object_id: "gpu0".into(),
        name,
        utilization,
        memory_used_bytes,
        memory_allocated_bytes: Some(memory_allocated_bytes),
        windows_memory: None,
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
#[cfg(test)]
mod tests {
    use super::parse_iostat_cumulative_mb;

    /// Explicit opt-in, read-only hardware smoke test; never writes history,
    /// registers login items, or starts the scheduler. Not run in CI.
    #[test]
    #[ignore = "requires local macOS hardware; explicit read-only diagnostic"]
    fn live_snapshot_collection_succeeds() {
        let mut sampler = super::Sampler::new();
        let snapshot = sampler
            .sample()
            .expect("read-only hardware collection failed");
        assert!(snapshot.memory.as_ref().unwrap().total_bytes > 0);
        assert!(snapshot.gpus[0].utilization.is_finite());
        assert!(snapshot.observed_at > 0);
    }

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
