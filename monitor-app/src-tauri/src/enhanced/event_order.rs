//! Bounded chronological handoff; late events invalidate rather than rewrite published rates.
use super::etw_decode::Event;
use std::collections::BTreeMap;
#[derive(Debug, PartialEq, Eq)]
pub enum OrderError {
    Late,
    Overflow,
    ClockReversed,
    Invalidated,
}
pub struct OrderedEvents {
    pending: BTreeMap<(u64, u64), Event>,
    watermark: Option<u64>,
    sequence: u64,
    capacity: usize,
    failed: bool,
}
impl OrderedEvents {
    pub fn new(capacity: usize) -> Self {
        Self {
            pending: BTreeMap::new(),
            watermark: None,
            sequence: 0,
            capacity,
            failed: false,
        }
    }
    fn fail<T>(&mut self, error: OrderError) -> Result<T, OrderError> {
        self.pending.clear();
        self.failed = true;
        Err(error)
    }
    pub fn push(&mut self, event: Event) -> Result<(), OrderError> {
        if self.failed {
            return Err(OrderError::Invalidated);
        }
        if self.watermark.is_some_and(|w| event.at_ns <= w) {
            return self.fail(OrderError::Late);
        }
        if self.pending.len() >= self.capacity {
            return self.fail(OrderError::Overflow);
        }
        let Some(next) = self.sequence.checked_add(1) else {
            return self.fail(OrderError::Overflow);
        };
        self.pending.insert((event.at_ns, self.sequence), event);
        self.sequence = next;
        Ok(())
    }
    /// Caller chooses a delayed watermark; records arriving behind it require new baseline.
    pub fn drain_through(&mut self, watermark: u64) -> Result<Vec<Event>, OrderError> {
        if self.failed {
            return Err(OrderError::Invalidated);
        }
        if self.watermark.is_some_and(|w| watermark < w) {
            return self.fail(OrderError::ClockReversed);
        }
        let mut result = Vec::new();
        while self
            .pending
            .first_key_value()
            .is_some_and(|(key, _)| key.0 <= watermark)
        {
            result.push(self.pending.pop_first().unwrap().1);
        }
        self.watermark = Some(watermark);
        Ok(result)
    }
    #[cfg(target_os = "windows")]
    pub(crate) fn lifecycle_changed(&self, start: u64, end: u64) -> bool {
        use super::etw_decode::{Body, Lifecycle};
        self.pending.values().any(|e| {
            e.at_ns >= start
                && e.at_ns <= end
                && matches!(
                    e.body,
                    Body::Process {
                        phase: Lifecycle::Start | Lifecycle::End,
                        ..
                    } | Body::Thread {
                        phase: Lifecycle::Start | Lifecycle::End,
                        ..
                    }
                )
        })
    }
    pub fn reset(&mut self) {
        self.pending.clear();
        self.watermark = None;
        self.sequence = 0;
        self.failed = false;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::enhanced::{
        disk_io::{Accumulator, Direction, Limits, ProcessKey},
        etw_decode::Body,
    };
    fn event(at_ns: u64, bytes: u32) -> Event {
        Event {
            at_ns,
            body: Body::Complete {
                request: 7,
                bytes,
                direction: Direction::Read,
            },
        }
    }
    #[test]
    fn order_restores_init_before_completion_and_feeds_accumulator() {
        let mut queue = OrderedEvents::new(8);
        queue.push(event(20, 4096)).unwrap();
        queue
            .push(Event {
                at_ns: 10,
                body: Body::Begin {
                    request: 7,
                    tid: 2,
                    direction: Direction::Read,
                },
            })
            .unwrap();
        let mut a = Accumulator::new(0, Limits::default());
        let key = ProcessKey {
            pid: 1,
            creation: 2,
        };
        a.process_start(0, key);
        a.thread_start(0, 2, key);
        a.baseline_complete(0);
        for e in queue.drain_through(1_000_000_000).unwrap() {
            e.apply_disk(&mut a);
        }
        assert_eq!(a.window(1_000_000_000).rows[0].read_bps, Some(4096.0));
    }
    #[test]
    fn equal_timestamps_keep_arrival_order_and_future_records_wait() {
        let mut q = OrderedEvents::new(4);
        q.push(event(10, 1)).unwrap();
        q.push(event(10, 2)).unwrap();
        q.push(event(20, 3)).unwrap();
        let r = q.drain_through(10).unwrap();
        assert_eq!(r.len(), 2);
        assert!(matches!(r[0].body, Body::Complete { bytes: 1, .. }));
        assert_eq!(q.drain_through(20).unwrap().len(), 1);
    }
    #[test]
    fn late_overflow_and_clock_reversal_require_explicit_reset() {
        let mut q = OrderedEvents::new(1);
        q.drain_through(10).unwrap();
        assert_eq!(q.push(event(10, 1)), Err(OrderError::Late));
        assert_eq!(q.push(event(20, 1)), Err(OrderError::Invalidated));
        q.reset();
        q.push(event(20, 1)).unwrap();
        assert_eq!(q.push(event(30, 1)), Err(OrderError::Overflow));
        q.reset();
        q.drain_through(30).unwrap();
        assert_eq!(q.drain_through(20).unwrap_err(), OrderError::ClockReversed);
    }
}
