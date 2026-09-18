//! Versioned, atomically-saved user settings (S8).
//!
//! Settings persist to a JSON file next to the history DB. Saves are
//! atomic (write temp + rename) so a crash mid-write never leaves a
//! corrupt file; a parse failure falls back to defaults and is reported,
//! never silently reset.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(all(test, target_os = "windows"))]
mod windows_save_tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    #[test]
    fn locked_unicode_path_recovers_without_losing_unrelated_fields() {
        let root =
            std::env::temp_dir().join(format!("monitor 设置 recovery {}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = SettingsStore::new(root.clone());
        let original = Settings {
            language: "en".into(),
            foreground_interval_ms: 1700,
            ..Default::default()
        };
        store.save(&original).unwrap();
        let before = std::fs::read(&store.path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&store.path)
            .unwrap();
        assert!(store.load_outcome().error.is_some());
        assert!(store.modify(|s| s.background_interval_ms = 4000).is_err());
        drop(lock);
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
        let recovered = store.load_outcome();
        assert!(recovered.error.is_none());
        assert_eq!(recovered.settings.foreground_interval_ms, 1700);
        let saved = store.modify(|s| s.background_interval_ms = 4000).unwrap();
        assert_eq!(saved.language, "en");
        assert_eq!(saved.foreground_interval_ms, 1700);
        assert_eq!(store.load().background_interval_ms, 4000);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_file_can_be_replaced_and_failed_replacement_preserves_it() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "monitor-settings-replace-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let store = SettingsStore::new(root.clone());
        let original = Settings::default();
        store.save(&original).unwrap();
        let mut changed = original.clone();
        changed.foreground_interval_ms = 1500;
        store.save(&changed).unwrap();
        let before = std::fs::read(&store.path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&store.path)
            .unwrap();
        assert!(store.save(&original).is_err());
        drop(lock);
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
use std::sync::Mutex;

pub const SETTINGS_VERSION: u32 = 1;

/// Languages the UI actually supports. Anything else is invalid (A13).
const VALID_LANGUAGES: &[&str] = &["system", "zh", "en"];

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
    /// Strict validation; returns Err with a message for any out-of-range or
    /// unknown value so bad input is rejected visibly, never silently clamped
    /// into a meaning the user did not ask for (A13).
    pub fn validate(&self) -> Result<(), String> {
        if self.version == 0 || self.version > SETTINGS_VERSION {
            return Err(format!("unsupported settings version {}", self.version));
        }
        if !(500..=10_000).contains(&self.foreground_interval_ms) {
            return Err(format!(
                "foreground_interval_ms {} out of range 500-10000",
                self.foreground_interval_ms
            ));
        }
        if !(1000..=30_000).contains(&self.background_interval_ms) {
            return Err(format!(
                "background_interval_ms {} out of range 1000-30000",
                self.background_interval_ms
            ));
        }
        if !VALID_LANGUAGES.contains(&self.language.as_str()) {
            return Err(format!("unsupported language '{}'", self.language));
        }
        Ok(())
    }

    /// Load-time repair (A13): migrate/validate a just-parsed Settings.
    /// Returns Ok(possibly-migrated settings) when the value is salvageable,
    /// Err when it is not — the caller keeps the bad file and reports, never
    /// silently overwriting it with defaults.
    fn migrated(mut self) -> Result<Self, String> {
        // Version migration: accept anything <= current; older versions fill
        // new fields from defaults (serde already supplies them via Default on
        // parse failure per-field, but we re-affirm the version stamp).
        if self.version == 0 || self.version > SETTINGS_VERSION {
            return Err(format!("unsupported settings version {}", self.version));
        }
        self.version = SETTINGS_VERSION;
        // Language and ranges must be valid as-is; we do not guess intent.
        self.validate()?;
        Ok(self)
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

/// Outcome of a load: the settings to use plus whether the on-disk file was
/// invalid (and preserved, not overwritten).
pub struct LoadOutcome {
    pub settings: Settings,
    /// Human-readable reason the on-disk file was rejected, if it was.
    pub error: Option<String>,
}

impl SettingsStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("settings.json"),
        }
    }

    fn modify(&self, edit: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        let loaded = self.load_outcome();
        if let Some(error) = loaded.error {
            return Err(error);
        }
        let mut next = loaded.settings;
        edit(&mut next);
        next.validate()?;
        self.save(&next)?;
        Ok(next)
    }

    /// Load with validation + migration (A13).
    ///
    /// - File absent → defaults, no error (first run is not an error).
    /// - File present but unparsable / invalid / wrong version → defaults AND
    ///   a recorded error. The bad file is left untouched on disk; we never
    ///   silently overwrite it with defaults.
    pub fn load_outcome(&self) -> LoadOutcome {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return LoadOutcome {
                    settings: Settings::default(),
                    error: None,
                };
            }
            Err(e) => {
                return LoadOutcome {
                    settings: Settings::default(),
                    error: Some(format!("read: {}", e)),
                };
            }
        };
        let parsed = match serde_json::from_str::<Settings>(&text) {
            Ok(s) => s,
            Err(e) => {
                return LoadOutcome {
                    settings: Settings::default(),
                    error: Some(format!("parse: {}", e)),
                };
            }
        };
        match parsed.migrated() {
            Ok(s) => LoadOutcome {
                settings: s,
                error: None,
            },
            Err(e) => LoadOutcome {
                settings: Settings::default(),
                error: Some(e),
            },
        }
    }

    pub fn load(&self) -> Settings {
        self.load_outcome().settings
    }

    /// Atomic save: validate, write to a UNIQUE temp file, fsync, then rename
    /// over the target (A13: no shared temp name, no silent partial write).
    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        settings.validate()?;
        let text = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        let mut reserved = None;
        for _ in 0..32 {
            let temp = self.path.with_extension(format!(
                "json.tmp.{}.{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
            {
                Ok(file) => {
                    reserved = Some((temp, file));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        let (temp, mut file) = reserved.ok_or("could not reserve settings temporary file")?;
        let result = (|| -> std::io::Result<()> {
            file.write_all(&text)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(|error| error.to_string())?;
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
    // Holding the STORE lock across save serializes concurrent writers, so a
    // slower earlier write cannot overwrite a newer one (A13 race).
    store.save(&settings)
}

/// Patch the latest persisted state while holding the writer lock.
pub fn update(edit: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
    let guard = STORE.lock().map_err(|e| e.to_string())?;
    guard
        .as_ref()
        .ok_or("settings store not initialized")?
        .modify(edit)
}

/// Return settings and its read error from one snapshot under the store lock.
pub fn read() -> Result<LoadOutcome, String> {
    let guard = STORE.lock().map_err(|e| e.to_string())?;
    Ok(guard
        .as_ref()
        .ok_or("settings store not initialized")?
        .load_outcome())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_preserve_other_fields_and_corrupt_files() {
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-patch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SettingsStore::new(dir.clone());
        store.save(&Settings::default()).unwrap();
        store.modify(|s| s.language = "en".into()).unwrap();
        store.modify(|s| s.foreground_interval_ms = 2000).unwrap();
        assert_eq!(store.load().language, "en");
        store.modify(|s| s.language = "zh".into()).unwrap();
        assert_eq!(store.load().foreground_interval_ms, 2000);
        assert!(store.modify(|s| s.language = "invalid".into()).is_err());
        assert_eq!(store.load().language, "zh");
        std::fs::write(dir.join("settings.json"), "broken").unwrap();
        assert!(store.modify(|s| s.language = "en".into()).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("settings.json")).unwrap(),
            "broken"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn defaults_are_valid() {
        Settings::default().validate().unwrap();
    }

    #[test]
    fn rejects_out_of_range_interval() {
        let mut s = Settings {
            foreground_interval_ms: 10,
            ..Default::default()
        };
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
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SettingsStore::new(dir.clone());
        let s = Settings {
            foreground_interval_ms: 2000,
            ..Default::default()
        };
        store.save(&s).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.foreground_interval_ms, 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ not json").unwrap();
        let store = SettingsStore::new(dir.clone());
        let loaded = store.load();
        assert_eq!(loaded.foreground_interval_ms, 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- R9/A13: load-time validation, migration, and bad-file preservation ----

    fn write_and_load(dir: &std::path::Path, body: &str) -> LoadOutcome {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("settings.json"), body).unwrap();
        SettingsStore::new(dir.to_path_buf()).load_outcome()
    }

    #[test]
    fn absent_file_is_defaults_without_error() {
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-absent-{}", std::process::id()));
        let store = SettingsStore::new(dir.clone());
        let o = store.load_outcome();
        assert!(
            o.error.is_none(),
            "absent file is a first run, not an error"
        );
        assert_eq!(o.settings.foreground_interval_ms, 1000);
    }

    #[test]
    fn future_version_is_rejected_not_loaded() {
        // A13 repro: version=999 must NOT be loaded as-is.
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-futv-{}", std::process::id()));
        let o = write_and_load(
            &dir,
            r#"{"version":999,"foreground_interval_ms":1000,"background_interval_ms":3000,"launch_at_login":false,"language":"system"}"#,
        );
        assert!(o.error.is_some(), "future version rejected");
        assert_eq!(
            o.settings.version, SETTINGS_VERSION,
            "fell back to defaults"
        );
    }

    #[test]
    fn zero_intervals_rejected_not_loaded() {
        // A13 repro: 0 intervals were previously returned by load().
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-zero-{}", std::process::id()));
        let o = write_and_load(
            &dir,
            r#"{"version":1,"foreground_interval_ms":0,"background_interval_ms":0,"launch_at_login":false,"language":"system"}"#,
        );
        assert!(o.error.is_some());
        assert_eq!(o.settings.foreground_interval_ms, 1000);
    }

    #[test]
    fn invalid_language_rejected() {
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-lang-{}", std::process::id()));
        let o = write_and_load(
            &dir,
            r#"{"version":1,"foreground_interval_ms":1000,"background_interval_ms":3000,"launch_at_login":false,"language":"klingon"}"#,
        );
        assert!(o.error.is_some());
        assert_eq!(o.settings.language, "system");
    }

    #[test]
    fn bad_file_is_preserved_not_overwritten() {
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-keep-{}", std::process::id()));
        let original = r#"{"version":999,"foreground_interval_ms":0}"#;
        let o = write_and_load(&dir, original);
        assert!(o.error.is_some());
        // The on-disk file must be untouched so the user can inspect/recover it.
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert_eq!(
            on_disk, original,
            "bad file preserved, not silently overwritten"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_rejects_invalid_without_touching_disk() {
        let dir =
            std::env::temp_dir().join(format!("monitor-settings-saverej-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SettingsStore::new(dir.clone());
        let bad = Settings {
            foreground_interval_ms: 1,
            ..Default::default()
        };
        assert!(store.save(&bad).is_err());
        assert!(
            !dir.join("settings.json").exists(),
            "invalid save wrote nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
