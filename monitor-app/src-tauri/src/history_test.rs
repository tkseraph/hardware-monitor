//! Contract tests for tiered history (S4). Run against in-memory SQLite;
//! never touches real user data. These pin F01/F07/F11 acceptance.

use crate::history::HistoryDb;

/// Insert a raw sample at a specific timestamp.
fn ins(db: &HistoryDb, ts: i64, metric: &str, obj: &str, val: f64) {
    db.insert_sample_at(metric, obj, val, "%", ts).unwrap();
}

fn count(db: &HistoryDb, sql: &str) -> i64 {
    db.count_for_test(sql)
}

#[test]
fn schema_has_samples_and_unified_buckets() {
    let db = HistoryDb::new_in_memory().unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sqlite_master WHERE name='metric_samples'"), 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sqlite_master WHERE name='metric_buckets'"), 1);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sqlite_master WHERE name='schema_version'"), 1);
}

/// F01 core: after aggregation, raw rows older than 1h must be represented
/// in 10s buckets before they are deleted. Sample count must be conserved.
/// Rows younger than 1h stay raw (still within the raw tier).
#[test]
fn aggregation_conserves_samples_before_raw_deletion() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    // 2h of per-second samples.
    let start = now - 7200;
    for i in 0..7200 {
        ins(&db, start + i, "cpu.total_usage", "system", (i % 100) as f64);
    }
    let report = db.aggregate_and_prune_at(now).unwrap();

    // Samples in [now-3600, now) are still within raw TTL: 3600 remain raw.
    let raw_left = count(&db, "SELECT COUNT(*) FROM metric_samples");
    assert_eq!(raw_left, 3600, "recent hour stays raw");

    // The older hour (3600 samples) was aggregated into 10s buckets first.
    assert!(report.buckets_10s_written > 0);
    assert_eq!(report.raw_deleted, 3600, "exactly the covered hour was deleted");

    // No sample was lost: raw(3600 recent) + bucketed(3600 old) = 7200.
    let bucketed: i64 = count(&db, "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets WHERE granularity_secs=10");
    assert_eq!(bucketed, 3600, "older hour conserved in 10s buckets");
    assert_eq!(raw_left + bucketed, 7200, "total samples conserved");

    let b10 = count(&db, "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=10");
    assert_eq!(b10, 360, "3600s / 10s = 360 buckets for the aggregated hour");
}

/// Buckets older than 24h roll up into 60s buckets with weighted averages,
/// and the 10s sources are removed only after coverage exists.
#[test]
fn ten_second_buckets_rollup_to_sixty() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    // 48h of per-second samples → after aggregation these are >24h old, so
    // they should end up as 60s buckets, not 10s.
    let start = now - 172_800;
    for i in 0..3600 {
        ins(&db, start + i, "disk.throughput", "disk0", 100.0);
    }
    db.aggregate_and_prune_at(now).unwrap();

    let b60_count: i64 = count(&db, "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=60");
    assert_eq!(b60_count, 60, "3600s / 60s = 60 buckets");

    // 10s buckets for that window must be gone (rolled up).
    let b10_left = count(
        &db,
        "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=10 AND metric_id='disk.throughput'",
    );
    assert_eq!(b10_left, 0);

    // Weighted average of constant 100.0 must be 100.0.
    let avg: f64 = db.avg_for_test(
        "SELECT AVG(avg_value) FROM metric_buckets WHERE granularity_secs=60",
    );
    assert!((avg - 100.0).abs() < 1e-6);
}

/// Aggregation must be idempotent: running it twice does not double-count.
#[test]
fn aggregation_is_idempotent() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let start = now - 7200;
    for i in 0..600 {
        ins(&db, start + i, "memory.used_percent", "system", 50.0);
    }
    db.aggregate_and_prune_at(now).unwrap();
    let first = count(&db, "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets");
    db.aggregate_and_prune_at(now).unwrap();
    let second = count(&db, "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets");
    assert_eq!(first, second, "re-running aggregation must not double-count");
}

/// Range query returns raw points for the recent window and does not
/// fabricate points where there are gaps.
#[test]
fn query_returns_only_real_points() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    // Three recent raw samples with a gap.
    ins(&db, now - 100, "cpu.total_usage", "system", 10.0);
    ins(&db, now - 50, "cpu.total_usage", "system", 20.0);
    ins(&db, now - 10, "cpu.total_usage", "system", 30.0);
    let pts = db.query_range_at("cpu.total_usage", "system", now - 200, now, 2000, now).unwrap();
    assert_eq!(pts.len(), 3, "no fabricated points for gaps");
    assert_eq!(pts[0], (now - 100, 10.0));
    assert_eq!(pts[2], (now - 10, 30.0));
}

/// max_points caps the result by uniform stride downsampling.
#[test]
fn query_respects_max_points() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    for i in 0..100 {
        ins(&db, now - 100 + i, "cpu.total_usage", "system", i as f64);
    }
    let pts = db.query_range_at("cpu.total_usage", "system", now - 100, now, 10, now).unwrap();
    assert!(pts.len() <= 10, "got {} points, cap was 10", pts.len());
}
