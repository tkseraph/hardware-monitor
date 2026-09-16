//! Shared snapshot contract. Missing sources never become synthetic zeroes.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub name: String,
    pub physical_cores: Option<u32>,
    pub logical_cores: u32,
    pub total_usage: f32,
    pub per_core_usage: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub object_id: String,
    pub name: String,
    pub utilization: f32,
    pub memory_used_bytes: u64,
    pub memory_allocated_bytes: Option<u64>,
    #[serde(default)]
    pub windows_memory: Option<WindowsGpuMemory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsGpuMemory {
    pub dedicated_used_bytes: u64,
    pub shared_used_bytes: u64,
    pub unverified_adapter_count: usize,
    pub history_series: Vec<crate::storage_model::DiskHistorySeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Current enumeration address (e.g. "disk0") — volatile, display/live only.
    pub device: String,
    /// R7/A10: stable anonymous identity used as the history object_id, so a
    /// reused `diskN` never joins a previous disk's series. Empty until the
    /// identity registry assigns one; falls back to `device` for display.
    #[serde(default)]
    pub device_uid: String,
    /// Generation of `device_uid`; bumped when the same `device` address is
    /// re-seen as a physically different medium (hot-plug) (A10).
    #[serde(default)]
    pub generation: u32,
    pub name: String,
    pub size_bytes: u64,
    pub smart_status: String,
    pub temperature_celsius: Option<f32>,
    pub power_on_hours: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskThroughput {
    /// Current enumeration address (live label / command arg).
    pub device: String,
    /// R7/A10: stable anonymous identity used as the history object_id.
    #[serde(default)]
    pub device_uid: String,
    pub mb_per_sec: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu: Option<CpuInfo>,
    pub memory: Option<MemoryInfo>,
    pub gpus: Vec<GpuInfo>,
    pub source_states: std::collections::BTreeMap<String, SourceState>,
    #[serde(default)]
    pub source_meta: std::collections::BTreeMap<String, SourceMeta>,
    #[serde(default)]
    pub windows_storage: Option<crate::storage_model::WindowsStorageSnapshot>,
    #[serde(default)]
    pub windows_disk_throughput: Option<crate::storage_model::WindowsThroughput>,
    pub disks: Vec<DiskInfo>,
    pub disk_throughput: Vec<DiskThroughput>,
    /// Full physical-disk → container → volume topology (S5).
    pub storage: Vec<crate::storage_model::PhysicalDisk>,
    /// Unix seconds when this snapshot was sampled — not when it was read.
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Ok,
    WarmingUp,
    NotImplemented,
    Unverified,
    Unsupported,
    NotApplicable,
    PermissionRequired,
    Error,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceMeta {
    pub observed_at: Option<i64>,
    pub age_ms: Option<u64>,
    pub expected_interval_ms: u64,
    pub sequence: u64,
}
