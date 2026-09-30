//! Atomic installation of a VERIFIED SCOPE, not automatic system enumeration.
//! Caller must establish enumeration coverage and a valid FILETIME/QPC correspondence.
use super::{
    disk_io::{Accumulator, Limits},
    event_order::OrderedEvents,
    identity::{EventTime, PinnedProcess, PinnedThread},
};
use std::{collections::BTreeSet, io};
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "incomplete or ambiguous baseline",
    )
}
pub fn install(
    at_ns: u64,
    time: EventTime,
    processes: &[PinnedProcess],
    threads: &[PinnedThread],
    limits: Limits,
    accumulator: &mut Accumulator,
    queue: &mut OrderedEvents,
) -> io::Result<()> {
    // Invalidate old rates first; verification failure cannot leave apparently current data.
    accumulator.lost(0);
    queue.reset();
    if processes.len() > limits.processes
        || threads.len() > limits.threads
        || processes.len() > limits.rows
    {
        return Err(invalid());
    }
    let mut process_keys = Vec::new();
    let mut pids = BTreeSet::new();
    for process in processes {
        let key = process.key_at(time)?.ok_or_else(invalid)?;
        if !pids.insert(key.pid) {
            return Err(invalid());
        }
        process_keys.push(key);
    }
    let mut mappings = Vec::new();
    let mut tids = BTreeSet::new();
    for thread in threads {
        let (tid, key) = thread.identity_at(time)?.ok_or_else(invalid)?;
        if !process_keys.contains(&key) || !tids.insert(tid) {
            return Err(invalid());
        }
        mappings.push((tid, key));
    }
    let mut next = Accumulator::new(at_ns, limits);
    for key in process_keys {
        if !next.process_start(at_ns, key) {
            return Err(invalid());
        }
    }
    for (tid, key) in mappings {
        if !next.thread_start(at_ns, tid, key) {
            return Err(invalid());
        }
    }
    if !next.baseline_complete(at_ns) {
        return Err(invalid());
    }
    queue.drain_through(at_ns).map_err(|_| invalid())?;
    *accumulator = next;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::enhanced::{
        disk_io::Direction,
        etw_decode::{Body, Event},
    };
    fn now() -> EventTime {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            / 100
            + 116444736000000000;
        EventTime {
            earliest_filetime: t as u64,
            latest_filetime: t as u64,
        }
    }
    #[test]
    fn verified_scope_handoff_and_old_events_are_rejected() {
        let process = PinnedProcess::open(std::process::id()).unwrap();
        let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let thread = PinnedThread::open(tid).unwrap();
        let mut a = Accumulator::new(0, Limits::default());
        let mut q = OrderedEvents::new(10);
        install(
            10,
            now(),
            &[process],
            &[thread],
            Limits::default(),
            &mut a,
            &mut q,
        )
        .unwrap();
        assert!(a.begin(11, 1, tid, Direction::Read));
        a.complete(12, 1, Direction::Read, 100);
        assert_eq!(a.window(1_000_000_010).rows[0].read_bps, Some(100.0));
        assert!(q
            .push(Event {
                at_ns: 9,
                body: Body::Complete {
                    request: 2,
                    bytes: 20,
                    direction: Direction::Read
                }
            })
            .is_err());
    }
    #[test]
    fn ambiguous_baseline_does_not_leave_old_rates_valid() {
        let p1 = PinnedProcess::open(std::process::id()).unwrap();
        let p2 = PinnedProcess::open(std::process::id()).unwrap();
        let mut a = Accumulator::new(0, Limits::default());
        let mut q = OrderedEvents::new(10);
        assert!(install(0, now(), &[p1, p2], &[], Limits::default(), &mut a, &mut q).is_err());
        assert!(!a.window(1_000_000_000).ready);
    }
}
