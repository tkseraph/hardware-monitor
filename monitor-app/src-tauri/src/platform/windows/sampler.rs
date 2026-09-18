//! Ordinary-user CPU/RAM with guarded difference-counter intervals.
use crate::model::{CpuInfo, MemoryInfo, SourceState};
use std::time::{Duration, Instant};
use sysinfo::{CpuRefreshKind, RefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL};

pub struct Sampler {
    sys: System,
    last_cpu_refresh: Instant,
    clock: super::sample_clock::SampleClock,
}

pub fn memory_reading(total: u64, available: u64) -> Option<MemoryInfo> {
    if total == 0 || available > total {
        return None;
    }
    let used = total - available;
    Some(MemoryInfo {
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        used_percent: (used as f64 / total as f64 * 100.0) as f32,
    })
}

impl Sampler {
    pub fn new() -> Self {
        let sys =
            System::new_with_specifics(RefreshKind::new().with_cpu(CpuRefreshKind::everything()));
        let mut clock = super::sample_clock::SampleClock::default();
        clock.observe(Duration::from_millis(
            crate::sampler::effective_interval_ms(),
        ));
        Self {
            sys,
            clock,
            last_cpu_refresh: Instant::now(),
        }
    }

    pub fn cpu(&mut self) -> Result<CpuInfo, SourceState> {
        let elapsed = self.last_cpu_refresh.elapsed();
        if elapsed < MINIMUM_CPU_UPDATE_INTERVAL {
            return Err(SourceState::WarmingUp);
        }
        let warmed = self.clock.observe(Duration::from_millis(
            crate::sampler::effective_interval_ms(),
        ));
        self.sys.refresh_cpu_usage();
        self.last_cpu_refresh = Instant::now();
        let cores = self.sys.cpus();
        let valid = !cores.is_empty()
            && cores
                .iter()
                .all(|cpu| cpu.cpu_usage().is_finite() && (0.0..=100.0).contains(&cpu.cpu_usage()));
        if !warmed {
            return Err(SourceState::WarmingUp);
        }
        if !valid {
            return Err(SourceState::Error);
        }
        Ok(CpuInfo {
            name: cores[0].brand().to_owned(),
            physical_cores: self
                .sys
                .physical_core_count()
                .and_then(|n| n.try_into().ok()),
            logical_cores: cores.len() as u32,
            total_usage: cores.iter().map(|c| c.cpu_usage()).sum::<f32>() / cores.len() as f32,
            per_core_usage: cores.iter().map(|c| c.cpu_usage()).collect(),
        })
    }

    pub fn memory(&mut self) -> Result<MemoryInfo, SourceState> {
        self.sys.refresh_memory();
        memory_reading(self.sys.total_memory(), self.sys.available_memory())
            .ok_or(SourceState::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_uses_os_visible_total_and_available() {
        let memory = memory_reading(64_000, 16_000).unwrap();
        assert_eq!(memory.used_bytes, 48_000);
        assert_eq!(memory.used_percent, 75.0);
        assert_eq!(memory_reading(100, 100).unwrap().used_percent, 0.0);
        assert!(memory_reading(0, 0).is_none());
        assert!(memory_reading(100, 101).is_none());
    }

    #[test]
    #[ignore = "opt-in read-only Windows CPU/RAM probe; no history writes"]
    fn live_cpu_memory_snapshot() {
        let mut sampler = Sampler::new();
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL + Duration::from_millis(50));
        let cpu = sampler.cpu().expect("CPU must be readable");
        let memory = sampler.memory().expect("RAM must be readable");
        assert!(!cpu.per_core_usage.is_empty());
        assert!(memory.total_bytes > 0);
        println!(
            "logical_processors={} memory_total_bytes={}",
            cpu.logical_cores, memory.total_bytes
        );
    }
}
