//! Read-only pinned process/thread identity. FILETIME ranges are deliberately explicit:
//! QPC values must NEVER be passed here without an independently validated clock mapping.
use super::disk_io::ProcessKey;
use std::{
    io, mem,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::FILETIME,
    System::Threading::{
        GetProcessIdOfThread, GetProcessTimes, GetThreadTimes, OpenProcess, OpenThread,
        PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
    },
};
#[derive(Clone, Copy)]
pub struct EventTime {
    pub earliest_filetime: u64,
    pub latest_filetime: u64,
}
fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}
fn covers(created: u64, exited: u64, time: EventTime) -> bool {
    created > 0
        && time.earliest_filetime <= time.latest_filetime
        && created <= time.earliest_filetime
        && (exited == 0 || time.latest_filetime < exited)
}
fn times(handle: &OwnedHandle, thread: bool) -> io::Result<(u64, u64)> {
    let mut values: [FILETIME; 4] = unsafe { mem::zeroed() };
    let result = unsafe {
        if thread {
            GetThreadTimes(
                handle.as_raw_handle(),
                &mut values[0],
                &mut values[1],
                &mut values[2],
                &mut values[3],
            )
        } else {
            GetProcessTimes(
                handle.as_raw_handle(),
                &mut values[0],
                &mut values[1],
                &mut values[2],
                &mut values[3],
            )
        }
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((ticks(values[0]), ticks(values[1])))
}
pub struct PinnedProcess {
    handle: OwnedHandle,
    key: ProcessKey,
}
impl PinnedProcess {
    pub fn open(pid: u32) -> io::Result<Self> {
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let (creation, _) = times(&handle, false)?;
        if creation == 0 {
            return Err(io::Error::other("missing process creation time"));
        }
        Ok(Self {
            handle,
            key: ProcessKey { pid, creation },
        })
    }
    pub fn key_at(&self, time: EventTime) -> io::Result<Option<ProcessKey>> {
        let (created, exited) = times(&self.handle, false)?;
        Ok((created == self.key.creation && covers(created, exited, time)).then_some(self.key))
    }
}
pub struct PinnedThread {
    handle: OwnedHandle,
    tid: u32,
    created: u64,
    owner: PinnedProcess,
}
impl PinnedThread {
    pub fn open(tid: u32) -> io::Result<Self> {
        let raw = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let (created, _) = times(&handle, true)?;
        let pid = unsafe { GetProcessIdOfThread(handle.as_raw_handle()) };
        if pid == 0 {
            return Err(io::Error::last_os_error());
        }
        let owner = PinnedProcess::open(pid)?;
        // An old thread must not be joined to a new process reusing its owner's PID.
        if created == 0 || owner.key.creation > created {
            return Err(io::Error::other("thread owner identity changed"));
        }
        Ok(Self {
            handle,
            tid,
            created,
            owner,
        })
    }
    pub fn identity_at(&self, time: EventTime) -> io::Result<Option<(u32, ProcessKey)>> {
        let (created, exited) = times(&self.handle, true)?;
        if created != self.created || !covers(created, exited, time) {
            return Ok(None);
        }
        Ok(self.owner.key_at(time)?.map(|owner| (self.tid, owner)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifetime_requires_the_entire_time_range_and_rejects_reuse() {
        assert!(covers(
            10,
            0,
            EventTime {
                earliest_filetime: 10,
                latest_filetime: 20
            }
        ));
        assert!(!covers(
            30,
            0,
            EventTime {
                earliest_filetime: 10,
                latest_filetime: 20
            }
        ));
        assert!(!covers(
            10,
            20,
            EventTime {
                earliest_filetime: 19,
                latest_filetime: 21
            }
        ));
        assert!(!covers(
            10,
            20,
            EventTime {
                earliest_filetime: 20,
                latest_filetime: 20
            }
        ));
        assert!(!covers(
            10,
            0,
            EventTime {
                earliest_filetime: 20,
                latest_filetime: 19
            }
        ));
    }
    #[test]
    fn own_process_and_thread_handles_verify_one_owner_without_extra_queries() {
        let process = PinnedProcess::open(std::process::id()).unwrap();
        let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let thread = PinnedThread::open(tid).unwrap();
        let at = EventTime {
            earliest_filetime: thread.created,
            latest_filetime: thread.created,
        };
        assert_eq!(thread.identity_at(at).unwrap(), Some((tid, process.key)));
        let before = EventTime {
            earliest_filetime: thread.created - 1,
            latest_filetime: thread.created - 1,
        };
        assert!(thread.identity_at(before).unwrap().is_none());
    }
}
