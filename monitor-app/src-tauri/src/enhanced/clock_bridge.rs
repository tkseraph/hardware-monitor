//! Conservative QPC-nanosecond to FILETIME bounds. No clock is changed.
use super::{etw_decode::QpcClock, identity::EventTime};
use std::io;
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::{
        Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        SystemInformation::GetSystemTimePreciseAsFileTime,
    },
};
const MAX_DISTANCE_NS: u64 = 5_000_000_000;
const MAX_SAMPLE_NS: u64 = 5_000_000;
const MARGIN_TICKS: i128 = 10_000; // 1 ms conservative guard; not a universal clock accuracy claim.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub before_ns: u64,
    pub after_ns: u64,
    pub utc_filetime: u64,
}
pub struct Bridge {
    anchor: Anchor,
    valid: bool,
}
fn error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "clock correspondence unavailable",
    )
}
impl Bridge {
    pub fn new(anchor: Anchor) -> io::Result<Self> {
        if anchor.utc_filetime == 0
            || anchor
                .after_ns
                .checked_sub(anchor.before_ns)
                .is_none_or(|d| d > MAX_SAMPLE_NS)
        {
            return Err(error());
        }
        Ok(Self {
            anchor,
            valid: true,
        })
    }
    pub fn range(&self, at_ns: u64) -> io::Result<EventTime> {
        if !self.valid || at_ns.abs_diff(self.anchor.after_ns) > MAX_DISTANCE_NS {
            return Err(error());
        }
        let base = i128::from(self.anchor.utc_filetime);
        let lower = base + (i128::from(at_ns) - i128::from(self.anchor.after_ns)).div_euclid(100)
            - MARGIN_TICKS;
        let delta = i128::from(at_ns) - i128::from(self.anchor.before_ns);
        let upper = base - (-delta).div_euclid(100) + MARGIN_TICKS;
        Ok(EventTime {
            earliest_filetime: u64::try_from(lower).map_err(|_| error())?,
            latest_filetime: u64::try_from(upper).map_err(|_| error())?,
        })
    }
    pub fn confirm(&mut self, next: Anchor) -> io::Result<()> {
        let result = (|| {
            let candidate = Self::new(next)?;
            if next.before_ns < self.anchor.after_ns {
                return Err(error());
            }
            let expected_start = self.range(next.before_ns)?;
            let expected_end = self.range(next.after_ns)?;
            if next.utc_filetime < expected_start.earliest_filetime
                || next.utc_filetime > expected_end.latest_filetime
            {
                return Err(error());
            }
            Ok(candidate)
        })();
        match result {
            Ok(next) => {
                *self = next;
                Ok(())
            }
            Err(e) => {
                self.valid = false;
                Err(e)
            }
        }
    }
}
fn counter() -> io::Result<i64> {
    let mut value = 0;
    if unsafe { QueryPerformanceCounter(&mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}
pub struct HostClock {
    qpc: QpcClock,
    bridge: Bridge,
}
impl HostClock {
    pub fn new() -> io::Result<Self> {
        let mut frequency = 0;
        if unsafe { QueryPerformanceFrequency(&mut frequency) } == 0 || frequency <= 0 {
            return Err(error());
        }
        let qpc = QpcClock::new(counter()?, frequency as u64).map_err(|_| error())?;
        let bridge = Bridge::new(Self::sample(&qpc)?)?;
        Ok(Self { qpc, bridge })
    }
    fn sample(clock: &QpcClock) -> io::Result<Anchor> {
        let before_ns = clock.nanoseconds(counter()?).map_err(|_| error())?;
        let mut filetime = FILETIME::default();
        unsafe { GetSystemTimePreciseAsFileTime(&mut filetime) };
        let after_ns = clock.nanoseconds(counter()?).map_err(|_| error())?;
        Ok(Anchor {
            before_ns,
            after_ns,
            utc_filetime: (u64::from(filetime.dwHighDateTime) << 32)
                | u64::from(filetime.dwLowDateTime),
        })
    }
    pub fn now_ns(&self) -> io::Result<u64> {
        self.qpc.nanoseconds(counter()?).map_err(|_| error())
    }
    pub fn confirm(&mut self) -> io::Result<()> {
        match Self::sample(&self.qpc) {
            Ok(anchor) => self.bridge.confirm(anchor),
            Err(error) => {
                self.bridge.valid = false;
                Err(error)
            }
        }
    }
    pub fn range(&self, at_ns: u64) -> io::Result<EventTime> {
        self.bridge.range(at_ns)
    }
    pub fn qpc(&self) -> &QpcClock {
        &self.qpc
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn anchor(at: u64, wall: u64) -> Anchor {
        Anchor {
            before_ns: at,
            after_ns: at + 100,
            utc_filetime: wall,
        }
    }
    #[test]
    fn interval_bounds_and_rounding_do_not_claim_exact_clock_correspondence() {
        let b = Bridge::new(anchor(1000, 100000)).unwrap();
        let r = b.range(1100).unwrap();
        assert_eq!(r.earliest_filetime, 90000);
        assert_eq!(r.latest_filetime, 110001);
        assert!(b.range(6_000_000_000).is_err());
    }
    #[test]
    fn jump_reverse_and_slow_calibration_require_new_clock() {
        let mut b = Bridge::new(anchor(0, 100000)).unwrap();
        assert!(b.confirm(anchor(1_000_000, 110000)).is_ok());
        assert!(b.confirm(anchor(2_000_000, 500000)).is_err());
        assert!(b.range(2_000_000).is_err());
        let mut b = Bridge::new(anchor(1000, 100000)).unwrap();
        assert!(b.confirm(anchor(0, 100000)).is_err());
        assert!(Bridge::new(Anchor {
            before_ns: 0,
            after_ns: MAX_SAMPLE_NS + 1,
            utc_filetime: 100000
        })
        .is_err());
    }
    #[test]
    fn native_clock_confirms_and_bounds_current_process_identity() {
        let mut clock = HostClock::new().unwrap();
        let process = super::super::identity::PinnedProcess::open(std::process::id()).unwrap();
        clock.confirm().unwrap();
        let range = clock.range(clock.now_ns().unwrap()).unwrap();
        assert!(process.key_at(range).unwrap().is_some());
    }
}
