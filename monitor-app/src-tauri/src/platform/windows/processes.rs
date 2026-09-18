use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Copy)]
pub struct Reading {
    pub creation_marker: u64,
    pub cpu_ticks: u64,
    pub working_set_bytes: Option<u64>,
    pub observed: Instant,
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// All resource fields refer to the process held by this single read-only handle.
/// Failure is unknown, never a fabricated zero. No image path or command line is queried.
pub fn read_process(pid: u32) -> Option<Reading> {
    unsafe {
        let raw = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if raw.is_null() {
            return None;
        }
        let handle = ProcessHandle(raw);
        let (mut created, mut exited, mut kernel, mut user): (
            FILETIME,
            FILETIME,
            FILETIME,
            FILETIME,
        ) = std::mem::zeroed();
        if GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
            return None;
        }
        let creation_marker = ticks(created);
        if creation_marker == 0 {
            return None;
        }
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mut memory: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        memory.cb = size;
        let memory_ok = K32GetProcessMemoryInfo(handle.0, &mut memory, size) != 0;
        Some(Reading {
            creation_marker,
            cpu_ticks: ticks(kernel).checked_add(ticks(user))?,
            working_set_bytes: memory_ok.then_some(memory.WorkingSetSize as u64),
            observed: Instant::now(),
        })
    }
}

pub fn cpu_percent(
    previous: Option<&Reading>,
    current: &Reading,
    logical_cores: usize,
) -> Option<f32> {
    let previous = previous?;
    if logical_cores == 0 || previous.creation_marker != current.creation_marker {
        return None;
    }
    let elapsed = current.observed.checked_duration_since(previous.observed)?;
    if elapsed < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL || elapsed >= Duration::from_secs(120) {
        return None;
    }
    let delta = current.cpu_ticks.checked_sub(previous.cpu_ticks)?;
    let value = delta as f64 / 10_000_000.0 / elapsed.as_secs_f64() / logical_cores as f64 * 100.0;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reading(at: Instant, marker: u64, cpu_ticks: u64) -> Reading {
        Reading {
            creation_marker: marker,
            cpu_ticks,
            working_set_bytes: Some(0),
            observed: at,
        }
    }
    #[test]
    fn cpu_uses_actual_window_and_whole_machine_capacity() {
        let now = Instant::now();
        let previous = reading(now, 1, 100);
        let current = reading(now + Duration::from_secs(2), 1, 20_000_100);
        assert_eq!(cpu_percent(Some(&previous), &current, 4), Some(25.0));
        let idle = reading(now + Duration::from_secs(2), 1, 100);
        assert_eq!(cpu_percent(Some(&previous), &idle, 4), Some(0.0));
    }
    #[test]
    fn first_sample_reuse_reset_and_invalid_windows_are_unknown() {
        let now = Instant::now();
        let previous = reading(now, 1, 100);
        for current in [
            reading(now, 1, 200),
            reading(now + Duration::from_secs(1), 2, 200),
            reading(now + Duration::from_secs(1), 1, 99),
            reading(now + Duration::from_secs(120), 1, 200),
        ] {
            assert!(cpu_percent(Some(&previous), &current, 4).is_none());
        }
        let current = reading(now + Duration::from_secs(1), 1, 200);
        assert!(cpu_percent(None, &current, 4).is_none());
        assert!(cpu_percent(Some(&previous), &current, 0).is_none());
    }
    #[test]
    #[ignore = "opt-in read-only current-process native metrics; no process termination"]
    fn live_current_process_metrics() {
        // Other native tests can consume several cores in this same test process.
        let system = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::new().with_cpu(sysinfo::CpuRefreshKind::everything()),
        );
        let logical_cores = system.cpus().len();
        assert!(logical_cores > 0);
        let first = read_process(std::process::id()).expect("current process is readable");
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL + Duration::from_millis(50));
        let second = read_process(std::process::id()).unwrap();
        assert_eq!(first.creation_marker, second.creation_marker);
        assert!(second.working_set_bytes.unwrap() > 0);
        assert!(cpu_percent(Some(&first), &second, logical_cores).is_some());
    }

    #[test]
    #[ignore = "opt-in read-only process-table integration; checks only own process values"]
    fn live_process_table_native_fields() {
        let mut collector = crate::processes::ProcessCollector::new();
        let first = collector.scan();
        let own = first
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        assert!(own.memory_bytes.unwrap() > 0);
        assert!(own.cpu_usage.is_none());
        assert!(!own.io_ok);
        let marker = own.start_marker.clone();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL + Duration::from_millis(50));
        let second = collector.scan();
        let own = second
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        assert_eq!(own.start_marker, marker);
        assert!(own.cpu_usage.is_some());
        assert!(own.disk_read_bps.is_none());
    }
}
