//! One bounded worker per source, a fast cache, and a separate bounded history queue.
//! No source or SQLite call runs while the cache lock is held.
use crate::model::{CpuInfo, GpuInfo, MemoryInfo, SourceMeta, SourceState, SystemInfo};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceId {
    Cpu,
    Memory,
    Gpu,
    Storage,
    DiskThroughput,
}
impl SourceId {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Gpu => "gpu",
            Self::Storage => "storage",
            Self::DiskThroughput => "disk_throughput",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum SourceValue {
    Cpu(CpuInfo),
    Memory(MemoryInfo),
    Gpu(Vec<GpuInfo>),
    Storage(crate::storage_model::WindowsStorageSnapshot),
    DiskThroughput(crate::storage_model::WindowsThroughput),
}
impl SourceValue {
    fn id(&self) -> SourceId {
        match self {
            Self::Cpu(_) => SourceId::Cpu,
            Self::Memory(_) => SourceId::Memory,
            Self::Gpu(_) => SourceId::Gpu,
            Self::Storage(_) => SourceId::Storage,
            Self::DiskThroughput(_) => SourceId::DiskThroughput,
        }
    }
    fn valid(&self) -> bool {
        let percent = |v: f32| v.is_finite() && (0.0..=100.0).contains(&v);
        match self {
            Self::Cpu(v) => {
                v.logical_cores > 0
                    && v.per_core_usage.len() == v.logical_cores as usize
                    && percent(v.total_usage)
                    && v.per_core_usage.iter().all(|&v| percent(v))
            }
            Self::Memory(v) => {
                v.total_bytes > 0
                    && v.available_bytes <= v.total_bytes
                    && v.used_bytes == v.total_bytes - v.available_bytes
                    && percent(v.used_percent)
            }
            Self::Storage(storage) => {
                storage.disks.iter().all(|d| d.size_bytes > 0)
                    && storage.volumes.iter().all(|v| {
                        v.free_bytes
                            .is_none_or(|free| v.size_bytes.is_some_and(|size| free <= size))
                    })
            }
            Self::DiskThroughput(rates) => rates.disks.iter().all(|r| {
                [r.read_bps, r.write_bps]
                    .iter()
                    .all(|value| value.is_none_or(|v| v.is_finite() && v >= 0.0))
            }),
            Self::Gpu(gpus) => {
                !gpus.is_empty()
                    && gpus
                        .iter()
                        .all(|v| !v.object_id.is_empty() && percent(v.utilization))
            }
        }
    }
}

// Created and used on its source thread; COM collectors must not be Send.
pub type Collector = Box<dyn FnMut() -> Result<SourceValue, SourceState>>;
pub struct SourceSpec {
    pub id: SourceId,
    pub minimum_interval: Duration,
    // Factory executes on the source thread too: initialization can block.
    pub create: Box<dyn FnOnce() -> Collector + Send>,
}

#[derive(Debug)]
pub struct HistoryRow {
    pub metric: &'static str,
    pub object: String,
    pub value: f64,
    pub unit: &'static str,
}
#[derive(Debug)]
pub struct HistoryBatch {
    pub source: SourceId,
    pub sequence: u64,
    pub boundary: bool,
    pub observed_at: i64,
    pub rows: Vec<HistoryRow>,
}
pub type HistoryWriter = Box<dyn FnMut(HistoryBatch) -> Result<(), ()> + Send>;

#[derive(Default)]
pub struct HistoryContinuity {
    saved: BTreeMap<SourceId, (u64, String)>,
}
impl HistoryContinuity {
    pub fn segment(&self, batch: &HistoryBatch) -> String {
        match self.saved.get(&batch.source) {
            Some((sequence, segment))
                if !batch.boundary && sequence.checked_add(1) == Some(batch.sequence) =>
            {
                segment.clone()
            }
            _ => uuid::Uuid::new_v4().to_string(),
        }
    }
    pub fn saved(&mut self, batch: &HistoryBatch, segment: String) {
        self.saved.insert(batch.source, (batch.sequence, segment));
    }
}

fn history_batch(value: &SourceValue, observed_at: i64) -> HistoryBatch {
    let mut rows = Vec::new();
    let mut push = |metric, object, value, unit| {
        rows.push(HistoryRow {
            metric,
            object,
            value,
            unit,
        })
    };
    match value {
        SourceValue::Cpu(cpu) => {
            push(
                "cpu.total_usage",
                "system".into(),
                cpu.total_usage as f64,
                "%",
            );
            for (i, value) in cpu.per_core_usage.iter().enumerate() {
                push("cpu.per_core", format!("core{i}"), *value as f64, "%");
            }
        }
        SourceValue::Memory(memory) => push(
            "memory.used_percent",
            "system".into(),
            memory.used_percent as f64,
            "%",
        ),
        SourceValue::Storage(_) => {}
        SourceValue::DiskThroughput(rates) => {
            for rate in &rates.disks {
                if rate.state != SourceState::Ok {
                    continue;
                }
                if let (Some(uid), Some(read), Some(write)) =
                    (&rate.device_uid, rate.read_bps, rate.write_bps)
                {
                    push(
                        "disk.throughput",
                        uid.clone(),
                        (read + write) / 1_000_000.0,
                        "MB/s",
                    );
                }
            }
        }
        SourceValue::Gpu(gpus) => {
            for gpu in gpus {
                push(
                    "gpu.utilization",
                    gpu.object_id.clone(),
                    gpu.utilization as f64,
                    "%",
                );
            }
        }
    }
    HistoryBatch {
        source: value.id(),
        sequence: 0,
        boundary: true,
        observed_at,
        rows,
    }
}

struct Entry {
    state: SourceState,
    value: Option<SourceValue>,
    observed_at: Option<i64>,
    updated: Instant,
    last_success: Option<Instant>,
    sequence: u64,
    minimum_interval: Duration,
}
struct Cache {
    entries: BTreeMap<SourceId, Entry>,
}
impl Cache {
    fn new(specs: &[SourceSpec], now: Instant) -> Self {
        Self {
            entries: specs
                .iter()
                .map(|s| {
                    (
                        s.id,
                        Entry {
                            state: SourceState::WarmingUp,
                            value: None,
                            observed_at: None,
                            updated: now,
                            last_success: None,
                            sequence: 0,
                            minimum_interval: s.minimum_interval,
                        },
                    )
                })
                .collect(),
        }
    }
    fn update(
        &mut self,
        id: SourceId,
        reading: Result<SourceValue, SourceState>,
        now: Instant,
        observed_at: i64,
    ) {
        let entry = self.entries.get_mut(&id).expect("registered source");
        entry.updated = now;
        entry.sequence += 1;
        match reading {
            Ok(value) => {
                entry.state = SourceState::Ok;
                entry.observed_at = Some(observed_at);
                entry.last_success = Some(now);
                entry.value = Some(value);
            }
            Err(state) => {
                entry.state = if state == SourceState::Ok {
                    SourceState::Error
                } else {
                    state
                };
                entry.value = None;
            }
        }
    }
    fn snapshot(&self, now: Instant, interval: Duration) -> SystemInfo {
        let mut snapshot = SystemInfo {
            cpu: None,
            memory: None,
            gpus: Vec::new(),
            disks: Vec::new(),
            storage: Vec::new(),
            disk_throughput: Vec::new(),
            source_states: ["cpu", "memory", "gpu", "storage", "disk_throughput"]
                .into_iter()
                .map(|name| (name.into(), SourceState::NotImplemented))
                .collect(),
            source_meta: BTreeMap::new(),
            windows_storage: None,
            windows_disk_throughput: None,
            observed_at: 0,
        };
        for (id, entry) in &self.entries {
            let period = interval.max(entry.minimum_interval);
            let age = now.saturating_duration_since(entry.updated);
            let stale = matches!(entry.state, SourceState::Ok | SourceState::WarmingUp)
                && age > period.saturating_mul(3).max(Duration::from_secs(15));
            let state = if stale {
                SourceState::Stale
            } else {
                entry.state
            };
            snapshot.source_states.insert(id.name().into(), state);
            snapshot.source_meta.insert(
                id.name().into(),
                SourceMeta {
                    observed_at: entry.observed_at,
                    age_ms: entry
                        .last_success
                        .map(|at| now.saturating_duration_since(at).as_millis() as u64),
                    expected_interval_ms: period.as_millis() as u64,
                    sequence: entry.sequence,
                },
            );
            if state == SourceState::Ok {
                if let Some(value) = &entry.value {
                    match value {
                        SourceValue::Cpu(v) => snapshot.cpu = Some(v.clone()),
                        SourceValue::Memory(v) => snapshot.memory = Some(v.clone()),
                        SourceValue::Gpu(v) => snapshot.gpus = v.clone(),
                        SourceValue::Storage(v) => snapshot.windows_storage = Some(v.clone()),
                        SourceValue::DiskThroughput(v) => {
                            snapshot.windows_disk_throughput = Some(v.clone())
                        }
                    }
                }
            }
            snapshot.observed_at = snapshot.observed_at.max(entry.observed_at.unwrap_or(0));
        }
        snapshot
    }
}

struct Signal {
    inner: Mutex<(bool, Duration, u64)>,
    changed: Condvar,
}
impl Signal {
    fn current(&self) -> (bool, Duration, u64) {
        *self.inner.lock().unwrap()
    }
    fn stop(&self) {
        self.inner.lock().unwrap().0 = true;
        self.changed.notify_all();
    }
    fn interval(&self, interval: Duration) {
        let mut state = self.inner.lock().unwrap();
        if state.1 != interval {
            state.1 = interval;
            state.2 += 1;
            self.changed.notify_all();
        }
    }
    fn wait(&self, duration: Duration, generation: u64) {
        let guard = self.inner.lock().unwrap();
        let _ = self
            .changed
            .wait_timeout_while(guard, duration, |v| !v.0 && v.2 == generation);
    }
}

pub struct SourceRuntime {
    cache: Arc<Mutex<Cache>>,
    signal: Arc<Signal>,
    lost_history: Arc<AtomicU64>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}
impl SourceRuntime {
    pub fn start(
        specs: Vec<SourceSpec>,
        interval: Duration,
        history_capacity: usize,
        mut writer: HistoryWriter,
    ) -> std::io::Result<Self> {
        if specs
            .iter()
            .enumerate()
            .any(|(i, s)| specs[..i].iter().any(|p| p.id == s.id))
            || interval.is_zero()
            || history_capacity == 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid source configuration",
            ));
        }
        let cache = Arc::new(Mutex::new(Cache::new(&specs, Instant::now())));
        let signal = Arc::new(Signal {
            inner: Mutex::new((false, interval, 0)),
            changed: Condvar::new(),
        });
        let lost = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::sync_channel::<HistoryBatch>(history_capacity);
        let mut runtime = Self {
            cache,
            signal,
            lost_history: lost,
            handles: Mutex::new(Vec::new()),
        };
        let writer_signal = runtime.signal.clone();
        let writer_lost = runtime.lost_history.clone();
        let history_thread = thread::Builder::new()
            .name("monitor-history".into())
            .spawn(move || loop {
                match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(batch) => {
                        if !matches!(catch_unwind(AssertUnwindSafe(|| writer(batch))), Ok(Ok(()))) {
                            writer_lost.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) if writer_signal.current().0 => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            })?;
        runtime.handles.get_mut().unwrap().push(history_thread);
        for spec in specs {
            let source_cache = runtime.cache.clone();
            let source_signal = runtime.signal.clone();
            let source_lost = runtime.lost_history.clone();
            let source_sender = sender.clone();
            let handle = thread::Builder::new()
                .name(format!("monitor-{}", spec.id.name()))
                .spawn(move || {
                    let mut collector = match catch_unwind(AssertUnwindSafe(spec.create)) {
                        Ok(collector) => collector,
                        Err(_) => {
                            source_cache.lock().unwrap().update(
                                spec.id,
                                Err(SourceState::Error),
                                Instant::now(),
                                0,
                            );
                            return;
                        }
                    };
                    let mut sequence = 0u64;
                    let mut clock = crate::platform::windows::sample_clock::SampleClock::default();
                    loop {
                        let (stopped, requested, generation) = source_signal.current();
                        if stopped {
                            break;
                        }
                        sequence += 1;
                        let start = Instant::now();
                        let outcome = catch_unwind(AssertUnwindSafe(&mut collector));
                        if source_signal.current().0 {
                            break;
                        }
                        let panicked = outcome.is_err();
                        let reading = outcome.unwrap_or(Err(SourceState::Error)).and_then(|v| {
                            if v.id() == spec.id && v.valid() {
                                Ok(v)
                            } else {
                                Err(SourceState::Error)
                            }
                        });
                        let boundary = !clock.observe(requested.max(spec.minimum_interval));
                        let timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|v| v.as_secs() as i64);
                        let reading = if timestamp.is_ok() {
                            reading
                        } else {
                            Err(SourceState::Error)
                        };
                        let observed = timestamp.unwrap_or(0);
                        let batch = reading.as_ref().ok().map(|v| {
                            let mut batch = history_batch(v, observed);
                            batch.sequence = sequence;
                            batch.boundary = boundary;
                            batch
                        });
                        source_cache.lock().unwrap().update(
                            spec.id,
                            reading,
                            Instant::now(),
                            observed,
                        );
                        if let Some(batch) = batch.filter(|b| !b.rows.is_empty()) {
                            // Never let a slow/full DB delay realtime. A dropped batch is visible.
                            if source_sender.try_send(batch).is_err() {
                                source_lost.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        if panicked {
                            break;
                        }
                        let period = requested.max(spec.minimum_interval);
                        let remaining = period
                            .checked_sub(start.elapsed())
                            .filter(|v| !v.is_zero())
                            .unwrap_or_else(|| period.min(Duration::from_millis(250)));
                        source_signal.wait(remaining, generation);
                    }
                });
            match handle {
                Ok(handle) => runtime.handles.get_mut().unwrap().push(handle),
                Err(error) => {
                    runtime.shutdown(Duration::from_secs(1));
                    return Err(error);
                }
            }
        }
        drop(sender);
        Ok(runtime)
    }
    pub fn snapshot(&self) -> SystemInfo {
        self.cache
            .lock()
            .unwrap()
            .snapshot(Instant::now(), self.signal.current().1)
    }
    pub fn set_interval(&self, interval: Duration) {
        if !interval.is_zero() {
            self.signal.interval(interval);
        }
    }
    pub fn lost_history_batches(&self) -> u64 {
        self.lost_history.load(Ordering::Relaxed)
    }
    /// Bounded join. A non-cancellable source never causes replacement workers or an infinite wait.
    pub fn shutdown(&self, timeout: Duration) -> usize {
        self.signal.stop();
        let deadline = Instant::now() + timeout;
        let mut handles = self.handles.lock().unwrap();
        while handles.iter().any(|h| !h.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let mut pending = Vec::new();
        for handle in handles.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                pending.push(handle);
            }
        }
        let count = pending.len();
        *handles = pending;
        count
    }
}
impl Drop for SourceRuntime {
    fn drop(&mut self) {
        self.signal.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_segments_require_adjacent_successful_writes() {
        let mut state = HistoryContinuity::default();
        let mut batch = history_batch(&cpu(1.0), 1000);
        batch.sequence = 1;
        let first = state.segment(&batch);
        state.saved(&batch, first.clone());
        batch.sequence = 2;
        batch.boundary = false;
        assert_eq!(state.segment(&batch), first);
        // No acknowledgement for failed write / dropped attempt 2.
        batch.sequence = 3;
        let after_loss = state.segment(&batch);
        assert_ne!(after_loss, first);
        state.saved(&batch, after_loss.clone());
        batch.sequence = 4;
        batch.boundary = true;
        assert_ne!(state.segment(&batch), after_loss);
        assert_ne!(HistoryContinuity::default().segment(&batch), after_loss);
    }

    fn cpu(value: f32) -> SourceValue {
        SourceValue::Cpu(CpuInfo {
            name: "synthetic".into(),
            physical_cores: Some(1),
            logical_cores: 1,
            total_usage: value,
            per_core_usage: vec![value],
        })
    }
    fn spec(
        id: SourceId,
        callback: Box<dyn FnMut() -> Result<SourceValue, SourceState> + Send>,
    ) -> SourceSpec {
        SourceSpec {
            id,
            minimum_interval: Duration::ZERO,
            create: Box::new(move || callback),
        }
    }
    #[test]
    fn cache_has_per_source_time_staleness_and_no_zero_fill() {
        let now = Instant::now();
        let specs = vec![
            spec(SourceId::Cpu, Box::new(|| Ok(cpu(0.0)))),
            spec(SourceId::Gpu, Box::new(|| Err(SourceState::Unverified))),
        ];
        let mut cache = Cache::new(&specs, now);
        cache.update(SourceId::Cpu, Ok(cpu(0.0)), now, 123);
        cache.update(
            SourceId::Gpu,
            Err(SourceState::PermissionRequired),
            now,
            123,
        );
        let snap = cache.snapshot(now + Duration::from_secs(20), Duration::from_secs(30));
        assert_eq!(snap.cpu.unwrap().total_usage, 0.0);
        assert!(snap.gpus.is_empty());
        assert_eq!(snap.source_states["gpu"], SourceState::PermissionRequired);
        assert_eq!(snap.source_meta["cpu"].observed_at, Some(123));
        let stale = cache.snapshot(now + Duration::from_secs(91), Duration::from_secs(30));
        assert!(stale.cpu.is_none());
        assert_eq!(stale.source_states["cpu"], SourceState::Stale);
        assert_eq!(stale.source_meta["cpu"].observed_at, Some(123));
        cache.update(
            SourceId::Cpu,
            Err(SourceState::Error),
            now + Duration::from_secs(92),
            999,
        );
        let failed = cache.snapshot(now + Duration::from_secs(95), Duration::from_secs(30));
        assert_eq!(failed.source_meta["cpu"].observed_at, Some(123));
        assert_eq!(failed.source_meta["cpu"].age_ms, Some(95_000));
        assert!(failed.cpu.is_none());
    }
    #[test]
    fn a_blocked_gpu_does_not_block_cpu_or_spawn_extra_gpu_workers() {
        let (entered, entry) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (written, writes) = mpsc::channel();
        let runtime = SourceRuntime::start(
            vec![
                spec(SourceId::Cpu, Box::new(|| Ok(cpu(5.0)))),
                spec(
                    SourceId::Gpu,
                    Box::new(move || {
                        entered.send(()).unwrap();
                        let _ = blocked.recv();
                        Err(SourceState::Error)
                    }),
                ),
            ],
            Duration::from_millis(20),
            8,
            Box::new(move |batch| {
                written.send(batch).unwrap();
                Ok(())
            }),
        )
        .unwrap();
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..3 {
            assert!(writes
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .rows
                .iter()
                .all(|r| r.metric.starts_with("cpu.")));
        }
        assert!(entry.try_recv().is_err());
        assert!(runtime.snapshot().cpu.is_some());
        // Cancellation wakes idle workers, but does not falsely claim to cancel a blocking API.
        assert!(runtime.shutdown(Duration::from_millis(100)) > 0);
        release.send(()).unwrap();
        assert_eq!(runtime.shutdown(Duration::from_secs(2)), 0);
    }
    #[test]
    fn blocked_history_is_bounded_and_loss_is_visible_while_cpu_updates() {
        let (entered, entry) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (observed, observations) = mpsc::channel();
        let mut first = true;
        let runtime = SourceRuntime::start(
            vec![spec(
                SourceId::Cpu,
                Box::new(move || {
                    let _ = observed.send(());
                    Ok(cpu(2.0))
                }),
            )],
            Duration::from_millis(20),
            1,
            Box::new(move |_| {
                if first {
                    first = false;
                    entered.send(()).unwrap();
                    blocked.recv().unwrap();
                }
                Ok(())
            }),
        )
        .unwrap();
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..6 {
            observations.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        assert!(runtime.snapshot().source_meta["cpu"].sequence >= 4);
        assert!(runtime.lost_history_batches() > 0);
        release.send(()).unwrap();
        assert_eq!(runtime.shutdown(Duration::from_secs(2)), 0);
    }
    #[test]
    fn history_contains_only_new_source_values() {
        let batch = history_batch(&cpu(0.0), 1234);
        assert_eq!(batch.observed_at, 1234);
        assert_eq!(batch.rows.len(), 2);
        assert!(batch.rows.iter().all(|r| r.value == 0.0 && r.unit == "%"));
        assert!(!cpu(f32::NAN).valid());
        assert!(!cpu(101.0).valid());
    }

    #[test]
    fn changing_interval_wakes_a_worker_waiting_for_background_period() {
        let (observed, observations) = mpsc::channel();
        let runtime = SourceRuntime::start(
            vec![spec(
                SourceId::Cpu,
                Box::new(move || {
                    observed.send(()).unwrap();
                    Ok(cpu(3.0))
                }),
            )],
            Duration::from_secs(30),
            4,
            Box::new(|_| Ok(())),
        )
        .unwrap();
        observations.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.set_interval(Duration::from_millis(20));
        observations.recv_timeout(Duration::from_secs(2)).unwrap();
        observations.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            runtime.snapshot().source_meta["cpu"].expected_interval_ms,
            20
        );
        assert_eq!(runtime.shutdown(Duration::from_secs(2)), 0);
    }

    #[test]
    fn cached_gpu_value_is_not_rewritten_at_cpu_cadence() {
        let (entered, entry) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (written, writes) = mpsc::channel();
        let mut first = true;
        let runtime = SourceRuntime::start(
            vec![
                spec(SourceId::Cpu, Box::new(|| Ok(cpu(3.0)))),
                spec(
                    SourceId::Gpu,
                    Box::new(move || {
                        if first {
                            first = false;
                            return Ok(SourceValue::Gpu(vec![GpuInfo {
                                object_id: "synthetic-gpu".into(),
                                name: "synthetic".into(),
                                utilization: 4.0,
                                memory_used_bytes: 0,
                                memory_allocated_bytes: Some(0),
                                windows_memory: None,
                            }]));
                        }
                        entered.send(()).unwrap();
                        let _ = blocked.recv();
                        Err(SourceState::Error)
                    }),
                ),
            ],
            Duration::from_millis(20),
            16,
            Box::new(move |batch| {
                written.send(batch).unwrap();
                Ok(())
            }),
        )
        .unwrap();
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        let (mut cpu_count, mut gpu_count) = (0, 0);
        while cpu_count < 6 {
            let batch = writes.recv_timeout(Duration::from_secs(2)).unwrap();
            if batch.rows[0].metric == "gpu.utilization" {
                gpu_count += 1;
            } else {
                cpu_count += 1;
            }
        }
        assert_eq!(gpu_count, 1);
        assert_eq!(runtime.snapshot().gpus[0].utilization, 4.0);
        runtime.shutdown(Duration::from_millis(100));
        release.send(()).unwrap();
        assert_eq!(runtime.shutdown(Duration::from_secs(2)), 0);
    }
}
