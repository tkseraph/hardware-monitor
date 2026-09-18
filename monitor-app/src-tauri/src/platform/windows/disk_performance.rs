use super::storage::Bindings;
use crate::model::SourceState;
use crate::storage_model::{WindowsDiskRate, WindowsThroughput};
use std::collections::BTreeMap;
use windows_sys::Win32::System::Performance::*;

type Counters = BTreeMap<u32, Option<f64>>;

struct Query {
    query: PDH_HQUERY,
    read: PDH_HCOUNTER,
    write: PDH_HCOUNTER,
}
impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}
impl Query {
    fn new() -> Result<Self, SourceState> {
        let mut value = Self {
            query: std::ptr::null_mut(),
            read: std::ptr::null_mut(),
            write: std::ptr::null_mut(),
        };
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut value.query) } != 0 {
            return Err(SourceState::Error);
        }
        for (path, target) in [
            ("\\PhysicalDisk(*)\\Disk Read Bytes/sec", &mut value.read),
            ("\\PhysicalDisk(*)\\Disk Write Bytes/sec", &mut value.write),
        ] {
            let path: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
            if unsafe { PdhAddEnglishCounterW(value.query, path.as_ptr(), 0, target) } != 0 {
                return Err(SourceState::Unsupported);
            }
        }
        unsafe {
            PdhCollectQueryData(value.query);
        }
        Ok(value)
    }
    fn sample(&self) -> Result<(Counters, Counters), SourceState> {
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return Err(SourceState::Error);
        }
        Ok((read_array(self.read)?, read_array(self.write)?))
    }
}

fn disk_number(name: &str) -> Option<u32> {
    let leading = name.split_whitespace().next()?;
    if leading.is_empty() || !leading.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    leading.parse().ok()
}

fn read_array(counter: PDH_HCOUNTER) -> Result<Counters, SourceState> {
    let mut result = BTreeMap::new();
    for (name, value) in counter_values(counter)? {
        if let Some(number) = disk_number(&name) {
            result
                .entry(number)
                .and_modify(|v| *v = None)
                .or_insert(value);
        }
    }
    Ok(result)
}

pub(super) fn counter_values(
    counter: PDH_HCOUNTER,
) -> Result<Vec<(String, Option<f64>)>, SourceState> {
    // NOSCALE/NOCAP100 are pdh.h flags omitted by windows-sys 0.61's metadata.
    const FORMAT: u32 = PDH_FMT_DOUBLE | 0x1000 | 0x8000;
    for _ in 0..3 {
        let (mut size, mut count) = (0, 0);
        let code = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                FORMAT,
                &mut size,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        if code != PDH_MORE_DATA {
            return Err(SourceState::WarmingUp);
        }
        if size == 0 || size > 4 * 1024 * 1024 {
            return Err(SourceState::Error);
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let capacity = buffer.len() * std::mem::size_of::<usize>();
        let code = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                FORMAT,
                &mut size,
                &mut count,
                buffer.as_mut_ptr().cast(),
            )
        };
        if code == PDH_MORE_DATA {
            continue;
        }
        if code != 0 {
            return Err(SourceState::Error);
        }
        if size as usize > capacity
            || count as usize * std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() > size as usize
        {
            return Err(SourceState::Error);
        }
        let start = buffer.as_ptr() as usize;
        let end = start + size as usize;
        let items = buffer.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut results = Vec::new();
        for i in 0..count as usize {
            let item = unsafe { items.add(i).read() };
            let name_ptr = item.szName as usize;
            if name_ptr < start || name_ptr >= end || !name_ptr.is_multiple_of(2) {
                return Err(SourceState::Error);
            }
            let max_units = ((end - name_ptr) / 2).min(2048);
            let units = unsafe { std::slice::from_raw_parts(item.szName, max_units) };
            let len = units
                .iter()
                .position(|&v| v == 0)
                .ok_or(SourceState::Error)?;
            let name = String::from_utf16(&units[..len]).map_err(|_| SourceState::Error)?;
            let value = unsafe { item.FmtValue.Anonymous.doubleValue };
            let reading =
                ([0, 1].contains(&item.FmtValue.CStatus) && value.is_finite() && value >= 0.0)
                    .then_some(value);
            results.push((name, reading));
        }
        return Ok(results);
    }
    Err(SourceState::Error)
}

pub struct DiskPerformance {
    query: Option<Query>,
    bindings: Bindings,
    previous: BTreeMap<u32, Option<String>>,
    clock: super::sample_clock::SampleClock,
}
impl DiskPerformance {
    pub fn new(bindings: Bindings) -> Self {
        Self {
            query: None,
            bindings,
            previous: BTreeMap::new(),
            clock: super::sample_clock::SampleClock::default(),
        }
    }
    pub fn collect(&mut self) -> Result<WindowsThroughput, SourceState> {
        let bindings = self
            .bindings
            .lock()
            .unwrap()
            .clone()
            .ok_or(SourceState::WarmingUp)?;
        if !self.clock.observe(std::time::Duration::from_millis(
            crate::sampler::effective_interval_ms(),
        )) {
            self.query = None;
            self.previous.clear();
        }
        if bindings.observed.elapsed() > bindings.ttl {
            self.query = None;
            self.previous.clear();
            return Err(SourceState::Stale);
        }
        if self.query.is_none() {
            self.query = Some(Query::new()?);
        }
        let (read, write) = self.query.as_ref().unwrap().sample()?;
        let mut all = read.keys().chain(write.keys()).copied().collect::<Vec<_>>();
        all.sort_unstable();
        all.dedup();
        let unmapped = all
            .iter()
            .filter(|n| !bindings.disks.iter().any(|b| b.number == **n))
            .count();
        let mut disks = Vec::new();
        let mut previous = BTreeMap::new();
        for binding in bindings.disks {
            let valid_handle = binding.lease.as_ref().is_some_and(|lease| lease.valid());
            let changed = self.previous.get(&binding.number) != Some(&binding.uid);
            let r = read.get(&binding.number).copied().flatten();
            let w = write.get(&binding.number).copied().flatten();
            let state = if !valid_handle {
                SourceState::Unverified
            } else if changed {
                SourceState::WarmingUp
            } else if r.is_some() && w.is_some() {
                SourceState::Ok
            } else {
                SourceState::Error
            };
            if valid_handle {
                previous.insert(binding.number, binding.uid.clone());
            }
            disks.push(WindowsDiskRate {
                number: binding.number,
                device_uid: binding.uid,
                read_bps: if changed || !valid_handle { None } else { r },
                write_bps: if changed || !valid_handle { None } else { w },
                state,
            });
        }
        self.previous = previous;
        Ok(WindowsThroughput {
            disks,
            unmapped_instances: unmapped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_instance_parser_never_counts_total_or_guesses_from_drive_letter() {
        assert_eq!(disk_number("0 C: D:"), Some(0));
        assert_eq!(disk_number("12"), Some(12));
        for name in ["_Total", "C:", "disk0", "0fake", "-1"] {
            assert_eq!(disk_number(name), None);
        }
    }
}
