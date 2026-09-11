//! macOS login-item management via osascript (S8).
//!
//! Launch-at-login is opt-in (default off, D-007). Adding/removing uses the
//! System Events login-items API at normal privilege — no helper, no root.
//! Failures surface as errors to the UI rather than being hidden.

use std::process::Command;

fn script_for(action: &str, app_path: &str) -> String {
    // app_path is the .app bundle path; quote single quotes defensively.
    let escaped = app_path.replace('\'', "'\\''");
    match action {
        "add" => format!(
            "tell application \"System Events\" to make login item at end with properties {{path:\"{}\", hidden:false}}",
            escaped
        ),
        "delete" => format!(
            "tell application \"System Events\" to delete (every login item whose path is \"{}\")",
            escaped
        ),
        _ => String::new(),
    }
}

fn run_osascript(script: &str) -> Result<(), String> {
    let output = Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn set_launch_at_login(enable: bool, app_path: &str) -> Result<(), String> {
    let action = if enable { "add" } else { "delete" };
    let script = script_for(action, app_path);
    if script.is_empty() {
        return Err("unknown action".to_string());
    }
    run_osascript(&script)
}

/// Best-effort query of whether our bundle is registered as a login item.
pub fn is_registered(app_path: &str) -> Result<bool, String> {
    let escaped = app_path.replace('\'', "'\\''");
    let script = format!(
        "tell application \"System Events\" to get the path of every login item whose path is \"{}\"",
        escaped
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}
