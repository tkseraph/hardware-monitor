//! Tiered metrics history with real aggregation (S4).
//!
//! Retention tiers: raw samples for the last hour, 10s buckets to 24h,
//! 60s buckets to 7d. Aggregation is idempotent and runs inside a
//! transaction that only deletes source rows after their covering buckets
//! have been written and verified — the F01 failure mode (delete raw data
//! with empty buckets) is structurally impossible here.
//!
//! All times are Unix seconds. Sample timestamps are the source's
//! observation time, not the write time (F11).

use rusqlite::{Connection, Result as SqlResult};
use std::path::PathBuf;
use std::sync::Mutex;

/// raw retention before aggregation to 10s buckets.
const RAW_TTL_SECS: i64 = 3_600;
/// 10s bucket retention before aggregation to 60s buckets.
const BUCKET_10S_TTL_SECS: i64 = 86_400;
/// 60s bucket (and pre-aggregation raw) retention — the full 7-day window.
const RETENTION_SECS: i64 = 604_800;

/// R1a disk-budget stop-loss: when the database file grows past this many
/// bytes, new history *writes* stop and the over-budget flag is raised so the
/// UI can warn. Existing data is never deleted to get back under budget, and
/// reads keep working. 512 MiB is far above any realistic 7-day tiered history.
const DB_BUDGET_BYTES: i64 = 512 * 1024 * 1024;

pub struct HistoryDb {
    conn: Connection,
    /// File path for on-disk DBs (None for in-memory test DBs).
    path: Option<PathBuf>,
}

impl HistoryDb {
    pub fn new(db_path: PathBuf) -> SqlResult<Self> {
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let db = Self {
            conn,
            path: Some(db_path),
        };
        db.init_schema()?;
        Ok(db)
    }

    /// In-memory database for tests; never touches the filesystem.
    #[cfg(test)]
    pub fn new_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn, path: None };
        db.init_schema()?;
        Ok(db)
    }

    /// Current on-disk size in bytes (0 for in-memory / unknown).
    fn disk_bytes(&self) -> i64 {
        self.path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() as i64)
            .unwrap_or(0)
    }

    /// True when the DB file has exceeded the safety budget. Insert paths
    /// refuse new rows; reads and aggregation continue.
    pub fn over_budget(&self) -> bool {
        self.over_budget_with(DB_BUDGET_BYTES)
    }

    /// Budget check with an injectable threshold (bytes) for deterministic tests.
    fn over_budget_with(&self, budget_bytes: i64) -> bool {
        self.disk_bytes() > budget_bytes
    }

    /// Insert with an injectable budget for deterministic tests.
    #[cfg(test)]
    pub fn insert_sample_with_budget(
        &self,
        metric_id: &str,
        object_id: &str,
        value: f64,
        unit: &str,
        timestamp: i64,
        budget_bytes: i64,
    ) -> SqlResult<()> {
        if self.over_budget_with(budget_bytes) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                Some("history disk budget exceeded; writes paused".to_string()),
            ));
        }
        self.insert_sample_at(metric_id, object_id, value, unit, timestamp)
    }

    fn init_schema(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS metric_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                metric_id TEXT NOT NULL,
                object_id TEXT NOT NULL,
                value REAL NOT NULL,
                unit TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_samples_time
            ON metric_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_samples_metric_time
            ON metric_samples(metric_id, object_id, timestamp);

            CREATE TABLE IF NOT EXISTS metric_buckets (
                granularity_secs INTEGER NOT NULL,
                bucket_start INTEGER NOT NULL,
                metric_id TEXT NOT NULL,
                object_id TEXT NOT NULL,
                avg_value REAL NOT NULL,
                min_value REAL NOT NULL,
                max_value REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                first_ts INTEGER NOT NULL,
                last_ts INTEGER NOT NULL,
                unit TEXT NOT NULL,
                PRIMARY KEY (granularity_secs, metric_id, object_id, bucket_start)
            );
            CREATE INDEX IF NOT EXISTS idx_buckets_time
            ON metric_buckets(granularity_secs, bucket_start);
            ",
        )?;
        self.ensure_version(1)?;
        Ok(())
    }

    fn ensure_version(&self, v: i64) -> SqlResult<()> {
        let now = now_secs();
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            (v, now),
        )?;
        Ok(())
    }

    /// Single-row insert. Used by tests and the budget/late-sample paths; the
    /// production sampler writes whole snapshots via insert_samples_batch.
    #[allow(dead_code)]
    pub fn insert_sample_at(
        &self,
        metric_id: &str,
        object_id: &str,
        value: f64,
        unit: &str,
        timestamp: i64,
    ) -> SqlResult<()> {
        // R1a disk-budget stop-loss: refuse new rows when over budget rather
        // than growing the file unboundedly. Existing data is untouched.
        if self.over_budget() {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                Some("history disk budget exceeded; writes paused".to_string()),
            ));
        }
        self.conn.execute(
            "INSERT INTO metric_samples (timestamp, metric_id, object_id, value, unit) VALUES (?1, ?2, ?3, ?4, ?5)",
            (timestamp, metric_id, object_id, value, unit),
        )?;
        Ok(())
    }

    /// R2/A05: write one snapshot's samples atomically in a single transaction
    /// with a reused prepared statement. Either the whole batch lands or none
    /// does — a mid-loop failure can no longer leave half a snapshot persisted.
    /// `rows` is (metric_id, object_id, value, unit); all share `timestamp`.
    pub fn insert_samples_batch(
        &self,
        timestamp: i64,
        rows: &[(&str, &str, f64, &str)],
    ) -> SqlResult<()> {
        self.insert_samples_batch_inner(timestamp, rows, DB_BUDGET_BYTES)
    }

    #[cfg(test)]
    pub fn insert_samples_batch_with_budget(
        &self,
        timestamp: i64,
        rows: &[(&str, &str, f64, &str)],
        budget_bytes: i64,
    ) -> SqlResult<()> {
        self.insert_samples_batch_inner(timestamp, rows, budget_bytes)
    }

    fn insert_samples_batch_inner(
        &self,
        timestamp: i64,
        rows: &[(&str, &str, f64, &str)],
        budget_bytes: i64,
    ) -> SqlResult<()> {
        if rows.is_empty() {
            return Ok(());
        }
        if self.over_budget_with(budget_bytes) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                Some("history disk budget exceeded; writes paused".to_string()),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO metric_samples (timestamp, metric_id, object_id, value, unit) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (metric_id, object_id, value, unit) in rows {
                stmt.execute((timestamp, *metric_id, *object_id, *value, *unit))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Aggregate eligible raw samples into 10s buckets, and eligible 10s
    /// buckets into 60s buckets, then delete the source rows that are now
    /// fully covered. All steps run in one transaction: if aggregation
    /// fails, no source rows are deleted.
    pub fn aggregate_and_prune(&self) -> SqlResult<AggregateReport> {
        self.aggregate_and_prune_at(now_secs())
    }

    /// Time-injectable variant for deterministic tests.
    ///
    /// R1a stop-loss: sources are aggregated only in *closed* buckets. A 10s
    /// bucket is migrated only when `bucket_start + 10 <= raw_cutoff`, i.e. the
    /// whole bucket has aged out of the raw tier, so no still-raw row of that
    /// bucket remains to be recomputed and overwritten later. The raw cutoff is
    /// aligned DOWN to a 10s boundary so a partially-covered bucket is never
    /// split. Same for 10s -> 60s (aligned to 60s). Samples that remain raw
    /// because their bucket is not yet closed are reported as
    /// `pending_raw_samples` — visible, not silently lost.
    pub fn aggregate_and_prune_at(&self, now: i64) -> SqlResult<AggregateReport> {
        let mut report = AggregateReport::default();

        let tx = self.conn.unchecked_transaction()?;

        // Step 1: raw samples older than RAW_TTL → 10s buckets.
        // Align the cutoff down to a 10s boundary so we only ever migrate
        // buckets whose entire [start, start+10) window is below the cutoff.
        //
        // A bucket may already exist (written by an earlier pass). The source
        // rows still present are then *late arrivals* for an already-migrated
        // bucket: their original rows were deleted after the first write, so
        // recomputing with INSERT OR REPLACE would overwrite the stored
        // statistics with just the stragglers (A01 late-sample data loss). We
        // therefore MERGE: combine the incoming raw rows with any existing
        // bucket using count-weighted average, min/max, count, and first/last.
        let raw_cutoff = ((now - RAW_TTL_SECS) / 10) * 10;
        let raw_rows = tx.execute(
            "INSERT INTO metric_buckets
                (granularity_secs, bucket_start, metric_id, object_id,
                 avg_value, min_value, max_value, sample_count, first_ts, last_ts, unit)
             SELECT 10, (timestamp / 10) * 10, metric_id, object_id,
                    AVG(value), MIN(value), MAX(value), COUNT(*), MIN(timestamp), MAX(timestamp), unit
             FROM metric_samples
             WHERE timestamp < ?1
             GROUP BY (timestamp / 10), metric_id, object_id
             ON CONFLICT (granularity_secs, metric_id, object_id, bucket_start)
             DO UPDATE SET
                avg_value = (metric_buckets.avg_value * metric_buckets.sample_count
                             + excluded.avg_value * excluded.sample_count)
                            / (metric_buckets.sample_count + excluded.sample_count),
                min_value = MIN(metric_buckets.min_value, excluded.min_value),
                max_value = MAX(metric_buckets.max_value, excluded.max_value),
                sample_count = metric_buckets.sample_count + excluded.sample_count,
                first_ts = MIN(metric_buckets.first_ts, excluded.first_ts),
                last_ts = MAX(metric_buckets.last_ts, excluded.last_ts)",
            [raw_cutoff],
        )?;
        report.buckets_10s_written = raw_rows;

        // Only delete raw rows that are now covered by a 10s bucket. Because
        // the cutoff is aligned, every migrated row is in a fully-closed bucket.
        let deleted_raw = tx.execute(
            "DELETE FROM metric_samples
             WHERE timestamp < ?1
               AND EXISTS (
                 SELECT 1 FROM metric_buckets b
                 WHERE b.granularity_secs = 10
                   AND b.metric_id = metric_samples.metric_id
                   AND b.object_id = metric_samples.object_id
                   AND b.bucket_start = (metric_samples.timestamp / 10) * 10
               )",
            [raw_cutoff],
        )?;
        report.raw_deleted = deleted_raw;

        // Stop-loss visibility: rows that have exceeded the raw TTL but whose
        // bucket is not yet closed stay raw. With an aligned cutoff this should
        // be zero; any non-zero value signals a boundary bug or clock skew.
        report.pending_raw_samples = tx.query_row(
            "SELECT COUNT(*) FROM metric_samples WHERE timestamp < ?1",
            [now - RAW_TTL_SECS],
            |r| r.get(0),
        )?;

        // Step 2: 10s buckets older than BUCKET_10S_TTL → 60s buckets.
        // 60s bucket aggregates are sample-count-weighted over the 10s buckets.
        // Align to a 60s boundary so only closed 60s windows are migrated.
        let b10_cutoff = ((now - BUCKET_10S_TTL_SECS) / 60) * 60;
        let b10_rows = tx.execute(
            "INSERT INTO metric_buckets
                (granularity_secs, bucket_start, metric_id, object_id,
                 avg_value, min_value, max_value, sample_count, first_ts, last_ts, unit)
             SELECT 60, (bucket_start / 60) * 60, metric_id, object_id,
                    SUM(avg_value * sample_count) / SUM(sample_count),
                    MIN(min_value), MAX(max_value), SUM(sample_count),
                    MIN(first_ts), MAX(last_ts), unit
             FROM metric_buckets
             WHERE granularity_secs = 10 AND bucket_start < ?1
             GROUP BY (bucket_start / 60), metric_id, object_id
             ON CONFLICT (granularity_secs, metric_id, object_id, bucket_start)
             DO UPDATE SET
                avg_value = (metric_buckets.avg_value * metric_buckets.sample_count
                             + excluded.avg_value * excluded.sample_count)
                            / (metric_buckets.sample_count + excluded.sample_count),
                min_value = MIN(metric_buckets.min_value, excluded.min_value),
                max_value = MAX(metric_buckets.max_value, excluded.max_value),
                sample_count = metric_buckets.sample_count + excluded.sample_count,
                first_ts = MIN(metric_buckets.first_ts, excluded.first_ts),
                last_ts = MAX(metric_buckets.last_ts, excluded.last_ts)",
            [b10_cutoff],
        )?;
        report.buckets_60s_written = b10_rows;

        let deleted_b10 = tx.execute(
            "DELETE FROM metric_buckets
             WHERE granularity_secs = 10 AND bucket_start < ?1
               AND EXISTS (
                 SELECT 1 FROM metric_buckets b
                 WHERE b.granularity_secs = 60
                   AND b.metric_id = metric_buckets.metric_id
                   AND b.object_id = metric_buckets.object_id
                   AND b.bucket_start = (metric_buckets.bucket_start / 60) * 60
               )",
            [b10_cutoff],
        )?;
        report.buckets_10s_deleted = deleted_b10;

        // Stop-loss visibility for the coarse tier.
        report.pending_buckets_10s = tx.query_row(
            "SELECT COUNT(*) FROM metric_buckets WHERE granularity_secs = 10 AND bucket_start < ?1",
            [now - BUCKET_10S_TTL_SECS],
            |r| r.get(0),
        )?;

        // Step 3: hard retention. Raw samples and buckets older than the full
        // window are dropped regardless (they were never aggregatable — e.g.
        // the app was not running to aggregate them in time).
        let retention_cutoff = now - RETENTION_SECS;
        report.raw_expired = tx.execute(
            "DELETE FROM metric_samples WHERE timestamp < ?1",
            [retention_cutoff],
        )?;
        report.buckets_expired = tx.execute(
            "DELETE FROM metric_buckets WHERE bucket_start < ?1",
            [retention_cutoff],
        )?;

        tx.commit()?;
        Ok(report)
    }

    /// Query a time range using the coarsest tier that still covers it:
    /// raw for the last hour, 10s buckets to 24h, 60s buckets beyond.
    /// Points are returned as (timestamp, value); buckets use bucket_start
    /// and their avg_value. Never fabricates points for gaps.
    pub fn query_range(
        &self,
        metric_id: &str,
        object_id: &str,
        start: i64,
        end: i64,
        max_points: usize,
    ) -> SqlResult<Vec<(i64, f64)>> {
        self.query_range_at(metric_id, object_id, start, end, max_points, now_secs())
    }

    /// Time-injectable variant for deterministic tests.
    ///
    /// R2: tiers are selected by *actual data presence*, not by assuming every
    /// sample older than the wall-clock boundary has already been aggregated.
    /// Aggregation lags (it runs every 60s, and may fail), so old un-aggregated
    /// raw rows must still be found. Each tier therefore reads:
    ///   - 60s buckets whose window [bucket_start, bucket_start+60) intersects the range
    ///   - 10s buckets whose window intersects the range
    ///   - raw rows (the full range — they fill blind spots the buckets miss)
    ///
    /// Results are merged by timestamp and de-duplicated, finer tier winning,
    /// so a sample present both as a raw row and inside a bucket is reported
    /// once. No points are fabricated for gaps.
    pub fn query_range_at(
        &self,
        metric_id: &str,
        object_id: &str,
        start: i64,
        end: i64,
        max_points: usize,
        now: i64,
    ) -> SqlResult<Vec<(i64, f64)>> {
        let raw_boundary = now - RAW_TTL_SECS;
        let b10_boundary = now - BUCKET_10S_TTL_SECS;

        // Merged point set: key = timestamp, value = (value, tier_rank).
        // tier_rank: raw=0 (finest), 10s=1, 60s=2. Finer wins on collision.
        use std::collections::BTreeMap;
        let mut merged: BTreeMap<i64, (f64, u8)> = BTreeMap::new();
        let mut insert = |ts: i64, val: f64, rank: u8| {
            merged
                .entry(ts)
                .and_modify(|e| {
                    if rank < e.1 {
                        *e = (val, rank);
                    }
                })
                .or_insert((val, rank));
        };

        // 60s buckets: only meaningful for the part of the range at/below the
        // 24h boundary. A bucket covers [bucket_start, bucket_start+60), so it
        // intersects [start,end) iff bucket_start+60 > start AND bucket_start < end.
        if start < b10_boundary {
            let seg_end = end.min(b10_boundary);
            let mut stmt = self.conn.prepare(
                "SELECT bucket_start, avg_value FROM metric_buckets
                 WHERE granularity_secs = 60 AND metric_id = ?1 AND object_id = ?2
                   AND bucket_start + 60 > ?3 AND bucket_start < ?4
                 ORDER BY bucket_start ASC",
            )?;
            let rows = stmt.query_map((metric_id, object_id, start, seg_end), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })?;
            for r in rows.flatten() {
                insert(r.0, r.1, 2);
            }
        }

        // 10s buckets: for the part at/below the raw boundary (and above the
        // 24h boundary where they still exist). Window is bucket_start+10.
        if start < raw_boundary && end > b10_boundary {
            let seg_start = start.max(b10_boundary);
            let seg_end = end.min(raw_boundary);
            let mut stmt = self.conn.prepare(
                "SELECT bucket_start, avg_value FROM metric_buckets
                 WHERE granularity_secs = 10 AND metric_id = ?1 AND object_id = ?2
                   AND bucket_start + 10 > ?3 AND bucket_start < ?4
                 ORDER BY bucket_start ASC",
            )?;
            let rows = stmt.query_map((metric_id, object_id, seg_start, seg_end), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })?;
            for r in rows.flatten() {
                insert(r.0, r.1, 1);
            }
        }

        // Raw rows: the FULL range. Recent rows are the primary source for the
        // last hour; older un-aggregated rows fill blind spots where buckets
        // have not caught up. Raw is the finest tier, so it wins collisions.
        {
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, value FROM metric_samples
                 WHERE metric_id = ?1 AND object_id = ?2
                   AND timestamp >= ?3 AND timestamp < ?4
                 ORDER BY timestamp ASC",
            )?;
            let rows = stmt.query_map((metric_id, object_id, start, end), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })?;
            for r in rows.flatten() {
                insert(r.0, r.1, 0);
            }
        }

        let mut out: Vec<(i64, f64)> = merged.into_iter().map(|(ts, (v, _))| (ts, v)).collect();

        // Downsample to max_points by uniform stride if we exceeded the cap.
        if out.len() > max_points {
            let stride = out.len().div_ceil(max_points);
            out = out.into_iter().step_by(stride).collect();
        }

        Ok(out)
    }
}

#[derive(Debug, Default)]
pub struct AggregateReport {
    pub buckets_10s_written: usize,
    pub buckets_60s_written: usize,
    pub raw_deleted: usize,
    pub buckets_10s_deleted: usize,
    pub raw_expired: usize,
    pub buckets_expired: usize,
    /// Rows past the raw TTL whose 10s bucket is not yet closed (R1a). Should
    /// be zero with the aligned cutoff; non-zero indicates a boundary bug or
    /// clock skew and must be surfaced, not silently dropped.
    pub pending_raw_samples: i64,
    /// 10s buckets past the 24h TTL whose 60s window is not yet closed (R1a).
    pub pending_buckets_10s: i64,
}

#[cfg(test)]
impl HistoryDb {
    pub fn count_for_test(&self, sql: &str) -> i64 {
        self.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }
    pub fn avg_for_test(&self, sql: &str) -> f64 {
        self.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

#[cfg(test)]
pub fn test_now() -> i64 {
    // Fixed reference time so tier boundaries are deterministic in tests.
    1_800_000_000
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub static DB: Mutex<Option<HistoryDb>> = Mutex::new(None);

/// R2/R4: history-write health, surfaced so the UI can warn instead of the
/// sampler silently dropping rows. Updated on every batch write / aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryHealth {
    /// Writes are succeeding.
    Ok,
    /// DB file exceeds the safety budget; new writes are paused, data kept.
    OverBudget,
    /// A write/aggregate failed (lock, disk full, corruption, ...).
    WriteError,
    /// No DB is initialized (init failed) — history unavailable, realtime ok.
    Unavailable,
}

static HEALTH: Mutex<HistoryHealth> = Mutex::new(HistoryHealth::Ok);

/// Current history-write health for the UI / status line. Consumed by the R4
/// source-health DTO and IPC; not yet read by the running app.
#[allow(dead_code)]
pub fn health() -> HistoryHealth {
    *HEALTH.lock().unwrap()
}

fn set_health(h: HistoryHealth) {
    *HEALTH.lock().unwrap() = h;
}

/// Resolve the data directory. Tests and isolated acceptance runs set
/// Initialize against an explicit, already-resolved DB path so the whole
/// app shares one DataPaths decision (R3) — no second read of MONITOR_DATA_DIR.
pub fn init_db_at(db_path: PathBuf) -> SqlResult<()> {
    match HistoryDb::new(db_path) {
        Ok(db) => {
            *DB.lock().unwrap() = Some(db);
            set_health(HistoryHealth::Ok);
            Ok(())
        }
        Err(e) => {
            // R2/A05: init failure degrades to "realtime ok, history
            // unavailable" — never create a substitute empty DB over the
            // user's data, and surface the state instead of panicking.
            *DB.lock().unwrap() = None;
            set_health(HistoryHealth::Unavailable);
            Err(e)
        }
    }
}

/// R2/A05: record a whole snapshot atomically; update health on the result.
/// Returns the error to the caller instead of swallowing it (no `let _ =`).
pub fn record_snapshot_batch(timestamp: i64, rows: &[(&str, &str, f64, &str)]) -> SqlResult<()> {
    let guard = DB.lock().unwrap();
    if let Some(ref db) = *guard {
        match db.insert_samples_batch(timestamp, rows) {
            Ok(()) => {
                set_health(HistoryHealth::Ok);
                Ok(())
            }
            Err(e) => {
                let h = if db.over_budget() {
                    HistoryHealth::OverBudget
                } else {
                    HistoryHealth::WriteError
                };
                set_health(h);
                Err(e)
            }
        }
    } else {
        set_health(HistoryHealth::Unavailable);
        Ok(())
    }
}

/// Run one aggregation + retention pass. Called by the scheduler.
pub fn aggregate_and_prune() -> SqlResult<AggregateReport> {
    if let Some(ref db) = *DB.lock().unwrap() {
        let r = db.aggregate_and_prune();
        if r.is_err() {
            set_health(HistoryHealth::WriteError);
        }
        r
    } else {
        Ok(AggregateReport::default())
    }
}

pub fn query_range(
    metric_id: &str,
    object_id: &str,
    start: i64,
    end: i64,
    max_points: usize,
) -> SqlResult<Vec<(i64, f64)>> {
    if let Some(ref db) = *DB.lock().unwrap() {
        db.query_range(metric_id, object_id, start, end, max_points)
    } else {
        Ok(Vec::new())
    }
}
