//! macOS login-item management via osascript (S8).
//!
//! Launch-at-login is opt-in (default off, D-007). Adding/removing uses the
//! System Events login-items API at normal privilege — no helper, no root.
//!
//! R9/A12: the path is passed to osascript as an ARGV argument and read inside
//! the script via `item 1 of argv`, never interpolated into the script source.
//! That removes the AppleScript injection / quoting-breakage surface entirely —
//! single-quote escaping into a double-quoted AppleScript string was wrong and
//! let quotes, backslashes, or newlines in a path corrupt the script. Register
//! and verify are separate steps whose results are reported independently;
//! a failed query is "unknown", never the user's intended state.

use std::process::Command;

/// Fixed script bodies with NO interpolated data. The path arrives via argv.
const ADD_SCRIPT: &str = r#"on run argv
  set p to item 1 of argv
  tell application "System Events" to make login item at end with properties {path:p, hidden:false}
end run"#;

const DELETE_SCRIPT: &str = r#"on run argv
  set p to item 1 of argv
  tell application "System Events" to delete (every login item whose path is p)
end run"#;

const QUERY_SCRIPT: &str = r#"on run argv
  set p to item 1 of argv
  tell application "System Events" to get the path of every login item whose path is p
end run"#;

fn run_osascript(script: &str, app_path: &str) -> Result<String, String> {
    let output = Command::new("osascript")
        .args(["-e", script, "--", app_path])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Validate that a path looks like a real .app bundle root before touching the
/// login-items list (A12): no arbitrary path is ever passed to System Events.
fn validate_bundle_path(app_path: &str) -> Result<(), String> {
    if !app_path.ends_with(".app") {
        return Err("not an .app bundle path".to_string());
    }
    if !std::path::Path::new(app_path).exists() {
        return Err("bundle path does not exist".to_string());
    }
    Ok(())
}

pub fn set_launch_at_login(enable: bool, app_path: &str) -> Result<(), String> {
    validate_bundle_path(app_path)?;
    let script = if enable { ADD_SCRIPT } else { DELETE_SCRIPT };
    run_osascript(script, app_path).map(|_| ())
}

/// Query whether our bundle is registered as a login item. Returns Err on
/// query failure — the caller must NOT substitute the intended state (A12).
pub fn is_registered(app_path: &str) -> Result<bool, String> {
    validate_bundle_path(app_path)?;
    let out = run_osascript(QUERY_SCRIPT, app_path)?;
    Ok(!out.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_carry_no_interpolated_path() {
        // The whole point of A12: the scripts are fixed templates with an argv
        // placeholder, so no path characters can break or inject AppleScript.
        for s in [ADD_SCRIPT, DELETE_SCRIPT, QUERY_SCRIPT] {
            assert!(s.contains("item 1 of argv"));
            assert!(!s.contains("{path:\""), "no interpolated quoted path");
        }
    }

    #[test]
    fn bundle_path_validation_rejects_non_app_and_missing() {
        assert!(validate_bundle_path("/tmp/notanapp").is_err());
        assert!(validate_bundle_path("/nonexistent/Foo.app").is_err());
        // A path with quotes/backslashes/unicode is fine as an ARGV value — the
        // validation only checks .app suffix + existence, never script safety.
        assert!(
            validate_bundle_path("/Applications/My \"Quoted\" App.app").is_err(),
            "missing but well-formed"
        );
    }
}
