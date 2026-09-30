//! Windows source supervision. The IPC reads only cached values.
use crate::model::SourceState;
pub use crate::model::SystemInfo;
use crate::platform::Sampler;
use crate::source_runtime::{SourceId, SourceRuntime, SourceSpec, SourceValue};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

static DISK_BINDINGS: Mutex<Option<crate::platform::windows::storage::Bindings>> = Mutex::new(None);
static RUNTIME: Mutex<Option<SourceRuntime>> = Mutex::new(None);
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
pub fn window_visible() -> bool {
    WINDOW_VISIBLE.load(Ordering::SeqCst)
}

pub fn effective_interval_ms() -> u64 {
    let settings = crate::settings::get();
    if WINDOW_VISIBLE.load(Ordering::SeqCst) {
        settings.foreground_interval_ms
    } else {
        settings.background_interval_ms
    }
}
pub fn set_window_visible(visible: bool) {
    WINDOW_VISIBLE.store(visible, Ordering::SeqCst);
    configuration_changed();
}
pub fn configuration_changed() {
    if let Some(runtime) = RUNTIME.lock().unwrap().as_ref() {
        runtime.set_interval(Duration::from_millis(effective_interval_ms()));
    }
}
pub fn latest_snapshot() -> Option<SystemInfo> {
    RUNTIME.lock().ok()?.as_ref().map(SourceRuntime::snapshot)
}
pub fn lost_history_batches() -> u64 {
    RUNTIME
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, SourceRuntime::lost_history_batches)
}
pub struct HistoryTarget {
    pub number: u32,
    pub uid: String,
    pub name: String,
    pub size_bytes: u64,
    pub lease: std::sync::Arc<crate::platform::windows::storage_native::DiskLease>,
}
pub fn temperature_targets() -> Vec<HistoryTarget> {
    let Some(bindings) = DISK_BINDINGS.lock().unwrap().as_ref().cloned() else {
        return vec![];
    };
    let Some(live) = bindings.lock().unwrap().clone() else {
        return vec![];
    };
    if live.observed.elapsed() > live.ttl {
        return vec![];
    }
    let Some(storage) = latest_snapshot().and_then(|s| s.windows_storage) else {
        return vec![];
    };
    let mut targets = Vec::new();
    for d in storage.disks {
        if d.kind != "physical" {
            continue;
        }
        let Some(uid) = d.device_uid else {
            continue;
        };
        if let Some(lease) = live
            .disks
            .iter()
            .find(|b| b.number == d.number && b.uid.as_ref() == Some(&uid))
            .and_then(|b| b.lease.clone())
            .filter(|l| l.valid())
        {
            targets.push(HistoryTarget {
                number: d.number,
                uid,
                name: d.name,
                size_bytes: d.size_bytes,
                lease,
            });
        }
    }
    targets
}

pub fn shutdown() {
    crate::enhanced::gpu_temperature::stop();
    let runtime = RUNTIME.lock().unwrap().take();
    if let Some(runtime) = runtime {
        let pending = runtime.shutdown(Duration::from_secs(2));
        if pending > 0 {
            log::warn!("{pending} source/history workers did not return before shutdown deadline");
        }
    }
}

pub async fn run_scheduler(data_root: Option<std::path::PathBuf>) {
    let mut active = RUNTIME.lock().unwrap();
    if active.is_some() {
        return;
    }
    let mut specs: Vec<SourceSpec> = [SourceId::Cpu, SourceId::Memory]
        .into_iter()
        .map(|id| SourceSpec {
            id,
            minimum_interval: sysinfo::MINIMUM_CPU_UPDATE_INTERVAL,
            create: Box::new(move || {
                let mut sampler = Sampler::new();
                Box::new(move || {
                    crate::metric_status::note_attempt();
                    let result = match id {
                        SourceId::Cpu => sampler.cpu().map(SourceValue::Cpu),
                        SourceId::Memory => sampler.memory().map(SourceValue::Memory),
                        _ => Err(SourceState::NotImplemented),
                    };
                    match &result {
                        Ok(_) => crate::metric_status::note_success(),
                        Err(SourceState::WarmingUp) => {}
                        Err(_) => crate::metric_status::note_failure(),
                    }
                    result
                })
            }),
        })
        .collect();
    if let Some(root) = data_root {
        let gpu_root = root.clone();
        specs.push(SourceSpec {
            id: SourceId::Gpu,
            minimum_interval: Duration::from_millis(500),
            create: Box::new(move || {
                let mut collector = crate::platform::windows::gpu::GpuCollector::new(gpu_root);
                Box::new(move || collector.collect().map(SourceValue::Gpu))
            }),
        });
        crate::enhanced::gpu_temperature::start();
        specs.push(SourceSpec {
            id: SourceId::GpuTemperature,
            minimum_interval: Duration::from_secs(2),
            create: Box::new(|| {
                let mut collector = crate::enhanced::gpu_temperature::Collector::default();
                Box::new(move || collector.collect().map(SourceValue::GpuTemperature))
            }),
        });
        let bindings = std::sync::Arc::new(Mutex::new(None));
        *DISK_BINDINGS.lock().unwrap() = Some(bindings.clone());
        let storage_bindings = bindings.clone();
        specs.push(SourceSpec {
            id: SourceId::Storage,
            minimum_interval: Duration::from_secs(5),
            create: Box::new(move || {
                let mut storage = crate::platform::windows::storage::StorageCollector::new(
                    root,
                    storage_bindings,
                );
                Box::new(move || storage.collect().map(SourceValue::Storage))
            }),
        });
        specs.push(SourceSpec {
            id: SourceId::DiskThroughput,
            minimum_interval: Duration::from_millis(500),
            create: Box::new(move || {
                let mut collector =
                    crate::platform::windows::disk_performance::DiskPerformance::new(bindings);
                Box::new(move || collector.collect().map(SourceValue::DiskThroughput))
            }),
        });
    }
    let mut continuity = crate::source_runtime::HistoryContinuity::default();
    match SourceRuntime::start(
        specs,
        Duration::from_millis(effective_interval_ms()),
        16,
        Box::new(move |batch| {
            let rows: Vec<_> = batch
                .rows
                .iter()
                .map(|row| (row.metric, row.object.as_str(), row.value, row.unit))
                .collect();
            let segment = continuity.segment(&batch);
            crate::history::record_segment_batch(batch.observed_at, &rows, Some(&segment))
                .map_err(|_| ())?;
            continuity.saved(&batch, segment);
            Ok(())
        }),
    ) {
        Ok(runtime) => *active = Some(runtime),
        Err(error) => {
            crate::metric_status::note_failure();
            log::error!("could not start source workers: {error}");
        }
    }
}
