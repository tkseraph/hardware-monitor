use super::device_registry::{Descriptor, IdentityInput, Registry};
use super::storage_native::{volume_disks, DiskLease};
use crate::model::SourceState;
use crate::storage_model::{WindowsDisk, WindowsStorageSnapshot, WindowsVolume};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wmi::{WMIConnection, WMIError};

#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum Number {
    Integer(u64),
    Text(String),
}
impl Number {
    fn value(&self) -> Option<u64> {
        match self {
            Self::Integer(v) => Some(*v),
            Self::Text(s) => s.parse().ok(),
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskRow {
    number: u32,
    friendly_name: String,
    size: Number,
    bus_type: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VolumeRow {
    path: Option<String>,
    drive_letter: Option<Number>,
    size: Option<Number>,
    size_remaining: Option<Number>,
}

fn query<T: serde::de::DeserializeOwned>(
    conn: &WMIConnection,
    query: &str,
    limit: usize,
) -> Result<Vec<T>, SourceState> {
    let mut rows = Vec::new();
    for item in conn.exec_query(query).map_err(wmi_state)?.take(limit + 1) {
        if rows.len() == limit {
            return Err(SourceState::Error);
        }
        rows.push(item.map_err(wmi_state)?.into_desr().map_err(wmi_state)?);
    }
    Ok(rows)
}
fn wmi_state(error: WMIError) -> SourceState {
    match error {
        WMIError::HResultError { hres } if [0x80070005u32, 0x80041003].contains(&(hres as u32)) => {
            SourceState::PermissionRequired
        }
        _ => SourceState::Error,
    }
}
fn letter(raw: &Option<Number>) -> Option<String> {
    let letter = match raw.as_ref()? {
        Number::Integer(v) => char::from_u32((*v).try_into().ok()?)?,
        Number::Text(v) if v.len() == 1 => v.chars().next()?,
        _ => return None,
    };
    letter
        .is_ascii_alphabetic()
        .then(|| letter.to_ascii_uppercase().to_string())
}
fn kind(bus: u16) -> &'static str {
    match bus {
        3 | 11 | 17 => "physical",
        14..=16 => "virtual",
        _ => "unknown",
    }
}

#[derive(Clone)]
pub struct Binding {
    pub number: u32,
    pub uid: Option<String>,
    pub lease: Option<Arc<DiskLease>>,
}
#[derive(Clone)]
pub struct BindingSnapshot {
    pub disks: Vec<Binding>,
    pub observed: Instant,
    pub ttl: Duration,
}
pub type Bindings = Arc<Mutex<Option<BindingSnapshot>>>;

pub struct StorageCollector {
    root: PathBuf,
    connection: Option<WMIConnection>,
    registry: Option<Registry>,
    rows: Vec<DiskRow>,
    refreshed: Option<Instant>,
    tracked: BTreeMap<u32, (Descriptor, Arc<DiskLease>)>,
    bindings: Bindings,
}
impl StorageCollector {
    pub fn new(root: PathBuf, bindings: Bindings) -> Self {
        Self {
            root,
            connection: None,
            registry: None,
            rows: Vec::new(),
            refreshed: None,
            tracked: BTreeMap::new(),
            bindings,
        }
    }
    pub fn collect(&mut self) -> Result<WindowsStorageSnapshot, SourceState> {
        if self.connection.is_none() {
            self.connection = Some(
                WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage")
                    .map_err(wmi_state)?,
            );
        }
        let conn = self.connection.as_ref().unwrap();
        let invalid_handle = self.tracked.values().any(|(_, lease)| !lease.valid());
        if invalid_handle
            || self
                .refreshed
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
        {
            self.rows = query(
                conn,
                "SELECT Number,FriendlyName,Size,BusType FROM MSFT_Disk",
                128,
            )?;
            self.refreshed = Some(Instant::now());
        }
        let volume_result = query::<VolumeRow>(
            conn,
            "SELECT Path,DriveLetter,Size,SizeRemaining FROM MSFT_Volume",
            512,
        );
        let volume_state = if volume_result.is_ok() {
            SourceState::Ok
        } else {
            volume_result.as_ref().err().copied().unwrap()
        };
        let mut volumes = Vec::new();
        for (index, row) in volume_result.unwrap_or_default().into_iter().enumerate() {
            let mapping = row
                .path
                .as_deref()
                .ok_or(SourceState::Unverified)
                .and_then(volume_disks);
            let size = row.size.as_ref().and_then(Number::value).filter(|v| *v > 0);
            let free = row
                .size_remaining
                .as_ref()
                .and_then(Number::value)
                .filter(|v| size.is_some_and(|s| *v <= s));
            volumes.push(WindowsVolume {
                alias: format!("Volume {}", index + 1),
                drive_letter: letter(&row.drive_letter),
                size_bytes: size,
                free_bytes: free,
                mapping_state: mapping.as_ref().err().copied().unwrap_or(SourceState::Ok),
                disk_numbers: mapping.unwrap_or_default(),
            });
        }
        let mut tracked = BTreeMap::new();
        let mut inputs = Vec::new();
        let mut disks = Vec::new();
        let mut bindings = Vec::new();
        for row in &self.rows {
            let Some(size) = row.size.value().filter(|v| *v > 0) else {
                continue;
            };
            let descriptor = Descriptor {
                name: row.friendly_name.trim().to_string(),
                size_bytes: size,
            };
            let old = self
                .tracked
                .get(&row.number)
                .filter(|(d, h)| d == &descriptor && h.valid());
            let continuous = old.is_some();
            let lease = old
                .map(|(_, h)| h.clone())
                .or_else(|| DiskLease::open(row.number).ok().map(Arc::new));
            let reliable = kind(row.bus_type) == "physical" && lease.is_some();
            if reliable {
                inputs.push(IdentityInput {
                    number: row.number,
                    descriptor: descriptor.clone(),
                    continuous,
                });
            }
            if let Some(lease) = &lease {
                tracked.insert(row.number, (descriptor.clone(), lease.clone()));
            }
            disks.push(WindowsDisk {
                number: row.number,
                name: descriptor.name,
                size_bytes: size,
                bus_type: row.bus_type,
                kind: kind(row.bus_type).into(),
                device_uid: None,
                identity_state: if reliable {
                    SourceState::WarmingUp
                } else {
                    SourceState::Unverified
                },
            });
            bindings.push(Binding {
                number: row.number,
                uid: None,
                lease,
            });
        }
        if self.registry.is_none() {
            self.registry = Registry::open(&self.root).ok();
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| SourceState::Error)?
            .as_secs() as i64;
        let ids = self
            .registry
            .as_mut()
            .ok_or("registry_unavailable")
            .and_then(|r| r.reconcile(&inputs, now));
        let registry_state = if ids.is_ok() {
            SourceState::Ok
        } else {
            SourceState::Error
        };
        if let Ok(ids) = ids {
            for disk in &mut disks {
                disk.device_uid = ids.get(&disk.number).cloned();
                if disk.device_uid.is_some() {
                    disk.identity_state = SourceState::Ok;
                } else if inputs.iter().any(|i| i.number == disk.number) {
                    disk.identity_state = registry_state;
                }
            }
            for binding in &mut bindings {
                binding.uid = ids.get(&binding.number).cloned();
            }
        } else {
            for disk in &mut disks {
                if disk.identity_state == SourceState::WarmingUp {
                    disk.identity_state = SourceState::Error;
                }
            }
        }
        self.tracked = tracked;
        let period = Duration::from_millis(crate::sampler::effective_interval_ms())
            .max(Duration::from_secs(5));
        *self.bindings.lock().unwrap() = Some(BindingSnapshot {
            disks: bindings,
            observed: Instant::now(),
            ttl: period * 3,
        });
        Ok(WindowsStorageSnapshot {
            disks,
            volumes,
            volume_state,
            history_series: self
                .registry
                .as_ref()
                .map_or_else(Vec::new, Registry::records),
            registry_state,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_types_preserve_unknowns_and_classify_virtual_devices() {
        assert_eq!(letter(&Some(Number::Integer(67))), Some("C".into()));
        assert_eq!(letter(&Some(Number::Text("e".into()))), Some("E".into()));
        assert_eq!(letter(&Some(Number::Integer(0))), None);
        assert_eq!(kind(17), "physical");
        assert_eq!(kind(16), "virtual");
        assert_eq!(kind(7), "unknown");
    }
    #[test]
    #[ignore = "opt-in ordinary-user storage probe; registry written only into a new test directory"]
    fn live_storage_topology() {
        let root = std::env::temp_dir().join(format!("monitor-storage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let bindings = Arc::new(Mutex::new(None));
        let mut collector = StorageCollector::new(root.clone(), bindings);
        let snapshot = collector.collect().expect("storage source readable");
        println!(
            "disk_count={} volume_count={} mapped_volumes={} identified_disks={}",
            snapshot.disks.len(),
            snapshot.volumes.len(),
            snapshot
                .volumes
                .iter()
                .filter(|v| !v.disk_numbers.is_empty())
                .count(),
            snapshot
                .disks
                .iter()
                .filter(|d| d.device_uid.is_some())
                .count()
        );
        assert!(!snapshot.disks.is_empty());
        assert!(!snapshot.volumes.is_empty());
        assert!(snapshot
            .volumes
            .iter()
            .any(|v| v.mapping_state == SourceState::Ok && !v.disk_numbers.is_empty()));
        assert_eq!(snapshot.registry_state, SourceState::Ok);
        assert!(snapshot.disks.iter().any(|d| d.device_uid.is_some()));
        drop(collector);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "opt-in ordinary-user disk performance integration; small registry only, no load generation"]
    fn live_storage_throughput() {
        let root = std::env::temp_dir().join(format!("monitor-rates-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let bindings = Arc::new(Mutex::new(None));
        let mut storage = StorageCollector::new(root.clone(), bindings.clone());
        let topology = storage.collect().unwrap();
        let mut performance = super::super::disk_performance::DiskPerformance::new(bindings);
        let mut verified = None;
        for _ in 0..4 {
            if let Ok(rates) = performance.collect() {
                if rates
                    .disks
                    .iter()
                    .any(|d| d.state == SourceState::Ok && d.device_uid.is_some())
                {
                    verified = Some(rates);
                    break;
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let rates = verified.expect("mapped physical disk rates must become readable");
        assert_eq!(rates.disks.len(), topology.disks.len());
        println!(
            "disk_count={} readable_rate_count={} unmapped_instances={}",
            topology.disks.len(),
            rates
                .disks
                .iter()
                .filter(|d| d.state == SourceState::Ok)
                .count(),
            rates.unmapped_instances
        );
        drop(performance);
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
}
