//! Enrich IO initiation only when held OS identities cover the full event-time range.
//! No path/name queries, privilege changes or completion-PID guesses.
use super::{
    clock_bridge::HostClock,
    disk_io::{Limits, ProcessKey},
    enumeration::Candidate,
    etw_decode::{Body, Event},
    identity::PinnedThread,
};
use std::collections::BTreeMap;
pub type Statistics = super::disk_snapshot::IdentityQuality;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::enhanced::disk_io::Direction;
    #[test]
    fn a_new_live_thread_cannot_be_joined_to_a_precreation_io_event() {
        let clock = HostClock::new().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
            sender.send(tid).unwrap();
            wait.recv().unwrap();
        });
        let tid = receiver.recv().unwrap();
        let mut resolver = Resolver::new(Limits::default());
        let event = |at_ns| Event {
            at_ns,
            body: Body::Begin {
                request: 1,
                tid,
                direction: Direction::Read,
            },
        };
        assert!(matches!(
            resolver.enrich(event(0), &clock).body,
            Body::UnverifiedBegin { .. }
        ));
        std::thread::sleep(std::time::Duration::from_millis(5));
        let verified = resolver.enrich(event(clock.now_ns().unwrap()), &clock);
        assert!(
            matches!(verified.body,Body::VerifiedBegin{owner,..} if owner.pid==std::process::id())
        );
        done.send(()).unwrap();
        worker.join().unwrap();
    }
}
pub struct Resolver {
    threads: BTreeMap<u32, PinnedThread>,
    limits: Limits,
    pub statistics: Statistics,
}
impl Resolver {
    pub fn new(limits: Limits) -> Self {
        Self {
            threads: BTreeMap::new(),
            limits,
            statistics: Statistics::default(),
        }
    }
    pub fn adopt(&mut self, candidate: &mut Candidate) {
        for thread in std::mem::take(&mut candidate.threads) {
            if let Ok(Some((tid, _))) = thread.identity_at(candidate.lifetime_range) {
                self.threads.insert(tid, thread);
            }
        }
    }
    fn owner(&mut self, tid: u32, at: u64, clock: &HostClock) -> Option<ProcessKey> {
        let time = match clock.range(at) {
            Ok(time) => time,
            Err(_) => {
                self.statistics.clock_unverified += 1;
                return None;
            }
        };
        if let Some(thread) = self.threads.get(&tid) {
            if let Ok(Some((id, owner))) = thread.identity_at(time) {
                if id == tid {
                    return Some(owner);
                }
            }
        }
        // A reused TID is never joined to the old process; retired handles are replaced
        // only after the new handle proves creation/exit times at the initiation event.
        self.threads.remove(&tid);
        if self.threads.len() >= self.limits.threads {
            self.statistics.capacity_rejected += 1;
            return None;
        }
        let thread = match PinnedThread::open(tid) {
            Ok(thread) => thread,
            Err(_) => {
                self.statistics.open_failed += 1;
                return None;
            }
        };
        let (id, owner) = match thread.identity_at(time) {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                self.statistics.lifetime_unverified += 1;
                return None;
            }
            Err(_) => {
                self.statistics.open_failed += 1;
                return None;
            }
        };
        if id != tid {
            self.statistics.lifetime_unverified += 1;
            return None;
        }
        self.threads.insert(tid, thread);
        Some(owner)
    }
    pub fn enrich(&mut self, mut event: Event, clock: &HostClock) -> Event {
        if let Body::Begin {
            request,
            tid,
            direction,
        } = event.body
        {
            match self.owner(tid, event.at_ns, clock) {
                Some(owner) => {
                    self.statistics.resolved += 1;
                    event.body = Body::VerifiedBegin {
                        request,
                        tid,
                        owner,
                        direction,
                    };
                }
                None => {
                    self.statistics.unavailable += 1;
                    event.body = Body::UnverifiedBegin { request };
                }
            }
        }
        event
    }
    pub fn reap(&mut self, watermark: u64, clock: &HostClock) {
        if let Ok(time) = clock.range(watermark) {
            self.threads
                .retain(|_, thread| !matches!(thread.expired_before(time), Ok(true)));
        }
    }
}
