//! Versioned, atomically-saved user settings (S8).
//!
//! Settings persist to a JSON file next to the history DB. Saves are
//! atomic (write temp + rename) so a crash mid-write never leaves a
//! corrupt file; a parse failure falls back to defaults and is reported,
//! never silently reset.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    /// Foreground sampling interval in milliseconds (clamped 500..=10000).
    pub foreground_interval_ms: u64,
    /// Background sampling interval in milliseconds (clamped 1000..=30000).
    pub background_interval_ms: u64,
    /// Whether to launch at login. Default OFF (D-007); the user opts in.
    pub launch_at_login: bool,
    /// UI language override; "system" follows the app default.
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            foreground_interval_ms: 1000,
            background_interval_ms: 3000,
            launch_at_login: false,
            language: "system".to_string(),
        }
    }
}

impl Settings {
    /// Clamp values into valid ranges; returns Err with a message for
    /// out-of-range input so the IPC layer can reject it visibly.
    pub fn validate(&self) -> Result<(), String> {
        if !(500..=10_000).contains(&self.foreground_interval_ms) {
            return Err(format!("foreground_interval_ms {} out of range 500-10000", self.foreground_interval_ms));
        }
        if !(1000..=30_000).contains(&self.background_interval_ms) {
            return Err(format!("background_interval_ms {} out of range 1000-30000", self.background_interval_ms));
        }
        Ok(())
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { path: dir.join("settings.json") }
    }

    pub fn load(&self) -> Settings {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str::<Settings>(&text).unwrap_or_else(|_| Settings::default()),
            Err(_) => Settings::default(),
        }
    }

    /// Atomic save: write to a temp file then rename over the target.
    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        settings.validate()?;
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

static STORE: Mutex<Option<SettingsStore>> = Mutex::new(None);

pub fn init(dir: PathBuf) {
    if let Ok(mut guard) = STORE.lock() {
        *guard = Some(SettingsStore::new(dir));
    }
}

pub fn get() -> Settings {
    STORE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.load()))
        .unwrap_or_default()
}

pub fn set(settings: Settings) -> Result<(), String> {
    let guard = STORE.lock().map_err(|e| e.to_string())?;
    let store = guard.as_ref().ok_or("settings store not initialized")?;
    store.save(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Settings::default().validate().unwrap();
    }

    #[test]
    fn rejects_out_of_range_interval() {
        let mut s = Settings::default();
        s.foreground_interval_ms = 10;
        assert!(s.validate().is_err());
        s.foreground_interval_ms = 100_000;
        assert!(s.validate().is_err());
    }

    #[test]
    fn launch_at_login_defaults_off() {
        assert!(!Settings::default().launch_at_login);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("monitor-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SettingsStore::new(dir.clone());
        let mut s = Settings::default();
        s.foreground_interval_ms = 2000;
        store.save(&s).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.foreground_interval_ms, 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("monitor-settings-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ not json").unwrap();
        let store = SettingsStore::new(dir.clone());
        let loaded = store.load();
        assert_eq!(loaded.foreground_interval_ms, 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
