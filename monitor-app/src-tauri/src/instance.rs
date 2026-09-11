//! Cross-process single-instance lock keyed by the canonical data dir (R3/A08).
//!
//! Uses an OS `flock` on a lock file inside the data directory. The lock is
//! held by the running process and released automatically by the OS on exit
//! (including crashes) — there is no stale PID file to misread. A second
//! instance pointed at the SAME data dir fails to acquire the lock and must
//! not start sampling; a different `MONITOR_DATA_DIR` gets an independent lock
//! and may run in parallel.

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Held while this process owns the data dir. Dropping releases the lock.
pub struct InstanceLock {
    _file: File,
}

/// Outcome of trying to become the single instance for a data dir.
pub enum Acquire {
    /// This process now owns the lock and may sample.
    Acquired(InstanceLock),
    /// Another live process owns the lock for this data dir.
    AlreadyRunning,
}

impl Acquire {
    /// True if this process holds the lock. Consumed by lib.rs startup gating;
    /// the test module also uses it directly.
    #[allow(dead_code)]
    pub fn is_acquired(&self) -> bool {
        matches!(self, Acquire::Acquired(_))
    }
}

/// Try to acquire the single-instance lock for `lock_path`. Non-blocking:
/// returns AlreadyRunning immediately if held elsewhere.
pub fn try_acquire(lock_path: &Path) -> std::io::Result<Acquire> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    let fd = file.as_raw_fd();
    // LOCK_EX | LOCK_NB: exclusive, non-blocking.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(Acquire::Acquired(InstanceLock { _file: file }))
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(Acquire::AlreadyRunning)
        } else {
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("monitor-lock-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("monitor.lock")
    }

    #[test]
    fn second_acquire_same_file_fails_first_releases_on_drop() {
        let p = tmp("same");
        let first = try_acquire(&p).unwrap();
        assert!(first.is_acquired(), "first acquire must succeed");
        // Second acquire while first is held → AlreadyRunning.
        let second = try_acquire(&p).unwrap();
        assert!(!second.is_acquired(), "second acquire must be refused");
        // Drop the first → lock released → a new acquire succeeds.
        drop(first);
        let third = try_acquire(&p).unwrap();
        assert!(third.is_acquired(), "lock must be released on drop");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn different_files_are_independent() {
        let a = tmp("a");
        let b = tmp("b");
        let la = try_acquire(&a).unwrap();
        let lb = try_acquire(&b).unwrap();
        assert!(la.is_acquired());
        assert!(lb.is_acquired(), "different lock files must not block each other");
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }
}
