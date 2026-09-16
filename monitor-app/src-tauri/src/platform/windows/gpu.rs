//! DXGI inventory + global WDDM counters. No process-budget API or engine clamping.
use super::device_registry::{Descriptor, IdentityInput, Registry};
use super::disk_performance::counter_values;
use crate::model::{GpuInfo, SourceState, WindowsGpuMemory};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows_sys::Win32::System::Performance::*;

type Key = (u32, u32, u32);
fn key(name: &str) -> Option<Key> {
    let fields: Vec<_> = name.split('_').collect();
    let i = fields.iter().position(|v| *v == "luid")?;
    if *fields.get(i + 3)? != "phys" {
        return None;
    }
    Some((
        u32::from_str_radix(fields.get(i + 1)?.strip_prefix("0x")?, 16).ok()?,
        u32::from_str_radix(fields.get(i + 2)?.strip_prefix("0x")?, 16).ok()?,
        fields.get(i + 4)?.parse().ok()?,
    ))
}
fn busiest(rows: &[(String, Option<f64>)], adapter: Key) -> Option<f32> {
    let mut engines = BTreeMap::<u32, f64>::new();
    let mut seen = HashSet::new();
    for (name, value) in rows.iter().filter(|(n, _)| key(n) == Some(adapter)) {
        if !seen.insert(name) {
            return None;
        }
        let value = (*value)?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        let fields: Vec<_> = name.split('_').collect();
        let i = fields.iter().position(|v| *v == "eng")?;
        let engine = fields.get(i + 1)?.parse::<u32>().ok()?;
        *engines.entry(engine).or_default() += value;
    }
    let peak = engines.values().copied().reduce(f64::max)?;
    (peak.is_finite() && (0.0..=100.0).contains(&peak)).then_some(peak as f32)
}
fn memory(rows: &[(String, Option<f64>)], adapter: Key) -> Option<u64> {
    let values: Vec<_> = rows
        .iter()
        .filter(|(name, _)| key(name) == Some(adapter))
        .collect();
    if values.len() != 1 {
        return None;
    }
    let value = values[0].1?;
    (value.is_finite() && (0.0..=9_007_199_254_740_991.0).contains(&value) && value.fract() == 0.0)
        .then_some(value as u64)
}
struct Query {
    handle: PDH_HQUERY,
    counters: [PDH_HCOUNTER; 3],
}
impl Drop for Query {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                PdhCloseQuery(self.handle);
            }
        }
    }
}
impl Query {
    fn new() -> Result<Self, SourceState> {
        let mut query = Self {
            handle: std::ptr::null_mut(),
            counters: [std::ptr::null_mut(); 3],
        };
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query.handle) } != 0 {
            return Err(SourceState::Error);
        }
        for (index, path) in [
            "\\GPU Engine(*)\\Utilization Percentage",
            "\\GPU Adapter Memory(*)\\Dedicated Usage",
            "\\GPU Adapter Memory(*)\\Shared Usage",
        ]
        .iter()
        .enumerate()
        {
            let path: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
            if unsafe {
                PdhAddEnglishCounterW(query.handle, path.as_ptr(), 0, &mut query.counters[index])
            } != 0
            {
                return Err(SourceState::Unverified);
            }
        }
        unsafe {
            PdhCollectQueryData(query.handle);
        }
        Ok(query)
    }
}
struct Adapter {
    ordinal: u32,
    key: Key,
    name: String,
    capacity_hint: u64,
}
pub struct GpuCollector {
    root: PathBuf,
    registry: Option<Registry>,
    factory: Option<IDXGIFactory1>,
    adapters: Vec<Adapter>,
    query: Option<Query>,
    discovered: Option<Instant>,
    previous: BTreeMap<u32, Key>,
    last_poll: Option<Instant>,
}
impl GpuCollector {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            registry: None,
            factory: None,
            adapters: Vec::new(),
            query: None,
            discovered: None,
            previous: BTreeMap::new(),
            last_poll: None,
        }
    }
    fn discover(&mut self) -> Result<(), SourceState> {
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.map_err(|_| SourceState::Error)?;
        let mut adapters = Vec::new();
        for ordinal in 0..32 {
            let adapter = match unsafe { factory.EnumAdapters1(ordinal) } {
                Ok(v) => v,
                Err(e) if e.code().0 as u32 == 0x887a0002 => break,
                Err(_) => return Err(SourceState::Error),
            };
            let desc = unsafe { adapter.GetDesc1() }.map_err(|_| SourceState::Error)?;
            if desc.Flags & 2 != 0 {
                continue;
            }
            let len = desc
                .Description
                .iter()
                .position(|&v| v == 0)
                .unwrap_or(desc.Description.len());
            adapters.push(Adapter {
                ordinal,
                key: (
                    desc.AdapterLuid.HighPart as u32,
                    desc.AdapterLuid.LowPart,
                    0,
                ),
                name: String::from_utf16_lossy(&desc.Description[..len]),
                capacity_hint: desc.DedicatedVideoMemory.max(desc.SharedSystemMemory) as u64,
            });
        }
        self.factory = Some(factory);
        self.adapters = adapters;
        self.discovered = Some(Instant::now());
        Ok(())
    }
    pub fn collect(&mut self) -> Result<Vec<GpuInfo>, SourceState> {
        let changed = self
            .factory
            .as_ref()
            .is_none_or(|f| !unsafe { f.IsCurrent() }.as_bool());
        if changed {
            self.previous.clear();
            self.query = None;
        }
        if changed
            || self
                .discovered
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
        {
            self.discover()?;
        }
        let limit = (Duration::from_millis(crate::sampler::effective_interval_ms()) * 3)
            .max(Duration::from_secs(15));
        if self.last_poll.is_some_and(|t| t.elapsed() > limit) {
            self.query = None;
        }
        if self.query.is_none() {
            self.query = Some(Query::new()?);
            self.last_poll = Some(Instant::now());
            return Err(SourceState::WarmingUp);
        }
        let query = self.query.as_ref().unwrap();
        self.last_poll = Some(Instant::now());
        if unsafe { PdhCollectQueryData(query.handle) } != 0 {
            return Err(SourceState::Error);
        }
        let engines = counter_values(query.counters[0])?;
        let dedicated = counter_values(query.counters[1])?;
        let shared = counter_values(query.counters[2])?;
        let mut matched = Vec::new();
        let mut inputs = Vec::new();
        for adapter in &self.adapters {
            let (Some(util), Some(ded), Some(sh)) = (
                busiest(&engines, adapter.key),
                memory(&dedicated, adapter.key),
                memory(&shared, adapter.key),
            ) else {
                continue;
            };
            if adapter.capacity_hint == 0 {
                continue;
            }
            inputs.push(IdentityInput {
                number: adapter.ordinal,
                descriptor: Descriptor {
                    name: adapter.name.clone(),
                    size_bytes: adapter.capacity_hint,
                },
                continuous: self.previous.get(&adapter.ordinal) == Some(&adapter.key),
            });
            matched.push((adapter, util, ded, sh));
        }
        #[cfg(test)]
        {
            let outside_active = engines
                .iter()
                .filter(|(name, value)| {
                    value.is_some_and(|v| v > 0.0)
                        && matched.iter().any(|(a, _, _, _)| key(name) == Some(a.key))
                        && name
                            .strip_prefix("pid_")
                            .and_then(|n| n.split('_').next())
                            .and_then(|n| n.parse::<u32>().ok())
                            .is_some_and(|pid| pid > 0 && pid != std::process::id())
                })
                .count();
            println!("other_active_engine_instances={outside_active}");
        }
        if matched.is_empty() {
            return Err(SourceState::Unverified);
        }
        if self.registry.is_none() {
            self.registry = Some(Registry::open_gpu(&self.root).map_err(|_| SourceState::Error)?);
        }
        let registry = self.registry.as_mut().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| SourceState::Error)?
            .as_secs() as i64;
        let ids = registry
            .reconcile(&inputs, now)
            .map_err(|_| SourceState::Error)?;
        let archives = registry.records();
        let unmatched = self.adapters.len() - matched.len();
        let mut result = Vec::new();
        self.previous.clear();
        for (adapter, util, ded, sh) in matched {
            self.previous.insert(adapter.ordinal, adapter.key);
            result.push(GpuInfo {
                object_id: ids[&adapter.ordinal].clone(),
                name: adapter.name.clone(),
                utilization: util,
                memory_used_bytes: ded.checked_add(sh).ok_or(SourceState::Error)?,
                memory_allocated_bytes: None,
                windows_memory: Some(WindowsGpuMemory {
                    dedicated_used_bytes: ded,
                    shared_used_bytes: sh,
                    unverified_adapter_count: unmatched,
                    history_series: archives.clone(),
                }),
            });
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn name(pid: u32, engine: u32) -> String {
        format!("pid_{pid}_luid_0x00000000_0x00000001_phys_0_eng_{engine}_engtype_3D")
    }
    #[test]
    fn engine_aggregation_is_per_adapter_and_never_clamped() {
        let rows = vec![
            (name(1, 0), Some(20.0)),
            (name(2, 0), Some(30.0)),
            (name(3, 1), Some(40.0)),
        ];
        assert_eq!(busiest(&rows, (0, 1, 0)), Some(50.0));
        assert_eq!(busiest(&rows, (0, 2, 0)), None);
        assert_eq!(busiest(&[(name(1, 0), Some(110.0))], (0, 1, 0)), None);
        assert_eq!(busiest(&[(name(1, 0), Some(0.0))], (0, 1, 0)), Some(0.0));
        assert_eq!(busiest(&[(name(1, 0), None)], (0, 1, 0)), None);
    }
    #[test]
    fn global_memory_requires_one_valid_adapter_counter() {
        let n = "luid_0x00000000_0x00000001_phys_0".to_string();
        assert_eq!(memory(&[(n.clone(), Some(0.0))], (0, 1, 0)), Some(0));
        assert_eq!(
            memory(&[(n.clone(), Some(10.0)), (n, None)], (0, 1, 0)),
            None
        );
    }
    #[test]
    #[ignore = "opt-in ordinary-user global GPU counters; no generated GPU load"]
    fn live_gpu_counters() {
        let root = std::env::temp_dir().join(format!("monitor-gpu-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut collector = GpuCollector::new(root.clone());
        let mut reading = None;
        for _ in 0..4 {
            if let Ok(value) = collector.collect() {
                reading = Some(value);
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let gpus = reading.expect("global GPU counters must be readable");
        println!("mapped_adapters={} utilization={} dedicated_bytes={} shared_bytes={} unmatched_adapters={}",gpus.len(),gpus[0].utilization,gpus[0].windows_memory.as_ref().unwrap().dedicated_used_bytes,gpus[0].windows_memory.as_ref().unwrap().shared_used_bytes,gpus[0].windows_memory.as_ref().unwrap().unverified_adapter_count);
        assert!(!gpus.is_empty());
        drop(collector);
        std::fs::remove_dir_all(root).unwrap();
    }
}
