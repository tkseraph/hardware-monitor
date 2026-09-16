import type { WindowsStorageSnapshot, WindowsThroughput } from './windows-storage';
import type { SourceState } from './source-state';

export interface CpuInfo {
  name: string;
  physical_cores: number | null;
  logical_cores: number;
  total_usage: number;
  per_core_usage: number[];
}

export interface MemoryInfo {
  total_bytes: number;
  used_bytes: number;
  available_bytes: number;
  used_percent: number;
}

export interface GpuInfo {
  object_id: string;
  name: string;
  utilization: number;
  memory_used_bytes: number;
  memory_allocated_bytes: number | null;
  windows_memory?: { dedicated_used_bytes: number; shared_used_bytes: number; unverified_adapter_count: number; history_series: import("./windows-storage").DiskHistorySeries[] } | null;
}

export interface DiskInfo {
  device: string;
  /** Stable anonymous history identity (A10). */
  device_uid: string;
  generation: number;
  name: string;
  size_bytes: number;
  smart_status: string;
  temperature_celsius: number | null;
  power_on_hours: number | null;
}

export interface DiskThroughput {
  device: string;
  /** Stable anonymous history identity (A10). */
  device_uid: string;
  mb_per_sec: number;
}

export interface ProcessInfo {
  pid: number;
  start_marker: string | null;
  name: string;
  memory_bytes: number | null;
  cpu_usage: number | null;
  disk_read_bytes: number;
  disk_write_bytes: number;
  disk_read_bps: number | null;
  disk_write_bps: number | null;
  /** R6: false when I/O counters were unreadable; do not render the 0s. */
  io_ok: boolean;
}

export interface ProcessPage {
  processes: ProcessInfo[];
  total_readable: number;
  /** R6: rows matching the filter before pagination — real page count. */
  matched_total: number;
  offset: number;
  limit: number;
  observed_at: number;
}

export type ProcessSortKey = "memory" | "cpu" | "diskread" | "diskwrite";

export interface VolumeInfo {
  id: string;
  name: string;
  role: string;
  /** All APFS roles (multi-role volumes are not collapsed to one string). */
  roles: string[];
  capacity_consumed: number | null;
}

export interface ContainerInfo {
  container_ref: string;
  /** R7/A09: EVERY backing physical store, never truncated to the first. */
  physical_stores: string[];
  /** True when capacity is shared across more than one physical disk. */
  shared_pool: boolean;
  capacity_ceiling: number | null;
  capacity_free: number | null;
  capacity_in_use: number | null;
  volumes: VolumeInfo[];
}

export interface PhysicalDisk {
  /** Volatile enumeration address (e.g. disk0); live display only (A10). */
  device: string;
  /** Stable anonymous history identity (A10); falls back to device if absent. */
  device_uid: string | null;
  /** Generation of device_uid; bumps on hot-plug address reuse (A10). */
  generation: number;
  name: string;
  size_bytes: number;
  smart_status: string;
  temperature_celsius: number | null;
  power_on_hours: number | null;
  containers: ContainerInfo[];
}

export interface SystemInfo {
  windows_storage?: WindowsStorageSnapshot | null;
  windows_disk_throughput?: WindowsThroughput | null;
  cpu: CpuInfo | null;
  memory: MemoryInfo | null;
  gpus: GpuInfo[];
  source_states: Record<string, SourceState>;
  source_meta?: Record<string, { observed_at: number | null; age_ms: number | null; expected_interval_ms: number; sequence: number }>;
  disks: DiskInfo[];
  disk_throughput: DiskThroughput[];
  /** Full physical-disk → container → volume topology. */
  storage: PhysicalDisk[];
  /** Unix seconds when the snapshot was sampled by the Rust scheduler. */
  observed_at: number;
}

/** R4: sampling + history health from get_system_status. */
export interface SamplingHealth {
  last_success_at: number | null;
  success_age_secs: number | null;
  consecutive_failures: number;
  ever_succeeded: boolean;
}
export interface SystemStatus {
  sampling: SamplingHealth;
  history_lost_batches: number;
  history_health: "ok" | "over_budget" | "write_error" | "unavailable";
}

