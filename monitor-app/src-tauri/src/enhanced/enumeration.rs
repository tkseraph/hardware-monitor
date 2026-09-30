//! Explicit normal-permission baseline candidate; never claims race-free system coverage.
use super::{
    clock_bridge::HostClock,
    disk_io::{Limits, ProcessKey},
    identity::{EventTime, PinnedProcess, PinnedThread},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io, mem,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
    System::Diagnostics::ToolHelp::*,
};
#[derive(Default, Serialize)]
pub struct Coverage {
    pub processes_seen: usize,
    pub processes_verified: usize,
    pub processes_unavailable: usize,
    pub idle_excluded: usize,
    pub threads_seen: usize,
    pub threads_verified: usize,
    pub threads_unavailable: usize,
    pub lifecycle_handoff_verified: bool,
}
pub struct Candidate {
    pub processes: Vec<PinnedProcess>,
    pub threads: Vec<PinnedThread>,
    pub coverage: Coverage,
    pub start_ns: u64,
    pub end_ns: u64,
    pub lifetime_range: EventTime,
}
fn bounded(count: usize, limit: usize, started: Instant) -> io::Result<()> {
    if count >= limit {
        return Err(io::Error::other("enumeration capacity exceeded"));
    }
    if started.elapsed() > Duration::from_secs(4) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "enumeration deadline exceeded",
        ));
    }
    Ok(())
}
fn ended() -> io::Result<()> {
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
        Ok(())
    } else {
        Err(e)
    }
}
pub fn capture(clock: &mut HostClock, limits: Limits) -> io::Result<Candidate> {
    clock.confirm()?;
    let start_ns = clock.now_ns()?;
    let start_range = clock.range(start_ns)?;
    let started = Instant::now();
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS | TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut coverage = Coverage::default();
    let mut processes = Vec::new();
    let mut threads = Vec::new();
    let mut p = PROCESSENTRY32W {
        dwSize: mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut present = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut p) } != 0;
    while present {
        bounded(coverage.processes_seen, limits.processes, started)?;
        coverage.processes_seen += 1;
        if p.th32ProcessID == 0 {
            coverage.idle_excluded += 1;
        } else {
            match PinnedProcess::open(p.th32ProcessID) {
                Ok(p) => processes.push(p),
                Err(_) => coverage.processes_unavailable += 1,
            }
        }
        present = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut p) } != 0;
    }
    ended()?;
    let mut t = THREADENTRY32 {
        dwSize: mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut present = unsafe { Thread32First(snapshot.as_raw_handle(), &mut t) } != 0;
    while present {
        bounded(coverage.threads_seen, limits.threads, started)?;
        coverage.threads_seen += 1;
        match PinnedThread::open(t.th32ThreadID) {
            Ok(thread) => threads.push((t.th32ThreadID, t.th32OwnerProcessID, thread)),
            Err(_) => coverage.threads_unavailable += 1,
        }
        t.dwSize = mem::size_of::<THREADENTRY32>() as u32;
        present = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut t) } != 0;
    }
    ended()?;
    clock.confirm()?;
    let end_ns = clock.now_ns()?;
    let end_range = clock.range(end_ns)?;
    let lifetime_range = EventTime {
        earliest_filetime: start_range.earliest_filetime,
        latest_filetime: end_range.latest_filetime,
    };
    let mut keys: BTreeMap<u32, ProcessKey> = BTreeMap::new();
    let mut verified_processes = Vec::new();
    for process in processes {
        match process.key_at(lifetime_range) {
            Ok(Some(key)) if !keys.contains_key(&key.pid) => {
                keys.insert(key.pid, key);
                verified_processes.push(process);
            }
            _ => coverage.processes_unavailable += 1,
        }
    }
    let mut tids = BTreeSet::new();
    let mut verified_threads = Vec::new();
    for (expected_tid, expected_pid, thread) in threads {
        match thread.identity_at(lifetime_range) {
            Ok(Some((tid, key)))
                if tid == expected_tid
                    && key.pid == expected_pid
                    && keys.get(&key.pid) == Some(&key)
                    && tids.insert(tid) =>
            {
                verified_threads.push(thread)
            }
            _ => coverage.threads_unavailable += 1,
        }
    }
    if started.elapsed() > Duration::from_secs(4) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "enumeration validation deadline exceeded",
        ));
    }
    coverage.processes_verified = verified_processes.len();
    coverage.threads_verified = verified_threads.len();
    Ok(Candidate {
        processes: verified_processes,
        threads: verified_threads,
        coverage,
        start_ns,
        end_ns,
        lifetime_range,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_deadline_are_failures_not_truncated_success() {
        assert!(bounded(1, 1, Instant::now()).is_err());
        assert!(bounded(0, 1, Instant::now() - Duration::from_secs(5)).is_err());
        assert!(bounded(0, 1, Instant::now()).is_ok());
    }
}
