//! Development-only source verification. No implicit elevation or driver installation.
#[cfg(target_os = "windows")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use app_lib::enhanced::probe;
    use serde_json::{json, Value};
    use std::{io::Write, process::Stdio, time::Duration};
    use tokio::io::AsyncReadExt;
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--storage-worker" {
        println!("{}", probe::storage_probe());
        return Ok(());
    }
    if args.len() == 2 && args[1] == "--storage-method-worker" {
        println!("{}", probe::storage_method_probe());
        return Ok(());
    }
    if args.len() == 3 && args[1] == "--pipeline-worker" {
        println!("{}", app_lib::enhanced::trace_run::run(&args[2]));
        return Ok(());
    }
    if args.len() == 3 && args[1] == "--etw-worker" {
        println!("{}", probe::etw_probe(&args[2]));
        return Ok(());
    }
    let storage_only = args.len() == 2 && args[1] == "--storage-method-only";
    let pipeline_only = args.len() == 2 && args[1] == "--pipeline-only";
    if args.len() != 1 && !storage_only && !pipeline_only {
        return Err("Only the no-argument coordinator is supported for manual use".into());
    }
    async fn worker(args: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
        let mut child = tokio::process::Command::new(std::env::current_exe()?)
            .args(args)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdout = child.stdout.take().ok_or("missing stdout")?.take(32769);
        let mut bytes = Vec::new();
        let outcome = tokio::time::timeout(
            Duration::from_secs(if args.first() == Some(&"--pipeline-worker") {
                25
            } else {
                15
            }),
            async {
                stdout.read_to_end(&mut bytes).await?;
                if bytes.len() > 32768 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "oversized report",
                    ));
                }
                let status = child.wait().await?;
                if !status.success() {
                    return Err(std::io::Error::other("worker failed"));
                }
                Ok(())
            },
        )
        .await;
        if !matches!(outcome, Ok(Ok(()))) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Ok(json!({"state":"worker_failed_or_timed_out"}));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    if pipeline_only {
        let name = format!("HardwareMonitor-E2-{}", uuid::Uuid::new_v4().simple());
        let result = worker(&["--pipeline-worker", &name]).await?;
        let cleanup = if result.get("state").is_some()
            || result
                .get("stop_code")
                .and_then(Value::as_u64)
                .is_some_and(|v| v != 0)
        {
            Some(probe::cleanup_session(&name).map_err(std::io::Error::other)?)
        } else {
            None
        };
        let report = json!({"result":result,"recovery_stop_code":cleanup});
        let output = std::env::temp_dir().join(format!(
            "hardware-monitor-pipeline-{}.json",
            uuid::Uuid::new_v4()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
        file.sync_all()?;
        println!("{}", json!({"report_path":output,"report":report}));
        return Ok(());
    }
    if storage_only {
        let result = worker(&["--storage-method-worker"]).await?;
        let output = std::env::temp_dir().join(format!(
            "hardware-monitor-storage-method-{}.json",
            uuid::Uuid::new_v4()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        file.write_all(serde_json::to_string_pretty(&result)?.as_bytes())?;
        file.sync_all()?;
        println!("{}", json!({"report_path":output,"result":result}));
        return Ok(());
    }
    let storage = worker(&["--storage-worker"]).await?;
    let name = format!("HardwareMonitor-E2-{}", uuid::Uuid::new_v4().simple());
    let etw = worker(&["--etw-worker", &name]).await?;
    let cleanup = if etw.get("state").is_some()
        || etw
            .get("stop_code")
            .and_then(Value::as_u64)
            .is_some_and(|v| v != 0)
    {
        Some(probe::cleanup_session(&name).map_err(std::io::Error::other)?)
    } else {
        None
    };
    let report = json!({"version":1,"storage":storage,"etw":etw,"recovery_stop_code":cleanup,"scope":"capability only; not process attribution or validated temperature integration","drivers_installed":false,"privileges_adjusted":false});
    let output =
        std::env::temp_dir().join(format!("hardware-monitor-e2-{}.json", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    file.write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    file.sync_all()?;
    println!("{}", json!({"report_path":output,"result":report}));
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Windows only");
}
