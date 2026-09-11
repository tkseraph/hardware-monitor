//! Typed history query parameters with validation (F14).
//!
//! The IPC boundary must not accept arbitrary i64 durations or unbounded
//! result sets. This module is pure and unit-testable; lib.rs converts
//! raw IPC args into these validated types.

/// Maximum retention window we ever serve: 7 days.
pub const MAX_RANGE_SECS: i64 = 604_800;
/// Hard cap on points returned in one query, to bound IPC + render cost.
pub const MAX_POINTS: usize = 2_000;

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryQuery {
    pub metric_id: String,
    pub object_id: String,
    pub start_secs: i64,
    pub end_secs: i64,
    pub max_points: usize,
}

#[derive(Debug, PartialEq)]
pub enum QueryError {
    UnknownMetric(String),
    NonPositiveDuration,
    RangeTooLarge,
    // Reserved for when the IPC accepts an explicit end time and an explicit
    // point budget; not constructed by the current duration-only query path.
    #[allow(dead_code)]
    EndBeforeStart,
    #[allow(dead_code)]
    TooManyPoints { requested: usize, max: usize },
}

/// Metrics we actually record. Anything else is rejected rather than
/// silently returning empty.
pub const KNOWN_METRICS: &[&str] = &[
    "cpu.total_usage",
    "cpu.per_core",
    "memory.used_percent",
    "gpu.utilization",
    "disk.throughput",
    "disk.temperature",
];

pub fn validate_query(
    metric_id: &str,
    object_id: &str,
    duration_secs: i64,
    now_secs: i64,
) -> Result<HistoryQuery, QueryError> {
    if !KNOWN_METRICS.contains(&metric_id) {
        return Err(QueryError::UnknownMetric(metric_id.to_string()));
    }
    if duration_secs <= 0 {
        return Err(QueryError::NonPositiveDuration);
    }
    if duration_secs > MAX_RANGE_SECS {
        return Err(QueryError::RangeTooLarge);
    }
    // checked subtraction avoids i64 underflow on extreme inputs.
    let start = now_secs
        .checked_sub(duration_secs)
        .ok_or(QueryError::RangeTooLarge)?;
    Ok(HistoryQuery {
        metric_id: metric_id.to_string(),
        object_id: object_id.to_string(),
        start_secs: start,
        end_secs: now_secs,
        max_points: MAX_POINTS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn valid_query_ok() {
        let q = validate_query("cpu.total_usage", "system", 3600, NOW).unwrap();
        assert_eq!(q.start_secs, NOW - 3600);
        assert_eq!(q.end_secs, NOW);
        assert_eq!(q.max_points, MAX_POINTS);
    }

    #[test]
    fn rejects_unknown_metric() {
        assert!(matches!(
            validate_query("cpu.hacked", "system", 60, NOW),
            Err(QueryError::UnknownMetric(_))
        ));
    }

    #[test]
    fn rejects_zero_and_negative() {
        assert_eq!(
            validate_query("cpu.total_usage", "system", 0, NOW),
            Err(QueryError::NonPositiveDuration)
        );
        assert_eq!(
            validate_query("cpu.total_usage", "system", -500, NOW),
            Err(QueryError::NonPositiveDuration)
        );
    }

    #[test]
    fn rejects_over_seven_days() {
        assert_eq!(
            validate_query("cpu.total_usage", "system", MAX_RANGE_SECS + 1, NOW),
            Err(QueryError::RangeTooLarge)
        );
        // Exactly 7 days is allowed.
        assert!(validate_query("cpu.total_usage", "system", MAX_RANGE_SECS, NOW).is_ok());
    }

    #[test]
    fn rejects_extreme_i64_without_overflow() {
        // i64::MIN would overflow a naive `now - duration`.
        assert!(matches!(
            validate_query("cpu.total_usage", "system", i64::MIN, NOW),
            Err(QueryError::NonPositiveDuration) | Err(QueryError::RangeTooLarge)
        ));
    }
}
