use rusqlite::{Connection, Result as SqlResult};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct HistoryDb {
    pub conn: Connection,
}

impl HistoryDb {
    pub fn new(db_path: PathBuf) -> SqlResult<Self> {
        let conn = Connection::open(db_path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> SqlResult<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metric_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                metric_id TEXT NOT NULL,
                object_id TEXT NOT NULL,
                value REAL NOT NULL,
                unit TEXT NOT NULL,
                created_at INTEGER DEFAULT (strftime('%s', 'now'))
            );

            CREATE INDEX IF NOT EXISTS idx_metric_time
            ON metric_samples(metric_id, object_id, timestamp);

            CREATE TABLE IF NOT EXISTS metric_buckets_10s (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                bucket_start INTEGER NOT NULL,
                metric_id TEXT NOT NULL,
                object_id TEXT NOT NULL,
                avg_value REAL NOT NULL,
                min_value REAL NOT NULL,
                max_value REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                coverage_ms INTEGER NOT NULL,
                unit TEXT NOT NULL,
                created_at INTEGER DEFAULT (strftime('%s', 'now'))
            );

            CREATE INDEX IF NOT EXISTS idx_bucket10s_time
            ON metric_buckets_10s(metric_id, object_id, bucket_start);

            CREATE TABLE IF NOT EXISTS metric_buckets_60s (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                bucket_start INTEGER NOT NULL,
                metric_id TEXT NOT NULL,
                object_id TEXT NOT NULL,
                avg_value REAL NOT NULL,
                min_value REAL NOT NULL,
                max_value REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                coverage_ms INTEGER NOT NULL,
                unit TEXT NOT NULL,
                created_at INTEGER DEFAULT (strftime('%s', 'now'))
            );

            CREATE INDEX IF NOT EXISTS idx_bucket60s_time
            ON metric_buckets_60s(metric_id, object_id, bucket_start);
            "
        )?;
        Ok(())
    }

    pub fn insert_sample(&self, metric_id: &str, object_id: &str, value: f64, unit: &str, timestamp: i64) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO metric_samples (timestamp, metric_id, object_id, value, unit) VALUES (?1, ?2, ?3, ?4, ?5)",
            (timestamp, metric_id, object_id, value, unit),
        )?;
        Ok(())
    }

    pub fn cleanup_old_data(&self) -> SqlResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Keep raw samples for 1 hour
        let one_hour_ago = now - 3600;
        self.conn.execute(
            "DELETE FROM metric_samples WHERE timestamp < ?1",
            [one_hour_ago],
        )?;

        // Keep 10s buckets for 24 hours
        let one_day_ago = now - 86400;
        self.conn.execute(
            "DELETE FROM metric_buckets_10s WHERE bucket_start < ?1",
            [one_day_ago],
        )?;

        // Keep 60s buckets for 7 days
        let seven_days_ago = now - 604800;
        self.conn.execute(
            "DELETE FROM metric_buckets_60s WHERE bucket_start < ?1",
            [seven_days_ago],
        )?;

        Ok(())
    }
}

pub static DB: Mutex<Option<HistoryDb>> = Mutex::new(None);

pub fn init_db(app_data_dir: PathBuf) -> SqlResult<()> {
    let db_path = app_data_dir.join("monitor.db");
    let db = HistoryDb::new(db_path)?;
    *DB.lock().unwrap() = Some(db);
    Ok(())
}

pub fn record_sample(metric_id: &str, object_id: &str, value: f64, unit: &str) -> SqlResult<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    if let Some(ref db) = *DB.lock().unwrap() {
        db.insert_sample(metric_id, object_id, value, unit, timestamp)?;
    }
    Ok(())
}

pub fn cleanup_old_data() -> SqlResult<()> {
    if let Some(ref db) = *DB.lock().unwrap() {
        db.cleanup_old_data()?;
    }
    Ok(())
}