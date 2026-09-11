//! Sampling health / staleness tracking (R4/A03).
//!
//! The frontend polls the cached snapshot over IPC; an IPC success is NOT the
//! same as a fresh collection. This module records when the scheduler last
//! *attempted* and last *succeeded* at producing a snapshot, so the UI can
//! distinguish "实时采集中" (fresh) from a stale cached value without the
//! backend ever fabricating a reading.
//!
//! Ages use the wall clock (Unix seconds) because the snapshot's own
//! `observed_at` is wall-clock; the contract is documented per source.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Default)]
struct State {
    last_attempt: Option<i64>,
    last_success: Option<i64>,
    /// Consecutive failures since the last success.
    consecutive_failures: u32,
}

static STATE: Mutex<State> = Mutex::new(State {
    last_attempt: None,
    last_success: None,
    consecutive_failures: 0,
});

/// Serializable health view returned over IPC alongside the snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SamplingHealth {
    /// Unix seconds of the last successful snapshot; None if never succeeded.
    pub last_success_at: Option<i64>,
    /// Age in seconds of the last successful snapshot (now - last_success).
    /// None if never succeeded.
    pub success_age_secs: Option<i64>,
    /// Consecutive failed collection passes since the last success.
    pub consecutive_failures: u32,
    /// True once at least one snapshot has succeeded (past warm-up).
    pub ever_succeeded: bool,
}

/// Record that a collection pass was attempted (success or not).
pub fn note_attempt() {
    let mut s = STATE.lock().unwrap();
    s.last_attempt = Some(now_secs());
}

/// Record a successful snapshot. Resets the failure streak.
pub fn note_success() {
    let mut s = STATE.lock().unwrap();
    let now = now_secs();
    s.last_attempt = Some(now);
    s.last_success = Some(now);
    s.consecutive_failures = 0;
}

/// Record a failed collection pass. Increments the failure streak.
pub fn note_failure() {
    let mut s = STATE.lock().unwrap();
    s.last_attempt = Some(now_secs());
    s.consecutive_failures = s.consecutive_failures.saturating_add(1);
}

/// Current health snapshot for IPC.
pub fn health() -> SamplingHealth {
    let s = STATE.lock().unwrap();
    let now = now_secs();
    SamplingHealth {
        last_success_at: s.last_success,
        success_age_secs: s.last_success.map(|t| (now - t).max(0)),
        consecutive_failures: s.consecutive_failures,
        ever_succeeded: s.last_success.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmup_then_success_then_failure() {
        // Fresh state is possible in a test process only once; we assert the
        // transitions rather than absolute values.
        note_attempt();
        let h0 = health();
        // After only an attempt (no success yet in this assertion path), the
        // age may be from a prior test's success — so we drive a success and
        // assert it becomes fresh.
        note_success();
        let h1 = health();
        assert!(h1.ever_succeeded);
        assert_eq!(h1.consecutive_failures, 0);
        assert!(h1.success_age_secs.unwrap() <= 1, "fresh success age ~0");

        note_failure();
        note_failure();
        let h2 = health();
        assert_eq!(h2.consecutive_failures, 2, "failure streak increments");
        assert!(h2.ever_succeeded, "still past warm-up");
        let _ = h0;
    }
}
