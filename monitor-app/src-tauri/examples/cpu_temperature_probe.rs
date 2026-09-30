//! Explicit one-shot CPU source probe. Never installs, elevates or starts the app UI.
#[cfg(target_os = "windows")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        io::Write,
        mem,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::Stdio,
        time::Duration,
    };
    use tokio::io::AsyncReadExt;
    use windows_sys::Win32::System::JobObjects::*;
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--worker" {
        let mut readings = Vec::new();
        for i in 0..3 {
            if i > 0 {
                std::thread::sleep(Duration::from_secs(2));
            }
            readings.push(app_lib::enhanced::cpu_read::sample());
        }
        println!("{}", serde_json::to_string(&readings)?);
        return Ok(());
    }
    if args.len() != 1 {
        return Err("No arguments are accepted for the coordinator".into());
    }
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
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
        return Err(std::io::Error::last_os_error().into());
    }
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .arg("--worker")
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    if unsafe {
        AssignProcessToJobObject(
            job.as_raw_handle(),
            child.raw_handle().ok_or("missing worker handle")?,
        )
    } == 0
    {
        let _ = child.kill().await;
        return Err(std::io::Error::last_os_error().into());
    }
    let mut output = child
        .stdout
        .take()
        .ok_or("missing worker stdout")?
        .take(16385);
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(12), async {
        output.read_to_end(&mut bytes).await?;
        child.wait().await
    })
    .await;
    let report = match result {
        Ok(Ok(code)) if code.success() && bytes.len() <= 16384 => {
            serde_json::json!({"state":"completed","observations":serde_json::from_slice::<serde_json::Value>(&bytes)?})
        }
        _ => {
            unsafe {
                TerminateJobObject(job.as_raw_handle(), 1);
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            serde_json::json!({"state":"worker_failed_or_timed_out","observations":[]})
        }
    };
    let output_path = std::env::temp_dir().join(format!(
        "hardware-monitor-cpu-temperature-{}.json",
        uuid::Uuid::new_v4()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)?;
    file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    file.sync_all()?;
    println!(
        "{}",
        serde_json::json!({"report_path":output_path,"report":report})
    );
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Windows-only development probe");
    std::process::exit(1);
}
