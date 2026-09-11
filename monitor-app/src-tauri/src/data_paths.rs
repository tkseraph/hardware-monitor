//! Unified data-directory resolution (R3/A07).
//!
//! There is exactly one place that decides where the app's data lives:
//! `DataPaths::resolve`. When `MONITOR_DATA_DIR` is set (tests, isolated
//! acceptance runs), NOTHING under the real `app_data_dir` is created or read.
//! All consumers — history DB, settings, logs, instance lock — take their
//! location from a single `DataPaths` so isolation is total.

use std::path::PathBuf;

/// Canonical set of filesystem locations the app reads/writes.
#[derive(Debug, Clone)]
pub struct DataPaths {
    /// Root data directory (already canonicalized / created).
    pub root: PathBuf,
}

impl DataPaths {
    /// Resolve the effective data root. If `MONITOR_DATA_DIR` is set it wins
    /// and `app_data_dir` is never touched; otherwise `app_data_dir` is used.
    /// The directory is created if missing and canonicalized so that two
    /// spellings of the same path compare equal (used by the instance lock).
    pub fn resolve(app_data_dir: PathBuf) -> std::io::Result<Self> {
        let root = std::env::var_os("MONITOR_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or(app_data_dir);
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize().unwrap_or(root);
        Ok(Self { root })
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("monitor.db")
    }

    pub fn settings_dir(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("monitor.lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests mutate the process-wide MONITOR_DATA_DIR env var; serialize them
    // so parallel test threads don't race on set/remove.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn env_override_wins_and_isolates() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("monitor-dpaths-{}", std::process::id()));
        std::env::set_var("MONITOR_DATA_DIR", &dir);
        let real = std::env::temp_dir().join("should-not-be-created-xyz");
        let p = DataPaths::resolve(real.clone()).unwrap();
        assert!(p.root.starts_with(&dir) || p.root == dir.canonicalize().unwrap());
        assert!(
            !real.exists(),
            "real app_data_dir must not be created when MONITOR_DATA_DIR set"
        );
        assert_eq!(p.db_path().file_name().unwrap(), "monitor.db");
        std::env::remove_var("MONITOR_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn canonicalizes_root() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("monitor-dpaths-canon-{}", std::process::id()));
        std::env::set_var("MONITOR_DATA_DIR", &dir);
        let p = DataPaths::resolve(dir.clone()).unwrap();
        assert_eq!(p.root, p.root.canonicalize().unwrap());
        std::env::remove_var("MONITOR_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
