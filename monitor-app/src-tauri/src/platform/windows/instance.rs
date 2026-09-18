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

// A signal carries no command or data: its only meaning is "show the existing window".
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

fn event_name(path: &Path) -> std::io::Result<Vec<u16>> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.canonicalize()?.hash(&mut hash);
    Ok(
        format!("Local\\HardwareMonitor.Reopen.v1.{:016x}\0", hash.finish())
            .encode_utf16()
            .collect(),
    )
}

pub struct ReopenEvent(OwnedHandle);
impl ReopenEvent {
    pub fn create(lock_path: &Path) -> std::io::Result<Self> {
        let name = event_name(lock_path)?;
        // Auto reset; default user ACL; non-inheritable handle, session-local namespace.
        let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
    }
    pub fn requested(&self) -> std::io::Result<bool> {
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(std::io::Error::last_os_error()),
        }
    }
}

pub fn request_reopen(lock_path: &Path) -> std::io::Result<()> {
    let name = event_name(lock_path)?;
    // The lock may be acquired just before its notification event is created.
    for attempt in 0..20 {
        let raw = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if !raw.is_null() {
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            return if unsafe { SetEvent(handle.as_raw_handle()) } != 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            };
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(2) || attempt == 19 {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopen_signal_is_scoped_consumed_and_released() {
        let root = std::env::temp_dir().join(format!("monitor-reopen-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("one.lock");
        let other = root.join("two.lock");
        let _one_lock = try_acquire(&first).unwrap();
        let _two_lock = try_acquire(&other).unwrap();
        let event = ReopenEvent::create(&first).unwrap();
        let independent = ReopenEvent::create(&other).unwrap();
        assert!(!event.requested().unwrap());
        request_reopen(&first).unwrap();
        assert!(!independent.requested().unwrap());
        assert!(event.requested().unwrap());
        assert!(!event.requested().unwrap());
        drop(event);
        let recreated = ReopenEvent::create(&first).unwrap();
        assert!(!recreated.requested().unwrap());
        drop(recreated);
        drop(independent);
        drop(_one_lock);
        drop(_two_lock);
        std::fs::remove_dir_all(root).unwrap();
    }

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
