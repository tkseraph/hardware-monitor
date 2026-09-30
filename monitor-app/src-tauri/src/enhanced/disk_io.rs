//! Bounded attribution of NORMALIZED storage events. No OS reads, no UI or history writes.
//! The native adapter must provide verified process creation markers, ordered event time,
//! and matching disk initiation/completion events. Completion-header PID is never used.
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessKey {
    pub pid: u32,
    pub creation: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Read,
    Write,
}
#[derive(Clone, Copy)]
struct Process {
    key: ProcessKey,
    since: u64,
}
#[derive(Clone, Copy)]
struct Pending {
    started_ns: u64,
    owner: ProcessKey,
    direction: Direction,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct Quality {
    pub unattributed_operations: u64,
    pub unattributed_bytes: u64,
    pub lost_events: u64,
    pub rejected_events: u64,
    pub expired_operations: u64,
}
impl Quality {
    fn complete(&self) -> bool {
        self.unattributed_operations == 0
            && self.lost_events == 0
            && self.rejected_events == 0
            && self.expired_operations == 0
    }
}
#[derive(Debug)]
pub struct Rate {
    pub process: ProcessKey,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
}
#[derive(Debug)]
pub struct Window {
    pub start_ns: u64,
    pub end_ns: u64,
    pub ready: bool,
    pub quality: Quality,
    pub rows: Vec<Rate>,
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub processes: usize,
    pub threads: usize,
    pub pending: usize,
    pub rows: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            processes: 4096,
            threads: 65536,
            pending: 16384,
            rows: 4096,
        }
    }
}
pub struct Accumulator {
    limits: Limits,
    processes: BTreeMap<u32, Process>,
    threads: BTreeMap<u32, ProcessKey>,
    pending: BTreeMap<u64, Pending>,
    counts: BTreeMap<ProcessKey, (u64, u64)>,
    quality: Quality,
    ready: bool,
    baseline_allowed: bool,
    start: u64,
    last: u64,
}
impl Accumulator {
    pub fn new(now_ns: u64, limits: Limits) -> Self {
        Self {
            limits,
            processes: BTreeMap::new(),
            threads: BTreeMap::new(),
            pending: BTreeMap::new(),
            counts: BTreeMap::new(),
            quality: Quality::default(),
            ready: false,
            baseline_allowed: true,
            start: now_ns,
            last: now_ns,
        }
    }
    fn reject(&mut self) {
        self.ready = false;
        self.baseline_allowed = false;
        self.quality.rejected_events = self.quality.rejected_events.saturating_add(1);
    }
    fn time(&mut self, at: u64) -> bool {
        if at < self.last {
            self.reject();
            false
        } else {
            self.last = at;
            true
        }
    }
    /// Clears ambiguous identity state; native adapter must establish a fresh baseline.
    pub fn reset(&mut self, at: u64) {
        self.processes.clear();
        self.threads.clear();
        self.pending.clear();
        self.counts.clear();
        self.quality = Quality::default();
        self.ready = false;
        self.baseline_allowed = true;
        self.start = at;
        self.last = at;
    }
    /// Called only after the adapter has completed a verified process/thread baseline.
    pub fn baseline_complete(&mut self, at: u64) -> bool {
        if !self.time(at) || !self.baseline_allowed || !self.quality.complete() {
            return false;
        }
        self.baseline_allowed = false;
        self.start = at;
        self.counts.clear();
        self.quality = Quality::default();
        self.ready = true;
        true
    }
    pub fn process_start(&mut self, at: u64, key: ProcessKey) -> bool {
        if !self.time(at) {
            return false;
        }
        if key.pid == 0 || key.creation == 0 {
            self.reject();
            return false;
        }
        if let Some(old) = self.processes.get(&key.pid) {
            if old.key == key {
                return true;
            }
            let old = old.key;
            self.threads.retain(|_, owner| *owner != old);
        } else if self.processes.len() >= self.limits.processes {
            self.reject();
            return false;
        }
        self.processes.insert(key.pid, Process { key, since: at });
        true
    }
    pub fn process_end(&mut self, at: u64, key: ProcessKey) {
        if !self.time(at) {
            return;
        }
        if self.processes.get(&key.pid).is_some_and(|p| p.key == key) {
            self.processes.remove(&key.pid);
            self.threads.retain(|_, p| *p != key);
        }
        // Already-initiated IO retains its original owner even after PID reuse.
    }
    pub fn thread_start(&mut self, at: u64, tid: u32, key: ProcessKey) -> bool {
        if !self.time(at) {
            return false;
        }
        if tid == 0 || !self.processes.get(&key.pid).is_some_and(|p| p.key == key) {
            self.reject();
            return false;
        }
        if !self.threads.contains_key(&tid) && self.threads.len() >= self.limits.threads {
            self.reject();
            return false;
        }
        self.threads.insert(tid, key);
        true
    }
    pub fn thread_end(&mut self, at: u64, tid: u32, key: ProcessKey) {
        if self.time(at) && self.threads.get(&tid) == Some(&key) {
            self.threads.remove(&tid);
        }
    }
    pub fn begin(&mut self, at: u64, request: u64, tid: u32, direction: Direction) -> bool {
        if !self.time(at) {
            return false;
        }
        if !self.ready || request == 0 {
            self.reject();
            return false;
        }
        if self.pending.contains_key(&request) {
            // Reuse while outstanding makes correlation ambiguous. Invalidate the baseline;
            // neither the old nor new operation may be attributed by guessing.
            self.lost(0);
            self.reject();
            return false;
        }
        let Some(&owner) = self.threads.get(&tid) else {
            return false;
        };
        if self.pending.len() >= self.limits.pending {
            self.reject();
            return false;
        }
        self.pending.insert(
            request,
            Pending {
                started_ns: at,
                owner,
                direction,
            },
        );
        true
    }
    pub fn complete(&mut self, at: u64, request: u64, direction: Direction, bytes: u32) {
        if !self.time(at) {
            return;
        }
        let pending = self.pending.remove(&request);
        if !self.ready || !pending.is_some_and(|p| p.direction == direction) {
            self.quality.unattributed_operations =
                self.quality.unattributed_operations.saturating_add(1);
            self.quality.unattributed_bytes = self
                .quality
                .unattributed_bytes
                .saturating_add(u64::from(bytes));
            return;
        }
        let owner = pending.unwrap().owner;
        if !self.counts.contains_key(&owner) && self.counts.len() >= self.limits.rows {
            self.reject();
            return;
        }
        let entry = self.counts.entry(owner).or_default();
        let value = match direction {
            Direction::Read => &mut entry.0,
            Direction::Write => &mut entry.1,
        };
        if let Some(next) = value.checked_add(u64::from(bytes)) {
            *value = next;
        } else {
            self.reject();
        }
    }
    pub fn lost(&mut self, count: u64) {
        self.baseline_allowed = false;
        self.quality.lost_events = self.quality.lost_events.saturating_add(count);
        self.ready = false;
        self.processes.clear();
        self.threads.clear();
        self.pending.clear();
    }
    /// Windows are (start, end], and rates use actual monotonic elapsed time.
    /// A pause >15 s needs an explicit new baseline; old mappings cannot leak across it.
    pub fn window(&mut self, end: u64) -> Window {
        let start = self.start;
        let elapsed = end.checked_sub(start);
        let valid = elapsed.is_some_and(|n| (100_000_000..=15_000_000_000).contains(&n))
            && end >= self.last;
        if !valid {
            let quality = self.quality;
            self.reset(end);
            return Window {
                start_ns: start,
                end_ns: end,
                ready: false,
                quality,
                rows: vec![],
            };
        }
        let before = self.pending.len();
        self.pending
            .retain(|_, operation| end.saturating_sub(operation.started_ns) <= 60_000_000_000);
        let expired = before - self.pending.len();
        if expired > 0 {
            self.quality.expired_operations = self
                .quality
                .expired_operations
                .saturating_add(expired as u64);
            self.ready = false;
            self.baseline_allowed = false;
        }
        let mut counts = std::mem::take(&mut self.counts);
        // Only rows present for the entire window may claim a measured zero.
        for process in self.processes.values() {
            if !counts.contains_key(&process.key) && counts.len() >= self.limits.rows {
                self.reject();
                break;
            }
            counts.entry(process.key).or_default();
        }
        let ready = self.ready && self.quality.complete();
        let rows = counts
            .into_iter()
            .map(|(key, (read, write))| {
                let full_window = self
                    .processes
                    .get(&key.pid)
                    .is_some_and(|p| p.key == key && p.since <= start);
                let valid = ready && full_window;
                let seconds = elapsed.unwrap() as f64 / 1e9;
                Rate {
                    process: key,
                    read_bytes: read,
                    write_bytes: write,
                    read_bps: valid.then_some(read as f64 / seconds),
                    write_bps: valid.then_some(write as f64 / seconds),
                }
            })
            .collect();
        let quality = std::mem::take(&mut self.quality);
        self.start = end;
        self.last = end;
        Window {
            start_ns: start,
            end_ns: end,
            ready,
            quality,
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const S: u64 = 1_000_000_000;
    fn key(pid: u32, creation: u64) -> ProcessKey {
        ProcessKey { pid, creation }
    }
    fn ready() -> Accumulator {
        let mut a = Accumulator::new(0, Limits::default());
        a.process_start(0, key(10, 1));
        a.thread_start(0, 20, key(10, 1));
        a.baseline_complete(0);
        a
    }
    #[test]
    fn exact_read_write_bytes_use_actual_window_and_idle_zero() {
        let mut a = ready();
        a.begin(1, 30, 20, Direction::Read);
        a.complete(2, 30, Direction::Read, 4096);
        a.begin(3, 31, 20, Direction::Write);
        a.complete(4, 31, Direction::Write, 2048);
        let w = a.window(2 * S);
        assert!(w.ready);
        assert_eq!(w.rows[0].read_bps, Some(2048.0));
        assert_eq!(w.rows[0].write_bps, Some(1024.0));
        let idle = a.window(3 * S);
        assert_eq!(idle.rows[0].read_bps, Some(0.0));
    }
    #[test]
    fn pending_io_survives_pid_and_tid_reuse_without_changing_owner() {
        let mut a = ready();
        a.begin(1, 30, 20, Direction::Read);
        a.process_end(2, key(10, 1));
        a.process_start(3, key(10, 2));
        a.thread_start(4, 20, key(10, 2));
        a.complete(5, 30, Direction::Read, 900);
        let w = a.window(S);
        assert_eq!(
            w.rows
                .iter()
                .find(|r| r.process == key(10, 1))
                .unwrap()
                .read_bytes,
            900
        );
        let new = w.rows.iter().find(|r| r.process == key(10, 2)).unwrap();
        assert_eq!(new.read_bytes, 0);
        assert!(new.read_bps.is_none());
    }
    #[test]
    fn missing_init_wrong_direction_and_loss_never_become_valid_zero() {
        let mut a = ready();
        a.complete(1, 55, Direction::Read, 42);
        let w = a.window(S);
        assert!(!w.ready);
        assert_eq!(w.quality.unattributed_bytes, 42);
        assert!(w.rows[0].read_bps.is_none());
        a.begin(S + 1, 56, 20, Direction::Read);
        a.complete(S + 2, 56, Direction::Write, 12);
        assert!(!a.window(2 * S).ready);
        a.lost(3);
        assert!(!a.begin(2 * S + 1, 60, 20, Direction::Read));
        assert!(!a.window(3 * S).ready);
    }
    #[test]
    fn duplicate_request_and_long_pause_require_fresh_baseline() {
        let mut a = ready();
        assert!(a.begin(1, 30, 20, Direction::Read));
        assert!(!a.begin(2, 30, 20, Direction::Read));
        a.complete(3, 30, Direction::Read, 99);
        assert!(!a.window(S).ready);
        let mut a = ready();
        assert!(!a.window(20 * S).ready);
        assert!(!a.begin(20 * S + 1, 30, 20, Direction::Read));
    }
    #[test]
    fn lost_stream_cannot_recover_by_merely_advancing_the_window() {
        let mut a = ready();
        a.lost(1);
        assert!(!a.window(S).ready);
        assert!(!a.baseline_complete(S + 1));
        a.reset(2 * S);
        assert!(a.process_start(2 * S, key(10, 2)));
        assert!(a.thread_start(2 * S, 20, key(10, 2)));
        assert!(a.baseline_complete(2 * S));
        assert_eq!(a.window(3 * S).rows[0].read_bps, Some(0.0));
    }

    #[test]
    fn completion_in_a_later_window_preserves_owner_and_uses_completion_window() {
        let mut a = ready();
        assert!(a.begin(S / 2, 123, 20, Direction::Read));
        assert_eq!(a.window(S).rows[0].read_bps, Some(0.0));
        a.complete(S + S / 2, 123, Direction::Read, 8192);
        let completed = a.window(3 * S);
        assert!(completed.ready);
        assert_eq!(completed.rows[0].read_bytes, 8192);
        assert_eq!(completed.rows[0].read_bps, Some(4096.0));
    }
    #[test]
    fn missing_completions_expire_without_inventing_rates() {
        let mut a = ready();
        a.begin(1, 123, 20, Direction::Read);
        for second in 1..=60 {
            assert!(a.window(second * S).ready);
        }
        let expired = a.window(61 * S);
        assert!(!expired.ready);
        assert_eq!(expired.quality.expired_operations, 1);
        assert!(expired.rows.iter().all(|r| r.read_bps.is_none()));
        assert!(a.pending.is_empty());
        a.complete(61 * S + 1, 123, Direction::Read, 8192);
        let late = a.window(62 * S);
        assert!(!late.ready);
        assert_eq!(late.quality.unattributed_bytes, 8192);
        assert!(!a.baseline_complete(62 * S + 1));
    }

    #[test]
    fn bounds_and_out_of_order_events_are_visible() {
        let mut a = Accumulator::new(
            0,
            Limits {
                processes: 1,
                threads: 1,
                pending: 1,
                rows: 1,
            },
        );
        assert!(a.process_start(0, key(1, 1)));
        assert!(!a.process_start(1, key(2, 2)));
        assert!(a.thread_start(2, 1, key(1, 1)));
        assert!(!a.thread_start(3, 2, key(1, 1)));
        assert!(!a.baseline_complete(4));
        a.reset(4);
        assert!(a.process_start(4, key(1, 1)));
        assert!(a.thread_start(4, 1, key(1, 1)));
        assert!(a.baseline_complete(4));
        assert!(a.begin(5, 1, 1, Direction::Read));
        assert!(!a.begin(6, 2, 1, Direction::Read));
        a.complete(7, 1, Direction::Read, 10);
        a.complete(6, 3, Direction::Read, 10);
        let w = a.window(S);
        assert!(!w.ready);
        assert_eq!(w.quality.rejected_events, 2);
    }
}
