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
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='metric_samples'"
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='metric_buckets'"
        ),
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='schema_version'"
        ),
        1
    );
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
        ins(
            &db,
            start + i,
            "cpu.total_usage",
            "system",
            (i % 100) as f64,
        );
    }
    let report = db.aggregate_and_prune_at(now).unwrap();

    // Samples in [now-3600, now) are still within raw TTL: 3600 remain raw.
    let raw_left = count(&db, "SELECT COUNT(*) FROM metric_samples");
    assert_eq!(raw_left, 3600, "recent hour stays raw");

    // The older hour (3600 samples) was aggregated into 10s buckets first.
    assert!(report.buckets_10s_written > 0);
    assert_eq!(
        report.raw_deleted, 3600,
        "exactly the covered hour was deleted"
    );

    // No sample was lost: raw(3600 recent) + bucketed(3600 old) = 7200.
    let bucketed: i64 = count(
        &db,
        "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets WHERE granularity_secs=10",
    );
    assert_eq!(bucketed, 3600, "older hour conserved in 10s buckets");
    assert_eq!(raw_left + bucketed, 7200, "total samples conserved");

    let b10 = count(
        &db,
        "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=10",
    );
    assert_eq!(
        b10, 360,
        "3600s / 10s = 360 buckets for the aggregated hour"
    );
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

    let b60_count: i64 = count(
        &db,
        "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=60",
    );
    assert_eq!(b60_count, 60, "3600s / 60s = 60 buckets");

    // 10s buckets for that window must be gone (rolled up).
    let b10_left = count(
        &db,
        "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=10 AND metric_id='disk.throughput'",
    );
    assert_eq!(b10_left, 0);

    // Weighted average of constant 100.0 must be 100.0.
    let avg: f64 =
        db.avg_for_test("SELECT AVG(avg_value) FROM metric_buckets WHERE granularity_secs=60");
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
    let first = count(
        &db,
        "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
    );
    db.aggregate_and_prune_at(now).unwrap();
    let second = count(
        &db,
        "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
    );
    assert_eq!(
        first, second,
        "re-running aggregation must not double-count"
    );
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
    let pts = db
        .query_range_at("cpu.total_usage", "system", now - 200, now, 2000, now)
        .unwrap();
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
    let pts = db
        .query_range_at("cpu.total_usage", "system", now - 100, now, 10, now)
        .unwrap();
    assert!(pts.len() <= 10, "got {} points, cap was 10", pts.len());
}

// ---------------------------------------------------------------------------
// A01 reproductions: aggregation across rolling bucket boundaries loses
// samples. These FAIL against the current implementation and must pass after
// R1a/R1b. They run against in-memory DBs only — never real history.
// ---------------------------------------------------------------------------

/// A01 raw-tier loss (exact reproduction from the review): 10 samples at
/// ts=1000..1009, one 10s bucket. Aggregate at now=4605 (cutoff=1005, so only
/// ts<1005 aggregate: a PARTIAL bucket of 5; the other 5 stay raw). Then
/// aggregate at now=4665 (cutoff=1065, whole bucket eligible). The INSERT OR
/// REPLACE recomputes the bucket from the 5 still-raw rows and OVERWRITES the
/// earlier 5-sample bucket — net 10 -> 5.
#[test]
fn a01_raw_boundary_does_not_lose_samples_across_passes() {
    let db = HistoryDb::new_in_memory().unwrap();
    for ts in 1000..1010 {
        ins(&db, ts, "cpu.total_usage", "system", ts as f64);
    }

    db.aggregate_and_prune_at(4605).unwrap();
    let conserved_after_1: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(
        conserved_after_1, 10,
        "after pass 1 all 10 samples conserved"
    );

    db.aggregate_and_prune_at(4665).unwrap();
    let conserved_after_2: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(
        conserved_after_2, 10,
        "after pass 2 (advanced now) no samples lost"
    );
}

/// A01 coarse-tier loss (exact reproduction): 60 samples at ts=6000..6059.
/// Aggregate at now=92425 (10s cutoff=6025, so only part rolls up), then at
/// now=92485. The 60s bucket gets recomputed from a partial 10s source set and
/// overwritten — net 60 -> 30.
#[test]
fn a01_coarse_boundary_does_not_lose_samples_across_passes() {
    let db = HistoryDb::new_in_memory().unwrap();
    for ts in 6000..6060 {
        ins(&db, ts, "disk.throughput", "disk0", ts as f64);
    }

    db.aggregate_and_prune_at(92425).unwrap();
    let conserved_after_1: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(
        conserved_after_1, 60,
        "after pass 1 all 60 samples conserved"
    );

    db.aggregate_and_prune_at(92485).unwrap();
    let conserved_after_2: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(
        conserved_after_2, 60,
        "after pass 2 (advanced now) no samples lost"
    );
}

/// Conservation must hold for EVERY bucket-boundary offset of `now`, not just
/// aligned ones. Sweep the raw-cutoff phase 0..10s and the coarse phase
/// 0..60s; after repeated advancing passes the sample total must be conserved.
#[test]
fn aggregation_conserves_across_all_boundary_offsets() {
    for phase in 0..10i64 {
        let db = HistoryDb::new_in_memory().unwrap();
        let base = 1_800_000_000i64 + phase; // shift cutoff phase
                                             // 3h of per-second samples ending well before `base`.
        let start = base - 10_800;
        for i in 0..3600 {
            ins(&db, start + i, "cpu.total_usage", "system", (i % 90) as f64);
        }
        // Advance now in several non-aligned steps.
        for step in 0..5i64 {
            let now = base + step * 137; // odd stride crosses many boundaries
            db.aggregate_and_prune_at(now).unwrap();
            let conserved: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
                + count(
                    &db,
                    "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
                );
            assert_eq!(
                conserved, 3600,
                "phase={} step={} lost samples",
                phase, step
            );
        }
    }
}

/// R1a disk-budget stop-loss: with an injectable budget of 0 bytes, every
/// write is refused (error, not silent drop) and existing rows are never
/// deleted to get back under budget. Reads keep working. Temp file removed.
#[test]
fn over_budget_pauses_writes_but_keeps_data() {
    use std::env::temp_dir;
    let mut path = temp_dir();
    path.push(format!("monitor-budget-test-{}.db", std::process::id()));

    let db = HistoryDb::new(path.clone()).unwrap();
    // Under the default (large) budget, writes succeed.
    db.insert_sample_at("cpu.total_usage", "system", 1.0, "%", 1000)
        .unwrap();
    assert_eq!(count(&db, "SELECT COUNT(*) FROM metric_samples"), 1);

    // With an injectable budget of 0 bytes, the file is over budget: writes fail.
    let err = db.insert_sample_with_budget("cpu.total_usage", "system", 2.0, "%", 1001, 0);
    assert!(
        err.is_err(),
        "over-budget write must error, not silently drop"
    );
    // Existing data is NOT deleted to get back under budget.
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM metric_samples"),
        1,
        "over-budget must not delete existing rows"
    );
    // Reads still work.
    let pts = db
        .query_range_at("cpu.total_usage", "system", 0, 2000, 100, 2000)
        .unwrap();
    assert_eq!(pts.len(), 1, "reads continue while over budget");

    drop(db);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

// ---------------------------------------------------------------------------
// R1b verification matrix: statistical conservation, late samples, clock
// rewind, multi-metric/device, sparse data, re-entrancy. All in-memory.
// ---------------------------------------------------------------------------

/// Bucket statistics must faithfully reproduce the source: after aggregation,
/// every 10s bucket's count/sum-derived avg/min/max must match the raw rows it
/// covers. We verify min/max/sample_count exactly and avg within float eps.
#[test]
fn bucket_statistics_match_source_rows() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let base = now - 7200;
    // One full 10s bucket with known non-uniform values.
    let bucket_start = (base / 10) * 10;
    let vals = [1.0, 5.0, 2.0, 9.0, 3.0]; // min1 max9 sum20 count5 avg4
    for (i, v) in vals.iter().enumerate() {
        ins(
            &db,
            bucket_start + i as i64,
            "cpu.total_usage",
            "system",
            *v,
        );
    }
    db.aggregate_and_prune_at(now).unwrap();

    let cnt = count(
        &db,
        "SELECT sample_count FROM metric_buckets WHERE granularity_secs=10",
    );
    assert_eq!(cnt, 5);
    let mn = db.avg_for_test("SELECT min_value FROM metric_buckets WHERE granularity_secs=10");
    let mx = db.avg_for_test("SELECT max_value FROM metric_buckets WHERE granularity_secs=10");
    let av = db.avg_for_test("SELECT avg_value FROM metric_buckets WHERE granularity_secs=10");
    assert_eq!(mn, 1.0);
    assert_eq!(mx, 9.0);
    assert!((av - 4.0).abs() < 1e-6, "avg was {}", av);
    // first/last timestamps preserved
    let first = count(
        &db,
        "SELECT first_ts FROM metric_buckets WHERE granularity_secs=10",
    );
    let last = count(
        &db,
        "SELECT last_ts FROM metric_buckets WHERE granularity_secs=10",
    );
    assert_eq!(first, bucket_start);
    assert_eq!(last, bucket_start + 4);
}

/// 60s rollup must be sample-count-weighted, not a naive mean of bucket means.
#[test]
fn coarse_rollup_is_count_weighted() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let base = ((now - 100_000) / 60) * 60; // a closed 60s window, well past 24h
                                            // 10s bucket A: 10 samples of value 10. 10s bucket B: 1 sample of value 100.
                                            // Both inside the same 60s window [base, base+60).
    for i in 0..10 {
        ins(&db, base + i, "m", "o", 10.0);
    }
    ins(&db, base + 10, "m", "o", 100.0);
    db.aggregate_and_prune_at(now).unwrap();

    // Weighted avg = (10*10 + 1*100)/11 = 200/11 ≈ 18.18, NOT (10+100)/2 = 55.
    let av = db.avg_for_test("SELECT avg_value FROM metric_buckets WHERE granularity_secs=60");
    let expect = 200.0 / 11.0;
    assert!(
        (av - expect).abs() < 1e-4,
        "weighted avg {} != {}",
        av,
        expect
    );
    let total = count(
        &db,
        "SELECT SUM(sample_count) FROM metric_buckets WHERE granularity_secs=60",
    );
    assert_eq!(total, 11);
}

/// Late-arriving samples (written after their bucket was already aggregated)
/// must not be double-counted nor silently dropped without trace. With the
/// aligned cutoff, a sample inserted with an old timestamp lands in a closed
/// bucket that already exists; INSERT OR REPLACE would recompute from ONLY the
/// remaining raw rows and shrink the bucket. The stop-loss must therefore keep
/// such a bucket's source rows until they are old enough that re-aggregation
/// covers the whole bucket — verified here by conservation.
#[test]
fn late_sample_in_closed_bucket_is_conserved() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let bucket_start = ((now - 7200) / 10) * 10;
    // 5 samples aggregate at `now`.
    for i in 0..5 {
        ins(&db, bucket_start + i, "cpu.total_usage", "system", 10.0);
    }
    db.aggregate_and_prune_at(now).unwrap();
    let after1: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(after1, 5);

    // A late sample arrives for the same (already-migrated) bucket.
    ins(&db, bucket_start + 7, "cpu.total_usage", "system", 10.0);
    // Aggregate again at a later now — the late sample is still < cutoff? No:
    // it's old (bucket_start+7 << now-3600), so it IS eligible. Its bucket
    // already exists. Conservation must hold (6 total, not 5-overwritten-to-1).
    db.aggregate_and_prune_at(now + 60).unwrap();
    let after2: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(
        after2, 6,
        "late sample in closed bucket must be conserved, not overwrite"
    );
}

/// Clock rewind: if `now` moves backwards between runs, no samples may be lost
/// and no bucket may be recomputed from a partial set.
#[test]
fn clock_rewind_does_not_lose_samples() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let bucket_start = ((now - 7200) / 10) * 10;
    for i in 0..10 {
        ins(&db, bucket_start + i, "cpu.total_usage", "system", i as f64);
    }
    db.aggregate_and_prune_at(now).unwrap();
    // Rewind the clock by 5 minutes and aggregate again.
    db.aggregate_and_prune_at(now - 300).unwrap();
    let conserved: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(conserved, 10, "clock rewind must not lose samples");
}

/// Multiple metrics and devices aggregate independently; one metric's buckets
/// do not absorb another's.
#[test]
fn multi_metric_multi_device_independent() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let base = now - 7200;
    let b = (base / 10) * 10;
    for i in 0..10 {
        ins(&db, b + i, "cpu.total_usage", "system", 1.0);
        ins(&db, b + i, "disk.throughput", "disk0", 2.0);
        ins(&db, b + i, "disk.throughput", "disk1", 3.0);
    }
    db.aggregate_and_prune_at(now).unwrap();
    let groups = count(
        &db,
        "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=10",
    );
    assert_eq!(groups, 3, "one bucket per (metric, object)");
    let conserved: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(conserved, 30);
}

/// Sparse samples (a single sample in a bucket) still aggregate and conserve;
/// gaps must NOT be zero-filled.
#[test]
fn sparse_samples_conserve_without_zerofill() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let base = now - 7200;
    // One sample in bucket N, none in N+1, one in N+2.
    let b = (base / 10) * 10;
    ins(&db, b, "cpu.total_usage", "system", 42.0);
    ins(&db, b + 20, "cpu.total_usage", "system", 43.0);
    db.aggregate_and_prune_at(now).unwrap();
    let buckets = count(
        &db,
        "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs=10",
    );
    assert_eq!(buckets, 2, "no zero-filled bucket for the gap");
    let conserved: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(conserved, 2);
}

/// Re-entrancy: a fresh HistoryDb over the SAME file (simulating app restart)
/// sees already-aggregated buckets and does not double-count.
#[test]
fn restart_reentry_does_not_double_count() {
    use std::env::temp_dir;
    let mut path = temp_dir();
    path.push(format!("monitor-reentry-{}.db", std::process::id()));
    let now = crate::history::test_now();
    let base = now - 7200;
    let b = (base / 10) * 10;

    {
        let db = HistoryDb::new(path.clone()).unwrap();
        for i in 0..10 {
            ins(&db, b + i, "cpu.total_usage", "system", 1.0);
        }
        db.aggregate_and_prune_at(now).unwrap();
    } // drop = close

    // "Restart": reopen the same file and aggregate again at a later now.
    {
        let db = HistoryDb::new(path.clone()).unwrap();
        db.aggregate_and_prune_at(now + 120).unwrap();
        let conserved: i64 = count(&db, "SELECT COUNT(*) FROM metric_samples")
            + count(
                &db,
                "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
            );
        assert_eq!(conserved, 10, "restart must not double-count");
        drop(db);
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

/// MERGE-UPSERT safety: the whole aggregation pass is one transaction, so a
/// committed pass always deletes its sources; a rolled-back pass applies
/// nothing. This pins that two committed passes at the same `now` yield one
/// copy of each sample (no double-count via the merge path).
#[test]
fn merge_upsert_is_idempotent_at_same_now() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let b = ((now - 7200) / 10) * 10;
    for i in 0..10 {
        ins(&db, b + i, "cpu.total_usage", "system", 1.0);
    }
    db.aggregate_and_prune_at(now).unwrap();
    let first = count(
        &db,
        "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
    );
    assert_eq!(first, 10);
    db.aggregate_and_prune_at(now).unwrap();
    let second = count(
        &db,
        "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
    );
    assert_eq!(second, 10, "committed re-run must not double-count");
}

// ---------------------------------------------------------------------------
// A02 reproductions: tier selection by wall-clock `now` misses data that has
// not been aggregated yet (blind spot), and bucket queries that match only
// `bucket_start` drop a bucket whose window actually covers the query range.
// These FAIL against the current tier-by-now implementation.
// ---------------------------------------------------------------------------

/// A02 blind spot: a raw sample older than the raw TTL that has NOT been
/// aggregated yet (aggregation runs every 60s, may lag or fail). The query
/// assumes everything older than 1h is in buckets, so it reads zero points
/// even though the raw row exists.
#[test]
fn a02_unaggregated_old_raw_sample_is_queryable() {
    let db = HistoryDb::new_in_memory().unwrap();
    // One raw sample at ts=1000; do NOT aggregate. Query at now=4601 — the
    // sample is >3600s old, so wall-clock tiering looks only in buckets.
    ins(&db, 1000, "cpu.total_usage", "system", 7.0);
    let pts = db
        .query_range_at("cpu.total_usage", "system", 900, 4601, 100, 4601)
        .unwrap();
    assert_eq!(
        pts.len(),
        1,
        "un-aggregated old raw sample must still be returned"
    );
    assert_eq!(pts[0], (1000, 7.0));
}

/// A02 bucket coverage: a sample at ts=1005 lands in bucket [1000,1010) whose
/// bucket_start is 1000. Querying [1005, ...) must still return that bucket —
/// its window covers the query start even though bucket_start < 1005.
#[test]
fn a02_bucket_covering_query_start_is_not_dropped() {
    let db = HistoryDb::new_in_memory().unwrap();
    ins(&db, 1005, "cpu.total_usage", "system", 9.0);
    // Aggregate at a now that migrates it into a 10s bucket.
    db.aggregate_and_prune_at(4610).unwrap();
    let pts = db
        .query_range_at("cpu.total_usage", "system", 1005, 4610, 100, 4610)
        .unwrap();
    assert_eq!(
        pts.len(),
        1,
        "bucket whose window covers the range start must be returned"
    );
    assert_eq!(pts[0].0, 1000, "bucket start is 1000");
    assert_eq!(pts[0].1, 9.0);
}

/// A05 atomicity: insert_samples_batch lands all rows together on success;
/// an over-budget batch writes nothing. (Budget refusal needs an on-disk file
/// to measure size, so that part uses a temp file; atomicity uses in-memory.)
#[test]
fn batch_insert_is_atomic() {
    let db = HistoryDb::new_in_memory().unwrap();
    let rows = [
        ("cpu.total_usage", "system", 1.0, "%"),
        ("memory.used_percent", "system", 2.0, "%"),
        ("gpu.utilization", "gpu0", 3.0, "%"),
    ];
    db.insert_samples_batch(5000, &rows).unwrap();
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM metric_samples"),
        3,
        "full batch landed"
    );

    // Over-budget batch (budget 0, on-disk file) must write nothing.
    use std::env::temp_dir;
    let mut path = temp_dir();
    path.push(format!("monitor-batch-budget-{}.db", std::process::id()));
    let fdb = HistoryDb::new(path.clone()).unwrap();
    fdb.insert_samples_batch(5000, &rows).unwrap();
    let before = count(&fdb, "SELECT COUNT(*) FROM metric_samples");
    let more = [("disk.throughput", "disk0", 9.0, "MB/s")];
    let err = fdb.insert_samples_batch_with_budget(6000, &more, 0);
    assert!(err.is_err(), "over-budget batch must error");
    assert_eq!(
        count(&fdb, "SELECT COUNT(*) FROM metric_samples"),
        before,
        "rejected batch wrote nothing"
    );
    drop(fdb);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

/// R14 virtual clock: a sample older than the full 7-day retention window is
/// pruned, while one just inside the window is conserved (as a bucket).
#[test]
fn samples_older_than_seven_days_are_pruned() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    const RETENTION: i64 = 604_800;

    let old_ts = now - RETENTION - 120; // 8+ days old → must be pruned
    let kept_ts = now - RETENTION + 3600; // ~7 days minus 1h → conserved
    ins(&db, old_ts, "cpu.total_usage", "system", 5.0);
    ins(&db, kept_ts, "cpu.total_usage", "system", 7.0);

    db.aggregate_and_prune_at(now).unwrap();

    let remaining_raw = count(&db, "SELECT COUNT(*) FROM metric_samples");
    let remaining_buckets = count(&db, "SELECT COUNT(*) FROM metric_buckets");
    // The old sample is gone entirely; the recent one survives as a bucket.
    assert_eq!(
        remaining_raw, 0,
        "no raw rows should remain after aggregation"
    );
    assert_eq!(
        remaining_buckets, 1,
        "only the in-window sample is conserved"
    );
    let total_val: f64 = {
        // The surviving bucket must carry the kept sample's value, not the
        // pruned one (conservation of the right data). Single sample ⇒ avg == 7.
        db.avg_for_test("SELECT COALESCE(SUM(avg_value*sample_count),0) FROM metric_buckets")
    };
    assert_eq!(total_val, 7.0, "kept sample conserved; pruned sample gone");
}

/// R14 virtual clock: a long sampling gap (app off for 2 days) does not lose
/// the pre-gap data and does not fabricate points for the gap. After restart,
/// aggregation conserves exactly the samples that were actually recorded.
#[test]
fn long_gap_conserves_pre_gap_data_without_fabrication() {
    let db = HistoryDb::new_in_memory().unwrap();
    let now = crate::history::test_now();
    let gap_secs = 2 * 86_400; // app off for 2 days

    // 10 samples just before the gap.
    let base = now - gap_secs - 60;
    for i in 0..10 {
        ins(&db, base + i, "cpu.total_usage", "system", 1.0);
    }
    // 10 samples after the gap (recent).
    for i in 0..10 {
        ins(&db, now - 60 + i, "cpu.total_usage", "system", 3.0);
    }

    db.aggregate_and_prune_at(now).unwrap();

    // Nothing is lost: every recorded sample is in raw or a bucket.
    let conserved = count(&db, "SELECT COUNT(*) FROM metric_samples")
        + count(
            &db,
            "SELECT COALESCE(SUM(sample_count),0) FROM metric_buckets",
        );
    assert_eq!(conserved, 20, "pre-gap + post-gap samples all conserved");

    // No points fabricated for the 2-day gap: query a strictly-empty region in
    // the middle of the gap (well clear of both clusters) and expect zero.
    let mid_start = base + 100; // just after the pre-gap cluster (ends base+9)
    let mid_end = now - 100; // just before the post-gap cluster (starts now-60)
    let gap_points = db
        .query_range_at("cpu.total_usage", "system", mid_start, mid_end, 100, now)
        .unwrap();
    assert!(gap_points.is_empty(), "no fabricated points in the gap");
}
