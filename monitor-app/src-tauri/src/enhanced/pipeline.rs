//! Coordinates the existing components; invalidation always revokes published rates.
//! Lifecycle addresses are retired individually. New IO owners need native verification.
use super::{
    disk_io::{Accumulator, Limits, Window},
    etw_decode::{Body, Event, Lifecycle},
    event_order::{OrderError, OrderedEvents},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Stopped,
    OldGeneration,
    NotReady,
    OrderingLate,
    OrderingOverflow,
    OrderingClock,
    OrderingInvalidated,
    Decode,
    EventsLost,
    Clock,
    BaselineChanged,
    BaselineInvalid,
    IncompleteWindow,
    GenerationExhausted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    AwaitingBaseline,
    Collecting,
    Ready,
    Invalid(Fault),
    Stopped,
}
pub struct Output {
    pub generation: u64,
    pub verified_subset_only: bool,
    pub window: Window,
}
pub struct Pipeline {
    generation: u64,
    phase: Phase,
    scan_start: u64,
    cutover: Option<u64>,
    prebaseline_ignored: u64,
    limits: Limits,
    capacity: usize,
    events: OrderedEvents,
    accumulator: Accumulator,
}
fn order_fault(error: OrderError) -> Fault {
    match error {
        OrderError::Late => Fault::OrderingLate,
        OrderError::Overflow => Fault::OrderingOverflow,
        OrderError::ClockReversed => Fault::OrderingClock,
        OrderError::Invalidated => Fault::OrderingInvalidated,
    }
}
impl Pipeline {
    pub fn new(limits: Limits, capacity: usize) -> Self {
        Self {
            generation: 0,
            phase: Phase::AwaitingBaseline,
            scan_start: 0,
            cutover: None,
            prebaseline_ignored: 0,
            limits,
            capacity,
            events: OrderedEvents::new(capacity),
            accumulator: Accumulator::new(0, limits),
        }
    }
    pub fn prebaseline_ignored(&self) -> u64 {
        self.prebaseline_ignored
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    fn invalidate(&mut self, reason: Fault) {
        if self.phase == Phase::Stopped {
            return;
        }
        self.accumulator.lost(0);
        self.events.reset();
        self.phase = Phase::Invalid(reason);
    }
    pub fn report_fault(&mut self, generation: u64, reason: Fault) -> Result<(), Fault> {
        if self.phase == Phase::Stopped {
            return Err(Fault::Stopped);
        }
        if generation != self.generation {
            return Err(Fault::OldGeneration);
        }
        self.invalidate(reason);
        Ok(())
    }
    pub fn begin_baseline(&mut self, at_ns: u64) -> Result<u64, Fault> {
        if self.phase == Phase::Stopped {
            return Err(Fault::Stopped);
        }
        let Some(next) = self.generation.checked_add(1) else {
            self.stop();
            return Err(Fault::GenerationExhausted);
        };
        self.generation = next;
        self.scan_start = at_ns;
        self.cutover = None;
        self.prebaseline_ignored = 0;
        self.accumulator.reset(at_ns);
        self.events.reset();
        self.phase = Phase::Collecting;
        Ok(next)
    }
    pub fn enqueue(&mut self, generation: u64, event: Event) -> Result<(), Fault> {
        if self.phase == Phase::Stopped {
            return Err(Fault::Stopped);
        }
        // Late callbacks from a closed session cannot invalidate the new session.
        if generation != self.generation {
            return Err(Fault::OldGeneration);
        }
        if !matches!(self.phase, Phase::Collecting | Phase::Ready) {
            return Err(Fault::NotReady);
        }
        if (self.phase == Phase::Collecting && event.at_ns < self.scan_start)
            || (self.phase == Phase::Ready && self.cutover.is_some_and(|at| event.at_ns <= at))
        {
            self.prebaseline_ignored = self.prebaseline_ignored.saturating_add(1);
            return Ok(());
        }
        if let Err(error) = self.events.push(event) {
            let fault = order_fault(error);
            self.invalidate(fault);
            return Err(fault);
        }
        Ok(())
    }
    fn accept(&mut self, generation: u64, cutover: u64, next: Accumulator) -> Result<(), Fault> {
        if generation != self.generation {
            return Err(Fault::OldGeneration);
        }
        if self.phase != Phase::Collecting {
            return Err(Fault::NotReady);
        }
        if cutover < self.scan_start {
            self.invalidate(Fault::BaselineInvalid);
            return Err(Fault::BaselineInvalid);
        }
        // The candidate is only a subset whose held identities cover the whole scan.
        // Unrelated starts/ends do not invalidate that verified intersection.
        // Keep events after cutover, unlike replacing the entire pending queue.
        if let Err(error) = self.events.drain_through(cutover) {
            let fault = order_fault(error);
            self.invalidate(fault);
            return Err(fault);
        }
        self.accumulator = next;
        self.cutover = Some(cutover);
        self.phase = Phase::Ready;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    pub fn install_candidate(
        &mut self,
        generation: u64,
        candidate: &super::enumeration::Candidate,
    ) -> Result<(), Fault> {
        if generation != self.generation {
            return Err(Fault::OldGeneration);
        }
        if candidate.start_ns < self.scan_start || self.phase != Phase::Collecting {
            return Err(Fault::NotReady);
        }
        let mut next = Accumulator::new(candidate.end_ns, self.limits);
        let mut staging = OrderedEvents::new(self.capacity);
        if super::baseline::install(
            candidate.end_ns,
            candidate.lifetime_range,
            &candidate.processes,
            &candidate.threads,
            self.limits,
            &mut next,
            &mut staging,
        )
        .is_err()
        {
            self.invalidate(Fault::BaselineInvalid);
            return Err(Fault::BaselineInvalid);
        }
        self.accept(generation, candidate.end_ns, next)
    }
    pub fn window(&mut self, watermark: u64) -> Result<Output, Fault> {
        if self.phase != Phase::Ready {
            return Err(Fault::NotReady);
        }
        let events = match self.events.drain_through(watermark) {
            Ok(e) => e,
            Err(error) => {
                let fault = order_fault(error);
                self.invalidate(fault);
                return Err(fault);
            }
        };
        for event in events {
            match event.body {
                Body::Process {
                    pid,
                    phase: Lifecycle::Start | Lifecycle::End,
                } => self.accumulator.retire_process(event.at_ns, pid),
                Body::Thread {
                    pid,
                    tid,
                    phase: Lifecycle::Start | Lifecycle::End,
                } => self.accumulator.retire_thread(event.at_ns, tid, pid),
                _ => {
                    event.apply_disk(&mut self.accumulator);
                }
            }
        }
        let window = self.accumulator.window(watermark);
        if !self.accumulator.identity_valid() {
            self.invalidate(Fault::IncompleteWindow);
            return Err(Fault::IncompleteWindow);
        }
        // Unknown completions make this window incomplete, but must not destroy
        // independently held identity state. The next clean window can recover.
        Ok(Output {
            generation: self.generation,
            verified_subset_only: true,
            window,
        })
    }
    pub fn stop(&mut self) {
        self.accumulator.lost(0);
        self.events.reset();
        self.phase = Phase::Stopped;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::enhanced::disk_io::{Direction, ProcessKey};
    fn candidate(at: u64) -> Accumulator {
        let mut a = Accumulator::new(at, Limits::default());
        let p = ProcessKey {
            pid: 1,
            creation: 2,
        };
        a.process_start(at, p);
        a.thread_start(at, 3, p);
        a.baseline_complete(at);
        a
    }
    fn begin(at: u64) -> Event {
        Event {
            at_ns: at,
            body: Body::Begin {
                request: 9,
                tid: 3,
                direction: Direction::Read,
            },
        }
    }
    fn end(at: u64) -> Event {
        Event {
            at_ns: at,
            body: Body::Complete {
                request: 9,
                bytes: 4096,
                direction: Direction::Read,
            },
        }
    }
    #[test]
    fn cutover_retains_future_buffered_events_and_orders_them() {
        let mut p = Pipeline::new(Limits::default(), 16);
        let epoch = p.begin_baseline(0).unwrap();
        p.enqueue(epoch, end(30)).unwrap();
        p.enqueue(epoch, begin(20)).unwrap();
        p.accept(epoch, 10, candidate(10)).unwrap();
        let result = p.window(1_000_000_010).unwrap();
        assert!(result.verified_subset_only);
        assert_eq!(result.window.rows[0].read_bps, Some(4096.0));
    }
    #[test]
    fn unrelated_scan_lifecycle_does_not_discard_a_verified_subset() {
        let mut p = Pipeline::new(Limits::default(), 16);
        let epoch = p.begin_baseline(0).unwrap();
        p.enqueue(
            epoch,
            Event {
                at_ns: 5,
                body: Body::Thread {
                    pid: 99,
                    tid: 77,
                    phase: Lifecycle::End,
                },
            },
        )
        .unwrap();
        p.accept(epoch, 10, candidate(10)).unwrap();
        assert!(p.window(1_000_000_010).unwrap().window.ready);
    }
    #[test]
    fn faults_revoke_outputs_and_old_callbacks_cannot_poison_new_epoch() {
        let mut p = Pipeline::new(Limits::default(), 16);
        let old = p.begin_baseline(0).unwrap();
        p.accept(old, 0, candidate(0)).unwrap();
        p.invalidate(Fault::EventsLost);
        assert!(p.window(1_000_000_000).is_err());
        let next = p.begin_baseline(2_000_000_000).unwrap();
        assert_eq!(
            p.report_fault(old, Fault::Decode),
            Err(Fault::OldGeneration)
        );
        assert_eq!(p.phase(), Phase::Collecting);
        assert_eq!(
            p.enqueue(old, begin(3_000_000_000)),
            Err(Fault::OldGeneration)
        );
        p.accept(next, 2_000_000_000, candidate(2_000_000_000))
            .unwrap();
        assert_eq!(
            p.window(3_000_000_000).unwrap().window.rows[0].read_bps,
            Some(0.0)
        );
        assert_eq!(
            p.enqueue(next, begin(2_500_000_000)),
            Err(Fault::OrderingLate)
        );
        assert!(p.window(4_000_000_000).is_err());
        p.stop();
        assert_eq!(p.begin_baseline(5_000_000_000), Err(Fault::Stopped));
    }
    #[test]
    fn premeasurement_events_do_not_invalidate_but_actual_late_events_do() {
        let mut p = Pipeline::new(Limits::default(), 2);
        let epoch = p.begin_baseline(0).unwrap();
        p.accept(epoch, 10, candidate(10)).unwrap();
        p.enqueue(epoch, end(9)).unwrap();
        assert_eq!(p.prebaseline_ignored(), 1);
        assert_eq!(p.phase(), Phase::Ready);
        p.window(1_000_000_010).unwrap();
        assert_eq!(p.enqueue(epoch, end(20)), Err(Fault::OrderingLate));
        let mut p = Pipeline::new(Limits::default(), 1);
        let epoch = p.begin_baseline(0).unwrap();
        p.enqueue(epoch, begin(1)).unwrap();
        assert_eq!(p.enqueue(epoch, end(2)), Err(Fault::OrderingOverflow));
    }

    #[test]
    fn unrelated_lifecycle_and_a_missing_window_recover_without_rebaseline() {
        let mut p = Pipeline::new(Limits::default(), 16);
        let epoch = p.begin_baseline(0).unwrap();
        p.accept(epoch, 0, candidate(0)).unwrap();
        p.enqueue(
            epoch,
            Event {
                at_ns: 1,
                body: Body::Process {
                    pid: 99,
                    phase: Lifecycle::End,
                },
            },
        )
        .unwrap();
        assert_eq!(
            p.window(1_000_000_000).unwrap().window.rows[0].read_bps,
            Some(0.0)
        );
        p.enqueue(epoch, end(1_000_000_001)).unwrap();
        let missing = p.window(2_000_000_000).unwrap();
        assert!(!missing.window.ready);
        assert!(missing.window.rows.iter().all(|r| r.read_bps.is_none()));
        assert_eq!(p.phase(), Phase::Ready);
        assert_eq!(
            p.window(3_000_000_000).unwrap().window.rows[0].read_bps,
            Some(0.0)
        );
    }
    #[test]
    fn verified_initiation_keeps_original_owner_across_pid_and_tid_reuse() {
        let mut p = Pipeline::new(Limits::default(), 16);
        let epoch = p.begin_baseline(0).unwrap();
        p.accept(epoch, 0, candidate(0)).unwrap();
        let first = ProcessKey {
            pid: 1,
            creation: 2,
        };
        let reused = ProcessKey {
            pid: 1,
            creation: 99,
        };
        p.enqueue(epoch, begin(10)).unwrap();
        p.enqueue(
            epoch,
            Event {
                at_ns: 20,
                body: Body::Process {
                    pid: 1,
                    phase: Lifecycle::End,
                },
            },
        )
        .unwrap();
        p.enqueue(
            epoch,
            Event {
                at_ns: 30,
                body: Body::VerifiedBegin {
                    request: 10,
                    tid: 3,
                    owner: reused,
                    direction: Direction::Write,
                },
            },
        )
        .unwrap();
        p.enqueue(epoch, end(40)).unwrap();
        p.enqueue(
            epoch,
            Event {
                at_ns: 50,
                body: Body::Complete {
                    request: 10,
                    bytes: 2048,
                    direction: Direction::Write,
                },
            },
        )
        .unwrap();
        let window = p.window(1_000_000_000).unwrap().window;
        assert_eq!(
            window
                .rows
                .iter()
                .find(|r| r.process == first)
                .unwrap()
                .read_bytes,
            4096
        );
        assert_eq!(
            window
                .rows
                .iter()
                .find(|r| r.process == reused)
                .unwrap()
                .write_bytes,
            2048
        );
        assert!(window.rows.iter().all(|r| r.read_bps.is_none()));
        assert_eq!(
            p.window(2_000_000_000).unwrap().window.rows[0].write_bps,
            Some(0.0)
        );
    }
}
