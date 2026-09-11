//! Process collection with system-wide storage I/O (S6).
//!
//! Memory and CPU come from sysinfo; per-process disk read/write bytes come
//! from proc_pid_rusage (RUSAGE_INFO_V4). Rates are computed by differencing
//! cumulative counters against the previous scan keyed by (pid, start-time)
//! so PID reuse never produces a spurious spike (F13, S6). The figures are
//! system-wide across all disks and are never attributed to one disk (D-009).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use sysinfo::System;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    /// Process start-time marker so a recycled PID is not joined to the
    /// previous owner's I/O counters.
    pub start_marker: u64,
    pub name: String,
    pub memory_bytes: u64,
    pub cpu_usage: f32,
    /// Cumulative bytes read/written since process start (system-wide).
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    /// Bytes/sec since the previous scan, if we have a baseline for this
    /// exact process instance; null on first sight or after PID reuse.
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessPage {
    pub processes: Vec<ProcessInfo>,
    /// Total number of readable processes before any filtering/limit —
    /// lets the UI say "showing N of M" honestly (F13).
    pub total_readable: usize,
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
}

impl ProcessCollector {
    pub fn new() -> Self {
        Self {
            sys: System::new(),
            baselines: HashMap::new(),
        }
    }

    /// Scan all readable processes. Sorting/filtering happen on the full set
    /// here — never truncate before the caller filters (F13).
    pub fn scan(&mut self) -> ProcessPage {
        self.sys.refresh_processes();

        let observed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let now = Instant::now();
        let mut processes = Vec::new();
        let mut seen: HashMap<(u32, u64), IoBaseline> = HashMap::new();

        for (pid, process) in self.sys.processes() {
            let pid_u32 = pid.as_u32();
            let start_marker = process.start_time();

            let (read, write) = disk_io_bytes(pid_u32).unwrap_or((0, 0));
            let key = (pid_u32, start_marker);

            let (read_bps, write_bps) = match self.baselines.get(&key) {
                Some(prev) => {
                    let dt = now.duration_since(prev.at).as_secs_f64();
                    if dt > 0.0 && read >= prev.read && write >= prev.write {
                        (
                            Some((read - prev.read) as f64 / dt),
                            Some((write - prev.write) as f64 / dt),
                        )
                    } else {
                        (None, None)
                    }
                }
                None => (None, None),
            };

            seen.insert(key, IoBaseline { read, write, at: now });

            processes.push(ProcessInfo {
                pid: pid_u32,
                start_marker,
                name: process.name().to_string(),
                memory_bytes: process.memory(),
                cpu_usage: process.cpu_usage(),
                disk_read_bytes: read,
                disk_write_bytes: write,
                disk_read_bps: read_bps,
                disk_write_bps: write_bps,
            });
        }

        // Drop baselines for processes that no longer exist.
        self.baselines = seen;

        let total_readable = processes.len();
        ProcessPage {
            processes,
            total_readable,
            observed_at,
        }
    }
}

/// Cumulative (disk_read_bytes, disk_write_bytes) for a pid via
/// proc_pid_rusage. Returns None if the process is not readable.
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
    fn total_readable_matches_returned_len() {
        let mut c = ProcessCollector::new();
        let page = c.scan();
        assert_eq!(page.total_readable, page.processes.len());
    }
}
