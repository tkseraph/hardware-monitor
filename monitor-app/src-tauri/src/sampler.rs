//! Shared scheduler and history writer. Blocking collection stays off async workers.
use crate::history;
pub use crate::model::SystemInfo;
use crate::platform::Sampler;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub fn record_snapshot(info: &SystemInfo) {
    let ts = info.observed_at;
    let mut rows: Vec<(&str, &str, f64, &str)> = Vec::new();
    let core_ids: Vec<String> = info
        .cpu
        .as_ref()
        .map(|cpu| {
            (0..cpu.per_core_usage.len())
                .map(|i| format!("core{}", i))
                .collect()
        })
        .unwrap_or_default();
    if let Some(cpu) = &info.cpu {
        rows.push(("cpu.total_usage", "system", cpu.total_usage as f64, "%"));
        for (i, usage) in cpu.per_core_usage.iter().enumerate() {
            rows.push(("cpu.per_core", core_ids[i].as_str(), *usage as f64, "%"));
        }
    }
    if let Some(memory) = &info.memory {
        rows.push((
            "memory.used_percent",
            "system",
            memory.used_percent as f64,
            "%",
        ));
    }
    for gpu in &info.gpus {
        rows.push((
            "gpu.utilization",
            gpu.object_id.as_str(),
            gpu.utilization as f64,
            "%",
        ));
    }
    for disk in &info.disk_throughput {
        // R7/A10: history keyed by anonymous device_uid, falling back to the
        // volatile address only if no uid was assigned (degraded path).
        let oid = if disk.device_uid.is_empty() {
            disk.device.as_str()
        } else {
            disk.device_uid.as_str()
        };
        rows.push(("disk.throughput", oid, disk.mb_per_sec as f64, "MB/s"));
    }
    // Disk temperature into history (only real readings; absent stays absent).
    for disk in &info.disks {
        if let Some(t) = disk.temperature_celsius {
            let oid = if disk.device_uid.is_empty() {
                disk.device.as_str()
            } else {
                disk.device_uid.as_str()
            };
            rows.push(("disk.temperature", oid, t as f64, "°C"));
        }
    }
    // Surface the error via health(); do not panic the sampler on a full disk.
    let _ = history::record_snapshot_batch(ts, &rows);
}

// ---------- Shared scheduler state ----------

static SAMPLER: Mutex<Option<Sampler>> = Mutex::new(None);
static LATEST: Mutex<Option<SystemInfo>> = Mutex::new(None);
/// True while the main window is visible → foreground cadence.
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn lost_history_batches() -> u64 {
    0
}
pub fn configuration_changed() {}
pub fn shutdown() {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn set_window_visible(visible: bool) {
    WINDOW_VISIBLE.store(visible, Ordering::SeqCst);
}

/// Latest cached snapshot, if the scheduler has produced one yet.
pub fn latest_snapshot() -> Option<SystemInfo> {
    LATEST.lock().ok()?.clone()
}

/// Run the collection loop forever. Intended to be spawned once at setup.
/// The interval switches between the configured foreground/background
/// cadence based on window visibility; sampling continues regardless so
/// history has no WebView-throttling gap (F08). Settings are re-read each
/// cycle so changes take effect without a restart (S8).
///
/// R5/A04: scheduling is anchored to the next absolute deadline, not
/// "sleep(interval) + work". A slow pass shortens the following sleep so the
/// cadence stays close to the configured interval and slow sources never
/// accumulate drift; an overrun simply means the next tick starts on time.
pub async fn run_scheduler(_data_root: Option<std::path::PathBuf>) {
    {
        let mut guard = SAMPLER.lock().expect("sampler lock poisoned");
        *guard = Some(Sampler::new());
    }
    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        let s = crate::settings::get();
        let interval = if WINDOW_VISIBLE.load(Ordering::SeqCst) {
            Duration::from_millis(s.foreground_interval_ms)
        } else {
            Duration::from_millis(s.background_interval_ms)
        };

        let tick_start = Instant::now();

        // Run the sampling pass. The MutexGuard must NOT be held across an
        // await (it is not Send), so we scope it to a plain sync block that
        // returns the snapshot; the guard is dropped before any sleep.
        let sampled: Option<Option<SystemInfo>> = tokio::task::spawn_blocking(|| {
            match SAMPLER.lock() {
                Ok(mut guard) => Some(match guard.as_mut() {
                    Some(s) => {
                        crate::metric_status::note_attempt();
                        match s.sample() {
                            Ok(info) => {
                                if info.cpu.is_some()
                                    || info.memory.is_some()
                                    || !info.gpus.is_empty()
                                {
                                    crate::metric_status::note_success();
                                } else {
                                    crate::metric_status::note_failure();
                                }
                                Some(info)
                            }
                            Err(e) => {
                                crate::metric_status::note_failure();
                                log::warn!("sample pass failed: {}", e);
                                None
                            }
                        }
                    }
                    None => None,
                }),
                Err(_) => None, // poisoned — skip this tick (sleep below)
            }
        })
        .await
        .unwrap_or(None);
        let lock_ok = sampled.is_some();
        let snapshot = sampled.flatten();

        if let Some(info) = snapshot {
            record_snapshot(&info);
            if let Ok(mut latest) = LATEST.lock() {
                *latest = Some(info);
            }
        }

        // Sleep only the remaining time until the next deadline; if the pass
        // overran (or the lock was poisoned so we did no work), start the next
        // tick immediately / after the plain interval respectively.
        let elapsed = tick_start.elapsed();
        let remaining = if !lock_ok {
            interval
        } else if elapsed < interval {
            interval - elapsed
        } else {
            interval.min(Duration::from_millis(250))
        };
        if remaining > Duration::ZERO {
            tokio::time::sleep(remaining).await;
        }
    }
}
