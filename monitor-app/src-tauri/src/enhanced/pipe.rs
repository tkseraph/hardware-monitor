//! Local, single-client transport. Authentication is required in BOTH directions.
use std::{
    io, mem,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::{
    Foundation::{LocalFree, FILETIME, WAIT_TIMEOUT},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, TokenLinkedToken, TokenSessionId, TokenStatistics, TokenUser,
        SECURITY_ATTRIBUTES, TOKEN_LINKED_TOKEN, TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER,
    },
    System::{
        Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
        Threading::{
            GetProcessTimes, OpenProcess, OpenProcessToken, WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "unexpected enhanced pipe peer",
    )
}
struct Local(*mut std::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
#[derive(PartialEq, Eq)]
struct Identity {
    sid: String,
    session: u32,
    logon: (u32, i32),
    created: u64,
}
pub struct Peer {
    handle: OwnedHandle,
    pid: u32,
    identity: Identity,
}
impl Peer {
    pub fn open(pid: u32) -> io::Result<Self> {
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | 0x00100000, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let identity = identity(&handle, pid)?;
        Ok(Self {
            handle,
            pid,
            identity,
        })
    }
    fn matches(&self, pid: u32) -> io::Result<()> {
        if pid != self.pid
            || unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } != WAIT_TIMEOUT
        {
            return Err(denied());
        }
        let current = Peer::open(std::process::id())?;
        let live = identity(&self.handle, self.pid)?;
        if live != self.identity
            || live.sid != current.identity.sid
            || !logon_matches(
                live.logon,
                current.identity.logon,
                linked_logon(&current.handle),
            )
            || live.session != current.identity.session
        {
            return Err(denied());
        }
        Ok(())
    }
}
fn logon_matches(peer: (u32, i32), current: (u32, i32), linked: Option<(u32, i32)>) -> bool {
    peer == current || linked == Some(peer)
}
// Accept only the paired UAC token returned by Windows for this process, not any
// same-user logon. No impersonation or token duplication is performed.
fn linked_logon(process: &OwnedHandle) -> Option<(u32, i32)> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = OwnedHandle::from_raw_handle(token);
        let mut linked: TOKEN_LINKED_TOKEN = mem::zeroed();
        let mut needed = 0;
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenLinkedToken,
            (&mut linked as *mut TOKEN_LINKED_TOKEN).cast(),
            mem::size_of_val(&linked) as u32,
            &mut needed,
        ) == 0
            || linked.LinkedToken.is_null()
        {
            return None;
        }
        let linked = OwnedHandle::from_raw_handle(linked.LinkedToken);
        let mut stats: TOKEN_STATISTICS = mem::zeroed();
        if GetTokenInformation(
            linked.as_raw_handle(),
            TokenStatistics,
            (&mut stats as *mut TOKEN_STATISTICS).cast(),
            mem::size_of_val(&stats) as u32,
            &mut needed,
        ) == 0
        {
            return None;
        }
        Some((
            stats.AuthenticationId.LowPart,
            stats.AuthenticationId.HighPart,
        ))
    }
}

fn identity(process: &OwnedHandle, _pid: u32) -> io::Result<Identity> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token);
        // Aligned buffer, bounded well above a TOKEN_USER plus maximum SID.
        let mut buffer = [0usize; 512];
        let mut needed = 0;
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            mem::size_of_val(&buffer) as u32,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _allocation = Local(text.cast());
        let len = (0..256).find(|&i| *text.add(i) == 0).ok_or_else(denied)?;
        let sid =
            String::from_utf16(std::slice::from_raw_parts(text, len)).map_err(|_| denied())?;
        let mut stats: TOKEN_STATISTICS = mem::zeroed();
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenStatistics,
            (&mut stats as *mut TOKEN_STATISTICS).cast(),
            mem::size_of_val(&stats) as u32,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut session = 0;
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenSessionId,
            (&mut session as *mut u32).cast(),
            4,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut times: [FILETIME; 4] = mem::zeroed();
        if GetProcessTimes(
            process.as_raw_handle(),
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Identity {
            sid,
            session,
            logon: (
                stats.AuthenticationId.LowPart,
                stats.AuthenticationId.HighPart,
            ),
            created: (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime),
        })
    }
}
fn valid_name(name: &str) -> bool {
    name.strip_prefix(r"\\.\pipe\HardwareMonitor.Enhanced.")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub fn create() -> io::Result<(String, NamedPipeServer)> {
    let name = format!(
        r"\\.\pipe\HardwareMonitor.Enhanced.{}",
        uuid::Uuid::new_v4().simple()
    );
    let current = Peer::open(std::process::id())?;
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})\0", current.identity.sid)
        .encode_utf16()
        .collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _allocation = Local(descriptor);
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let server = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .max_instances(1)
            .in_buffer_size(8192)
            .out_buffer_size(8192)
            .create_with_security_attributes_raw(
                &name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    }?;
    Ok((name, server))
}
pub async fn accept(server: &NamedPipeServer, expected: &Peer) -> io::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), server.connect())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "pipe connect timeout"))??;
    let mut pid = 0;
    if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    expected.matches(pid)
}
pub fn connect(name: &str, expected: &Peer) -> io::Result<NamedPipeClient> {
    if !valid_name(name) {
        return Err(denied());
    }
    let client = ClientOptions::new().open(name)?;
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    expected.matches(pid)?;
    Ok(client)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_current_or_kernel_linked_logon_is_accepted() {
        assert!(logon_matches((1, 0), (1, 0), None));
        assert!(logon_matches((2, 0), (1, 0), Some((2, 0))));
        assert!(!logon_matches((3, 0), (1, 0), Some((2, 0))));
        assert!(!logon_matches((2, 0), (1, 0), None));
    }
    #[tokio::test]
    async fn native_pipe_identity_and_instance_isolation() {
        let peer = Peer::open(std::process::id()).unwrap();
        let (name, server) = create().unwrap();
        let (other, second) = create().unwrap();
        assert_ne!(name, other);
        let client = connect(&name, &peer).unwrap();
        accept(&server, &peer).await.unwrap();
        let mut wrong = Peer::open(std::process::id()).unwrap();
        wrong.pid += 1;
        assert!(wrong.matches(std::process::id()).is_err());
        assert!(connect(r"\\remote\pipe\test", &peer).is_err());
        for change in 0..3 {
            let mut invalid = Peer::open(std::process::id()).unwrap();
            match change {
                0 => invalid.identity.sid = "wrong".into(),
                1 => invalid.identity.logon.0 ^= 1,
                _ => invalid.identity.created ^= 1,
            };
            assert!(invalid.matches(std::process::id()).is_err());
        }
        // A wrong expected server process is rejected using the OS-returned server PID.
        let (wrong_name, wrong_server) = create().unwrap();
        assert!(connect(&wrong_name, &wrong).is_err());
        drop(wrong_server);
        drop(client);
        drop(server);
        drop(second);
        assert!(connect(&name, &peer).is_err());
    }
}
