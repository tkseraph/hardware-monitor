//! Explicit user-confirmed SIGTERM only; no elevation or force-kill.
use sysinfo::{Pid, System};

pub fn request(pid: u32, start_marker: u64) -> Result<(), String> {
    if pid <= 1 || pid > i32::MAX as u32 || pid == std::process::id() {
        return Err("protected".into());
    }
    let mut system = System::new();
    let id = Pid::from_u32(pid);
    system.refresh_process(id);
    let process = system.process(id).ok_or("gone")?;
    if start_marker == 0 || process.start_time() != start_marker {
        return Err("changed".into());
    }
    // A same-user check is stricter than relying on kill permissions alone.
    let uid = unsafe { libc::geteuid() };
    if process.user_id().map(|u| **u) != Some(uid) {
        return Err("permission".into());
    }
    // PID + start-time validation narrows, but cannot eliminate, the OS-level
    // PID reuse race between this check and kill on macOS.
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        return Ok(());
    }
    Err(match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => "gone",
        Some(libc::EPERM) => "permission",
        _ => "failed",
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_protected_ids() {
        for pid in [0, 1, u32::MAX, std::process::id()] {
            assert_eq!(request(pid, 1).unwrap_err(), "protected");
        }
    }
    #[test]
    fn terminates_only_disposable_child_after_identity_check() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut system = System::new();
        system.refresh_process(Pid::from_u32(pid));
        let marker = system.process(Pid::from_u32(pid)).unwrap().start_time();
        assert_eq!(request(pid, marker + 1).unwrap_err(), "changed");
        assert!(child.try_wait().unwrap().is_none());
        let result = request(pid, marker);
        if result.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().unwrap();
        assert!(result.is_ok(), "{result:?}");
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}
