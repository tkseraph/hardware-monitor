//! Reject difference counters that span a long pause or a wall-clock discontinuity.
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub struct SampleClock {
    previous: Option<(Instant, SystemTime, Duration)>,
}
impl SampleClock {
    pub fn observe(&mut self, period: Duration) -> bool {
        self.observe_at(Instant::now(), SystemTime::now(), period)
    }
    fn observe_at(&mut self, monotonic: Instant, wall: SystemTime, period: Duration) -> bool {
        let previous = self.previous.replace((monotonic, wall, period));
        let Some((old_monotonic, old_wall, old_period)) = previous else {
            return false;
        };
        let (Some(elapsed), Ok(wall_elapsed)) = (
            monotonic.checked_duration_since(old_monotonic),
            wall.duration_since(old_wall),
        ) else {
            return false;
        };
        // Preserve valid foreground/background transitions, including a 30 s background period.
        let limit = old_period
            .max(period)
            .saturating_mul(3)
            .max(Duration::from_secs(15));
        !elapsed.is_zero()
            && elapsed <= limit
            && wall_elapsed <= limit
            && elapsed.abs_diff(wall_elapsed) <= Duration::from_secs(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_reading_warms_up_then_normal_and_background_intervals_are_valid() {
        let mut clock = SampleClock::default();
        let m = Instant::now();
        let w = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        assert!(!clock.observe_at(m, w, Duration::from_secs(1)));
        assert!(clock.observe_at(
            m + Duration::from_secs(1),
            w + Duration::from_secs(1),
            Duration::from_secs(30)
        ));
        assert!(clock.observe_at(
            m + Duration::from_secs(31),
            w + Duration::from_secs(31),
            Duration::from_secs(1)
        ));
    }
    #[test]
    fn long_pause_discards_one_interval_and_next_interval_recovers() {
        let mut clock = SampleClock::default();
        let m = Instant::now();
        let w = SystemTime::UNIX_EPOCH;
        clock.observe_at(m, w, Duration::from_secs(1));
        assert!(!clock.observe_at(
            m + Duration::from_secs(20),
            w + Duration::from_secs(20),
            Duration::from_secs(1)
        ));
        assert!(clock.observe_at(
            m + Duration::from_secs(21),
            w + Duration::from_secs(21),
            Duration::from_secs(1)
        ));
    }
    #[test]
    fn suspended_monotonic_clock_and_wall_clock_adjustments_break_continuity() {
        let m = Instant::now();
        let w = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        for delta in [-1, 5, 300] {
            let mut clock = SampleClock::default();
            clock.observe_at(m, w, Duration::from_secs(30));
            let next_wall = if delta < 0 {
                w - Duration::from_secs(1)
            } else {
                w + Duration::from_secs(delta as u64)
            };
            assert!(!clock.observe_at(
                m + Duration::from_secs(1),
                next_wall,
                Duration::from_secs(30)
            ));
        }
    }
}
