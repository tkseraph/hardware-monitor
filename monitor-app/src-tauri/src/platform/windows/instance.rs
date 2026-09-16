//! Standard-library Windows file locks use LockFileEx; handles release on exit.
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

pub struct InstanceLock {
    _file: File,
}
pub enum Acquire {
    Acquired(InstanceLock),
    AlreadyRunning,
}

pub fn try_acquire(path: &Path) -> std::io::Result<Acquire> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Acquire::Acquired(InstanceLock { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(Acquire::AlreadyRunning),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_file_excludes_second_writer_and_release_allows_reopen() {
        let root = std::env::temp_dir().join(format!("monitor-win-lock-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("instance.lock");
        let first = try_acquire(&path).unwrap();
        assert!(matches!(first, Acquire::Acquired(_)));
        assert!(matches!(
            try_acquire(&path).unwrap(),
            Acquire::AlreadyRunning
        ));
        let independent = try_acquire(&root.join("other.lock")).unwrap();
        assert!(matches!(independent, Acquire::Acquired(_)));
        drop(first);
        let next = try_acquire(&path).unwrap();
        assert!(matches!(next, Acquire::Acquired(_)));
        drop(next);
        drop(independent);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
