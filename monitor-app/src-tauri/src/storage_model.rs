//! Shared storage DTOs; Windows topology enrichment is a later stage.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub id: String,
    pub name: String,
    /// Primary role label (first of `roles`, or the legacy single `Role`).
    /// Kept for display; `roles` carries the full set so multi-role volumes
    /// (e.g. Data+System) are not collapsed to one string (A09).
    pub role: String,
    /// All APFS roles reported for this volume. Empty when none reported.
    pub roles: Vec<String>,
    /// Bytes this volume actually consumes within the shared container.
    pub capacity_consumed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub container_ref: String,
    /// R7/A09: EVERY physical store backing this container, in reported order.
    /// A single-store container has one entry; a multi-disk shared pool has
    /// several. Never truncated to the first store.
    pub physical_stores: Vec<String>,
    /// True when this container's capacity is shared across more than one
    /// physical disk. Such a pool's capacity must be counted once and must NOT
    /// be attributed to any single disk (A09).
    pub shared_pool: bool,
    pub capacity_ceiling: Option<u64>,
    pub capacity_free: Option<u64>,
    pub capacity_in_use: Option<u64>,
    pub volumes: Vec<VolumeInfo>,
}

#[cfg(target_os = "macos")]
impl ContainerInfo {
    /// Primary physical store (first backing store), if any. Convenience for
    /// single-store containers; multi-store pools still keep the full list in
    /// `physical_stores` and must not be reduced to this for capacity.
    pub fn primary_store(&self) -> Option<&str> {
        self.physical_stores.first().map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalDisk {
    /// Current enumeration address (e.g. "disk0"). Volatile across reboots and
    /// hot-plug; used only for live addressing and command arguments (A10).
    pub device: String,
    pub name: String,
    pub size_bytes: u64,
    pub smart_status: String,
    pub temperature_celsius: Option<f32>,
    pub power_on_hours: Option<u64>,
    /// APFS containers wholly backed by this disk (single-store pools whose
    /// one physical store lives here). Shared multi-disk pools are NOT placed
    /// here; see `shared_containers` on the topology (A09).
    pub containers: Vec<ContainerInfo>,
    /// R7/A10: stable anonymous identity + generation. `None` until assigned
    /// by the device-identity registry (see `device_id`); live display works
    /// without it, history keying requires it.
    #[serde(default)]
    pub device_uid: Option<String>,
    /// Connection/generation counter for `device_uid`. Bumped when the same
    /// `device` address is re-seen backed by a different medium after hot-plug,
    /// so a reused `diskN` never silently joins the previous owner's series.
    #[serde(default)]
    pub generation: u32,
}

/// Full storage topology: per-disk single-store containers plus any shared
/// multi-disk pools, which are surfaced separately so their capacity is never
/// double-counted or mis-attributed to one disk.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTopology {
    pub disks: Vec<PhysicalDisk>,
    /// Containers spanning more than one physical disk (shared pools).
    /// Capacity counted once here; not present in any disk's `containers`.
    pub shared_containers: Vec<ContainerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskHistorySeries {
    pub uid: String,
    pub name: String,
    pub size_bytes: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsDisk {
    pub number: u32,
    pub name: String,
    pub size_bytes: u64,
    pub bus_type: u16,
    pub kind: String,
    pub device_uid: Option<String>,
    pub identity_state: crate::model::SourceState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsVolume {
    pub alias: String,
    pub drive_letter: Option<String>,
    pub size_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub disk_numbers: Vec<u32>,
    pub mapping_state: crate::model::SourceState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsStorageSnapshot {
    pub disks: Vec<WindowsDisk>,
    pub volumes: Vec<WindowsVolume>,
    pub volume_state: crate::model::SourceState,
    pub history_series: Vec<DiskHistorySeries>,
    pub registry_state: crate::model::SourceState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsDiskRate {
    pub number: u32,
    pub device_uid: Option<String>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub state: crate::model::SourceState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsThroughput {
    pub disks: Vec<WindowsDiskRate>,
    pub unmapped_instances: usize,
}
