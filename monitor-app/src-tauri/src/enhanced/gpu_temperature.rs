//! Ordinary GPU temperature source. Native driver calls never run in a UI worker.
use crate::model::{GpuTemperature, SourceState};
use std::{
    io::Read,
    mem,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::JobObjects::*;
static STOP: AtomicBool = AtomicBool::new(false);
pub fn start() {
    STOP.store(false, Ordering::SeqCst);
}
pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}
struct Pending {
    child: std::process::Child,
    job: OwnedHandle,
    output: std::thread::JoinHandle<std::io::Result<Vec<u8>>>,
}
#[derive(Default)]
pub struct Collector {
    pending: Option<Pending>,
}
impl Collector {
    pub fn collect(&mut self) -> Result<Vec<GpuTemperature>, SourceState> {
        let held = crate::platform::windows::gpu::temperature_targets();
        if held.is_empty() {
            return Err(SourceState::WarmingUp);
        }
        let result = serde_json::from_slice::<super::gpu_read::Reply>(
            &self.read_child("--gpu-temperature-read")?,
        )
        .map_err(|_| SourceState::Error)?;
        if result.state != SourceState::Ok {
            return Err(result.state);
        }
        if result.readings.len() > 32
            || result
                .readings
                .iter()
                .enumerate()
                .any(|(i, r)| result.readings[..i].iter().any(|p| p.luid == r.luid))
        {
            return Err(SourceState::Error);
        }
        let current = crate::platform::windows::gpu::temperature_targets();
        let mut temperatures = Vec::new();
        for target in held {
            if !target.lease.valid()
                || !current
                    .iter()
                    .any(|c| c.uid == target.uid && c.luid == target.luid && c.lease.valid())
            {
                continue;
            }
            let reading = result.readings.iter().find(|r| r.luid == target.luid);
            let edge = reading
                .and_then(|r| r.edge_c)
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 150.0);
            temperatures.push(GpuTemperature {
                object_id: target.uid,
                edge_celsius: edge,
                state: if edge.is_some() {
                    SourceState::Ok
                } else {
                    SourceState::Unsupported
                },
            });
        }
        if temperatures.is_empty() {
            Err(SourceState::Unverified)
        } else {
            Ok(temperatures)
        }
    }
    pub(crate) fn read_cpu_worker(&mut self) -> Result<super::cpu_read::Observation, SourceState> {
        serde_json::from_slice(&self.read_child("--cpu-companion-read")?)
            .map_err(|_| SourceState::Error)
    }
    fn read_child(&mut self, flag: &'static str) -> Result<Vec<u8>, SourceState> {
        if STOP.load(Ordering::SeqCst) {
            return Err(SourceState::Error);
        }
        if let Some(old) = &mut self.pending {
            if old
                .child
                .try_wait()
                .map_err(|_| SourceState::Error)?
                .is_none()
                || !old.output.is_finished()
            {
                return Err(SourceState::Error);
            }
            let old = self.pending.take().unwrap();
            let _ = old.output.join();
        }
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(SourceState::Error);
        }
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                mem::size_of_val(&info) as u32,
            )
        } == 0
        {
            return Err(SourceState::Error);
        }
        let mut child = Command::new(std::env::current_exe().map_err(|_| SourceState::Error)?)
            .arg(flag)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| SourceState::Error)?;
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
            let _ = child.kill();
            return Err(SourceState::Error);
        }
        let output = child.stdout.take().ok_or(SourceState::Error)?;
        // Keep at most one reader/child. A timed-out child must be reaped before any replacement.
        let output = std::thread::Builder::new()
            .name("monitor-gpu-output".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                output.take(32769).read_to_end(&mut bytes).map(|_| bytes)
            })
            .map_err(|_| SourceState::Error)?;
        self.pending = Some(Pending { child, job, output });
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pending = self.pending.as_mut().unwrap();
            if STOP.load(Ordering::SeqCst) || Instant::now() >= deadline {
                unsafe {
                    TerminateJobObject(pending.job.as_raw_handle(), 1);
                }
                return Err(SourceState::Error);
            }
            if let Some(status) = pending.child.try_wait().map_err(|_| SourceState::Error)? {
                if !status.success() {
                    return Err(SourceState::Error);
                }
                if pending.output.is_finished() {
                    let pending = self.pending.take().unwrap();
                    let bytes = pending
                        .output
                        .join()
                        .map_err(|_| SourceState::Error)?
                        .map_err(|_| SourceState::Error)?;
                    if bytes.len() > 32768 {
                        return Err(SourceState::Error);
                    }
                    return Ok(bytes);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
