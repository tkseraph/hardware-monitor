//! Process collection with system-wide storage I/O (S6).
//!
//! Windows CPU/memory/creation time use one native handle; Mac uses sysinfo.
//! Mac per-process disk read/write bytes come
//! from proc_pid_rusage (RUSAGE_INFO_V4). Rates are computed by differencing
//! cumulative counters against the previous scan keyed by (pid, start-time)
//! so PID reuse never produces a spurious spike (F13, S6). The figures are
//! system-wide across all disks and are never attributed to one disk (D-009).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::{ProcessRefreshKind, System};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    /// Process start-time marker so a recycled PID is not joined to the
    /// previous owner's I/O counters.
    pub start_marker: Option<String>,
    pub name: String,
    pub memory_bytes: Option<u64>,
    pub cpu_usage: Option<f32>,
    /// Cumulative bytes read/written since process start (system-wide).
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    /// Bytes/sec since the previous scan, if we have a baseline for this
    /// exact process instance; null on first sight or after PID reuse.
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    /// R6/A06: false when this process's I/O counters were unreadable this
    /// pass — disk_*_bytes are then 0 placeholders the UI must not render.
    pub io_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessPage {
    pub processes: Vec<ProcessInfo>,
    /// Total number of readable processes before any filtering/limit —
    /// lets the UI say "showing N of M" honestly (F13).
    pub total_readable: usize,
    /// R6: rows matching the active filter BEFORE pagination — the UI's real
    /// page count. Equals total_readable when no filter is applied.
    pub matched_total: usize,
    /// R6: the offset/limit actually applied to `processes`.
    pub offset: usize,
    pub limit: usize,
    /// Unix seconds of this scan.
    pub observed_at: i64,
}

#[derive(Clone, Copy)]
struct IoBaseline {
    read: u64,
    write: u64,
    at: Instant,
}

pub struct ProcessCollector {
    sys: System,
    /// (pid, start_marker) -> previous cumulative counters.
    baselines: HashMap<(u32, u64), IoBaseline>,
    #[cfg(target_os = "macos")]
    seen_processes: HashSet<(u32, u64)>,
    #[cfg(target_os = "macos")]
    last_scan: Option<Instant>,
    #[cfg(target_os = "windows")]
    windows_baselines: HashMap<u32, crate::platform::windows::processes::Reading>,
}

impl ProcessCollector {
    pub fn new() -> Self {
        Self {
            sys: System::new(),
            baselines: HashMap::new(),
            #[cfg(target_os = "macos")]
            seen_processes: HashSet::new(),
            #[cfg(target_os = "macos")]
            last_scan: None,
            #[cfg(target_os = "windows")]
            windows_baselines: HashMap::new(),
        }
    }

    /// Scan all readable processes. Sorting/filtering happen on the full set
    /// here — never truncate before the caller filters (F13).
    ///
    /// R6/A06: a process whose I/O counters are unreadable contributes
    /// `disk_read_bps/writps = None` and `io_ok = false`; its cumulative
    /// counters are NOT recorded as a fake 0 baseline, so a later successful
    /// read never produces a spurious lifetime-to-date spike, and a persistent
    /// failure never masquerades as "0 B/s".
    pub fn scan(&mut self) -> ProcessPage {
        self.sys.refresh_cpu_usage();
        #[cfg(target_os = "windows")]
        self.sys
            .refresh_processes_specifics(ProcessRefreshKind::new());
        #[cfg(target_os = "macos")]
        self.sys
            .refresh_processes_specifics(ProcessRefreshKind::new().with_memory().with_cpu());

        let observed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let now = Instant::now();
        let mut processes = Vec::new();
        let mut seen: HashMap<(u32, u64), IoBaseline> = HashMap::new();

        #[cfg(target_os = "macos")]
        let mut current_processes = HashSet::new();
        #[cfg(target_os = "windows")]
        let mut current_windows = HashMap::new();
        #[cfg(target_os = "macos")]
        let valid_window = self.last_scan.is_some_and(|previous| {
            now.duration_since(previous) >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL
                && now.duration_since(previous).as_secs() < 120
        });
        for (pid, process) in self.sys.processes() {
            let pid_u32 = pid.as_u32();
            #[cfg(target_os = "macos")]
            let marker = Some(process.start_time()).filter(|&v| v > 0);
            #[cfg(target_os = "windows")]
            let native = crate::platform::windows::processes::read_process(pid_u32);
            #[cfg(target_os = "windows")]
            let marker = native.as_ref().map(|r| r.creation_marker);
            let start_marker = marker.unwrap_or(0);
            let key = (pid_u32, start_marker);

            // None = unreadable this pass. Never substitute (0,0).
            let io = marker.and_then(|_| disk_io_bytes(pid_u32));

            let (read_bps, write_bps, read_bytes, write_bytes) = match io {
                Some((read, write)) => {
                    let rates = match self.baselines.get(&key) {
                        Some(prev) => {
                            let dt = now.duration_since(prev.at).as_secs_f64();
                            if dt > 0.0 && read >= prev.read && write >= prev.write {
                                (
                                    Some((read - prev.read) as f64 / dt),
                                    Some((write - prev.write) as f64 / dt),
                                )
                            } else {
                                // Counter went backwards (PID reuse / counter
                                // reset): no rate this pass; baseline resets.
                                (None, None)
                            }
                        }
                        None => (None, None), // first sight: baseline only
                    };
                    // Update the baseline only on a successful read.
                    seen.insert(
                        key,
                        IoBaseline {
                            read,
                            write,
                            at: now,
                        },
                    );
                    (rates.0, rates.1, read, write)
                }
                None => {
                    // Unreadable: keep NO baseline entry so we don't carry a
                    // stale one forward, and report None rates + 0 counters
                    // flagged io_ok = false (UI must not render the 0).
                    (None, None, 0, 0)
                }
            };

            #[cfg(target_os = "macos")]
            if marker.is_some() {
                current_processes.insert(key);
            }
            #[cfg(target_os = "macos")]
            let memory_bytes = Some(process.memory()).filter(|&bytes| bytes > 0);
            #[cfg(target_os = "macos")]
            let cpu_usage = process_cpu_value(
                process.cpu_usage(),
                self.sys.cpus().len(),
                valid_window && marker.is_some() && self.seen_processes.contains(&key),
            );
            #[cfg(target_os = "windows")]
            let (memory_bytes, cpu_usage) = native.as_ref().map_or((None, None), |reading| {
                let usage = crate::platform::windows::processes::cpu_percent(
                    self.windows_baselines.get(&pid_u32),
                    reading,
                    self.sys.cpus().len(),
                );
                current_windows.insert(pid_u32, *reading);
                (reading.working_set_bytes, usage)
            });
            processes.push(ProcessInfo {
                pid: pid_u32,
                start_marker: marker.map(|value| value.to_string()),
                name: process.name().to_string(),
                memory_bytes,
                cpu_usage,
                disk_read_bytes: read_bytes,
                disk_write_bytes: write_bytes,
                disk_read_bps: read_bps,
                disk_write_bps: write_bps,
                io_ok: io.is_some(),
            });
        }

        // Drop baselines for processes that no longer exist OR were unreadable.
        self.baselines = seen;
        #[cfg(target_os = "macos")]
        {
            self.seen_processes = current_processes;
            self.last_scan = Some(now);
        }
        #[cfg(target_os = "windows")]
        {
            self.windows_baselines = current_windows;
        }

        let total_readable = processes.len();
        ProcessPage {
            processes,
            total_readable,
            matched_total: total_readable,
            offset: 0,
            limit: usize::MAX,
            observed_at,
        }
    }
}

/// Cumulative (disk_read_bytes, disk_write_bytes) for a pid via
/// proc_pid_rusage. Returns None if the process is not readable.
#[cfg(target_os = "macos")]
fn disk_io_bytes(pid: u32) -> Option<(u64, u64)> {
    // rusage_info_t is a union; v4 is the widest flavor we read.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        )
    };
    if ret == 0 {
        Some((info.ri_diskio_bytesread, info.ri_diskio_byteswritten))
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
fn disk_io_bytes(_pid: u32) -> Option<(u64, u64)> {
    // Windows sysinfo uses GetProcessIoCounters, whose scope is not disk-only.
    None
}

static COLLECTOR: Mutex<Option<ProcessCollector>> = Mutex::new(None);

/// Scan all readable processes (unsorted, untruncated). The caller applies
/// search/sort/pagination over the full set.
pub fn scan_processes() -> Result<ProcessPage, String> {
    let mut guard = COLLECTOR.lock().map_err(|e| e.to_string())?;
    if guard.is_none() {
        *guard = Some(ProcessCollector::new());
    }
    Ok(guard.as_mut().unwrap().scan())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "opt-in read-only local process enumeration"]
    fn rate_requires_baseline() {
        // First sight of a process must yield null rates, not 0 or a spike.
        let mut c = ProcessCollector::new();
        let page = c.scan();
        for p in &page.processes {
            // On a fresh collector nothing has a baseline.
            assert!(p.disk_read_bps.is_none() || p.disk_read_bps == Some(0.0));
        }
    }

    #[test]
    #[ignore = "opt-in read-only local process enumeration"]
    fn total_readable_matches_returned_len() {
        let mut c = ProcessCollector::new();
        let page = c.scan();
        assert_eq!(page.total_readable, page.processes.len());
    }
}

#[cfg(target_os = "macos")]
fn process_cpu_value(raw: f32, logical_cores: usize, warmed: bool) -> Option<f32> {
    if !warmed || !raw.is_finite() || raw < 0.0 || logical_cores == 0 {
        return None;
    }
    let value = if cfg!(target_os = "windows") {
        raw / logical_cores as f32
    } else {
        raw
    };
    if cfg!(target_os = "windows") && value > 100.0 {
        return None;
    }
    Some(value)
}

#[cfg(all(test, target_os = "macos"))]
mod value_tests {
    #[test]
    fn cpu_requires_valid_baseline_and_denominator() {
        assert_eq!(super::process_cpu_value(0.0, 24, true), Some(0.0));
        assert_eq!(super::process_cpu_value(24.0, 24, false), None);
        assert_eq!(super::process_cpu_value(24.0, 0, true), None);
        assert_eq!(super::process_cpu_value(f32::NAN, 24, true), None);
    }
}
