//! Physical disk / APFS container / volume topology (S5).
//!
//! APFS shares capacity across volumes in a container, so per-volume "free"
//! must never be summed. We model the three layers explicitly and attribute
//! shared capacity to the container's resource pool exactly once (F05, F12).

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub id: String,
    pub name: String,
    pub role: String,
    /// Bytes this volume actually consumes within the shared container.
    pub capacity_consumed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub container_ref: String,
    /// Physical store device identifier backing this container (e.g. disk0s2).
    pub physical_store: Option<String>,
    pub capacity_ceiling: Option<u64>,
    pub capacity_free: Option<u64>,
    pub capacity_in_use: Option<u64>,
    pub volumes: Vec<VolumeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalDisk {
    pub device: String,
    pub name: String,
    pub size_bytes: u64,
    pub smart_status: String,
    pub temperature_celsius: Option<f32>,
    pub power_on_hours: Option<u64>,
    /// APFS containers whose physical store lives on this disk.
    pub containers: Vec<ContainerInfo>,
}

/// Build the storage topology: enumerate physical disks, then attach APFS
/// containers by matching their physical store device prefix.
pub fn build_topology() -> Result<Vec<PhysicalDisk>, String> {
    let mut disks = enumerate_physical_disks()?;
    let containers = enumerate_apfs_containers().unwrap_or_default();

    for container in containers {
        // A physical store like "disk0s2" belongs to base disk "disk0".
        if let Some(store) = &container.physical_store {
            let base = base_disk_id(store);
            if let Some(disk) = disks.iter_mut().find(|d| d.device == base) {
                disk.containers.push(container);
            }
            // If no base disk matches (e.g. synthesized or unplugged), the
            // container is dropped rather than misattributed to another disk.
        }
    }
    Ok(disks)
}

/// Strip partition suffix: "disk0s2" → "disk0", "disk10" → "disk10".
/// APFS physical stores append "sN" to the base disk identifier. We scan
/// from the end for a trailing 's<digits>' segment so the 's' inside the
/// literal "disk" prefix is never mistaken for the partition separator.
fn base_disk_id(device: &str) -> String {
    // Find the last 's' such that everything after it is digits.
    if let Some(idx) = device.rfind('s') {
        let suffix = &device[idx + 1..];
        if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
            return device[..idx].to_string();
        }
    }
    device.to_string()
}

fn enumerate_physical_disks() -> Result<Vec<PhysicalDisk>, String> {
    let output = Command::new("diskutil")
        .args(["list", "-plist", "physical"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("diskutil list physical failed".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut disks = Vec::new();

    let plist = plist::from_bytes::<plist::Value>(stdout.as_bytes())
        .map_err(|e| format!("diskutil list plist parse: {}", e))?;
    let array = plist
        .as_dictionary()
        .and_then(|d| d.get("AllDisksAndPartitions"))
        .and_then(|v| v.as_array())
        .ok_or("no AllDisksAndPartitions")?;

    for item in array {
        if let Some(dev_id) = item.as_dictionary()
            .and_then(|d| d.get("DeviceIdentifier"))
            .and_then(|v| v.as_string())
        {
            let (name, size_bytes, smart_status, temperature_celsius, power_on_hours) =
                disk_info(dev_id).unwrap_or_else(|_| (String::new(), 0, String::new(), None, None));
            disks.push(PhysicalDisk {
                device: dev_id.to_string(),
                name,
                size_bytes,
                smart_status,
                temperature_celsius,
                power_on_hours,
                containers: Vec::new(),
            });
        }
    }
    Ok(disks)
}

/// (name, size_bytes, smart_status, temperature_c, power_on_hours)
fn disk_info(dev_id: &str) -> Result<(String, u64, String, Option<f32>, Option<u64>), String> {
    let output = Command::new("diskutil")
        .args(["info", "-plist", dev_id])
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let plist = plist::from_bytes::<plist::Value>(stdout.as_bytes())
        .map_err(|e| e.to_string())?;
    let d = plist.as_dictionary().ok_or("no dict")?;

    let temperature_celsius = d.get("SMARTDeviceSpecificKeysMayVaryNotGuaranteed")
        .and_then(|v| v.as_dictionary())
        .and_then(|dd| dd.get("TEMPERATURE"))
        .and_then(|v| v.as_unsigned_integer())
        .and_then(|k| {
            let c = k as f32 - 273.15;
            if (0.0..=150.0).contains(&c) { Some(c) } else { None }
        });

    let power_on_hours = d.get("SMARTDeviceSpecificKeysMayVaryNotGuaranteed")
        .and_then(|v| v.as_dictionary())
        .and_then(|dd| dd.get("POWER_ON_HOURS_0"))
        .and_then(|v| v.as_unsigned_integer());

    Ok((
        d.get("MediaName").and_then(|v| v.as_string()).unwrap_or("").to_string(),
        d.get("TotalSize").and_then(|v| v.as_unsigned_integer()).unwrap_or(0),
        d.get("SMARTStatus").and_then(|v| v.as_string()).unwrap_or("").to_string(),
        temperature_celsius,
        power_on_hours,
    ))
}

fn enumerate_apfs_containers() -> Result<Vec<ContainerInfo>, String> {
    let output = Command::new("diskutil")
        .args(["apfs", "list", "-plist"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("diskutil apfs list failed".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let plist = plist::from_bytes::<plist::Value>(stdout.as_bytes())
        .map_err(|e| e.to_string())?;
    let containers_arr = plist
        .as_dictionary()
        .and_then(|d| d.get("Containers"))
        .and_then(|v| v.as_array())
        .ok_or("no Containers")?;

    let mut out = Vec::new();
    for c in containers_arr {
        let cd = match c.as_dictionary() { Some(d) => d, None => continue };
        // Handle multiple PhysicalStores; use the first as primary but note count.
        let physical_store = cd.get("PhysicalStores")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|ps| ps.as_dictionary())
            .and_then(|ps| ps.get("DeviceIdentifier"))
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());

        let volumes = cd.get("Volumes")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| {
                let vd = v.as_dictionary()?;
                Some(VolumeInfo {
                    id: vd.get("DeviceIdentifier")?.as_string()?.to_string(),
                    name: vd.get("Name").and_then(|v| v.as_string()).unwrap_or("").to_string(),
                    role: vd.get("Role").and_then(|v| v.as_string()).unwrap_or("").to_string(),
                    capacity_consumed: vd.get("CapacityInUse").and_then(|v| v.as_unsigned_integer()),
                })
            }).collect())
            .unwrap_or_default();

        out.push(ContainerInfo {
            container_ref: cd.get("ContainerReference").and_then(|v| v.as_string()).unwrap_or("").to_string(),
            physical_store,
            capacity_ceiling: cd.get("CapacityCeiling").and_then(|v| v.as_unsigned_integer()),
            capacity_free: cd.get("CapacityFree").and_then(|v| v.as_unsigned_integer()),
            capacity_in_use: cd.get("CapacityInUse").and_then(|v| v.as_unsigned_integer()),
            volumes,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::base_disk_id;

    #[test]
    fn strips_partition_suffix() {
        assert_eq!(base_disk_id("disk0s2"), "disk0");
        assert_eq!(base_disk_id("disk4s1"), "disk4");
        assert_eq!(base_disk_id("disk10"), "disk10");
        assert_eq!(base_disk_id("disk0"), "disk0");
    }
}
