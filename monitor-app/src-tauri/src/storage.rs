//! Physical disk / APFS container / volume topology (S5).
//!
//! APFS shares capacity across volumes in a container, so per-volume "free"
//! must never be summed. We model the three layers explicitly and attribute
//! shared capacity to the container's resource pool exactly once (F05, F12).
//!
//! R7/A09: a container keeps its FULL backing-store list (`physical_stores`).
//! A container spanning several physical disks is a shared pool — its capacity
//! is recorded once on the container and deliberately NOT attributed to any
//! single disk, so "used" never masquerades as one whole disk's usage.
//!
//! R7/A10: `device` (`disk0`, `disk4`, …) is a per-enumeration address, NOT a
//! durable hardware identity. Disks therefore also carry a locally-generated
//! anonymous `device_uid` with a connection/generation counter; history is
//! keyed by that uid so a reused `diskN` never joins a previous disk's series.

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTopology {
    pub disks: Vec<PhysicalDisk>,
    /// Containers spanning more than one physical disk (shared pools).
    /// Capacity counted once here; not present in any disk's `containers`.
    pub shared_containers: Vec<ContainerInfo>,
}

/// Build the storage topology: enumerate physical disks, then attach APFS
/// containers by matching their physical store device prefix. Single-store
/// containers attach to their one backing disk; multi-disk shared pools are
/// collected separately so their capacity is counted once and never attributed
/// to a single disk (A09).
pub fn build_topology() -> Result<Vec<PhysicalDisk>, String> {
    Ok(build_full_topology()?.disks)
}

/// Full topology including shared multi-disk pools. Live command execution.
pub fn build_full_topology() -> Result<StorageTopology, String> {
    let disks_plist = crate::cmd::run("/usr/sbin/diskutil", &["list", "-plist", "physical"], crate::cmd::DEFAULT_TIMEOUT)?
        .into_result()?;
    let containers_plist = crate::cmd::run("/usr/sbin/apfs", &["list", "-plist"], crate::cmd::DEFAULT_TIMEOUT)
        .or_else(|_| crate::cmd::run("/usr/sbin/diskutil", &["apfs", "list", "-plist"], crate::cmd::DEFAULT_TIMEOUT))
        .ok()
        .and_then(|o| o.into_result().ok());

    let disks = parse_physical_disks(&disks_plist, true)?;
    let containers: Vec<ContainerInfo> = containers_plist
        .as_deref()
        .map(parse_apfs_containers)
        .transpose()
        .unwrap_or_default()
        .unwrap_or_default();
    Ok(assemble_topology(disks, containers))
}

/// Attach containers to disks. Pure & testable: no command execution.
///
/// - A container with exactly one physical store is attached to the disk whose
///   `device` matches the store's base id; if no disk matches (synthesized or
///   unplugged store), the container is dropped rather than misattributed.
/// - A container with zero or multiple physical stores is a shared/unbacked
///   pool and goes to `shared_containers`, counted once, never per-disk (A09).
fn assemble_topology(mut disks: Vec<PhysicalDisk>, containers: Vec<ContainerInfo>) -> StorageTopology {
    let mut shared = Vec::new();
    for container in containers {
        if container.shared_pool || container.physical_stores.len() != 1 {
            shared.push(container);
            continue;
        }
        let store = &container.physical_stores[0];
        let base = base_disk_id(store);
        match disks.iter_mut().find(|d| d.device == base) {
            Some(disk) => disk.containers.push(container),
            None => shared.push(container),
        }
    }
    StorageTopology { disks, shared_containers: shared }
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

/// Parse `diskutil list -plist physical` into disks. When `fetch_info` is true,
/// per-disk `diskutil info` is queried for name/size/SMART; tests pass false
/// and supply size via the fixture so no hardware is touched.
fn parse_physical_disks(plist_xml: &str, fetch_info: bool) -> Result<Vec<PhysicalDisk>, String> {
    let plist = plist::from_bytes::<plist::Value>(plist_xml.as_bytes())
        .map_err(|e| format!("diskutil list plist parse: {}", e))?;
    let array = plist
        .as_dictionary()
        .and_then(|d| d.get("AllDisksAndPartitions"))
        .and_then(|v| v.as_array())
        .ok_or("no AllDisksAndPartitions")?;

    let mut disks = Vec::new();
    for item in array {
        if let Some(dev_id) = item.as_dictionary()
            .and_then(|d| d.get("DeviceIdentifier"))
            .and_then(|v| v.as_string())
        {
            // A disk entry (not a partition) is one whose identifier has no
            // "sN" partition suffix — "disk0" yes, "disk0s1" no.
            if base_disk_id(dev_id) != dev_id {
                continue;
            }
            let (name, size_bytes, smart_status, temperature_celsius, power_on_hours) = if fetch_info {
                disk_info(dev_id).unwrap_or_else(|_| (String::new(), 0, String::new(), None, None))
            } else {
                (String::new(), 0, String::new(), None, None)
            };
            disks.push(PhysicalDisk {
                device: dev_id.to_string(),
                name,
                size_bytes,
                smart_status,
                temperature_celsius,
                power_on_hours,
                containers: Vec::new(),
                device_uid: None,
                generation: 0,
            });
        }
    }
    Ok(disks)
}

/// (name, size_bytes, smart_status, temperature_c, power_on_hours)
fn disk_info(dev_id: &str) -> Result<(String, u64, String, Option<f32>, Option<u64>), String> {
    let stdout = crate::cmd::run("/usr/sbin/diskutil", &["info", "-plist", dev_id], crate::cmd::DEFAULT_TIMEOUT)?
        .into_result()?;
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

/// Parse `diskutil apfs list -plist` into containers. Pure & testable.
///
/// R7/A09: keeps the FULL `PhysicalStores` array (never truncated to first) and
/// marks `shared_pool` when more than one store backs the container. Volume
/// roles accept both the modern `Roles` array and the legacy single `Role`
/// string, so multi-role volumes are not collapsed.
fn parse_apfs_containers(plist_xml: &str) -> Result<Vec<ContainerInfo>, String> {
    let plist = plist::from_bytes::<plist::Value>(plist_xml.as_bytes())
        .map_err(|e| e.to_string())?;
    let containers_arr = plist
        .as_dictionary()
        .and_then(|d| d.get("Containers"))
        .and_then(|v| v.as_array())
        .ok_or("no Containers")?;

    let mut out = Vec::new();
    for c in containers_arr {
        let cd = match c.as_dictionary() { Some(d) => d, None => continue };

        // Keep every physical store, in reported order.
        let physical_stores: Vec<String> = cd.get("PhysicalStores")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|ps| {
                ps.as_dictionary()
                    .and_then(|d| d.get("DeviceIdentifier"))
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
            }).collect())
            .unwrap_or_default();
        let shared_pool = physical_stores.len() > 1;

        let volumes = cd.get("Volumes")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| {
                let vd = v.as_dictionary()?;
                let id = vd.get("DeviceIdentifier")?.as_string()?.to_string();
                // Roles array (modern) takes precedence; fall back to the
                // legacy single Role string.
                let roles: Vec<String> = vd.get("Roles")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|r| r.as_string().map(|s| s.to_string())).collect())
                    .or_else(|| vd.get("Role").and_then(|v| v.as_string()).map(|s| vec![s.to_string()]))
                    .unwrap_or_default();
                let role = roles.first().cloned().unwrap_or_default();
                Some(VolumeInfo {
                    id,
                    name: vd.get("Name").and_then(|v| v.as_string()).unwrap_or("").to_string(),
                    role,
                    roles,
                    capacity_consumed: vd.get("CapacityInUse").and_then(|v| v.as_unsigned_integer()),
                })
            }).collect())
            .unwrap_or_default();

        out.push(ContainerInfo {
            container_ref: cd.get("ContainerReference").and_then(|v| v.as_string()).unwrap_or("").to_string(),
            physical_stores,
            shared_pool,
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
    use super::*;

    #[test]
    fn strips_partition_suffix() {
        assert_eq!(base_disk_id("disk0s2"), "disk0");
        assert_eq!(base_disk_id("disk4s1"), "disk4");
        assert_eq!(base_disk_id("disk10"), "disk10");
        assert_eq!(base_disk_id("disk0"), "disk0");
    }

    // ---- Anonymized diskutil plist fixtures (no serials/hostnames/paths) ----

    fn disk_list_plist(devs: &[&str]) -> String {
        let items: String = devs.iter().map(|d| format!(
            "<dict><key>DeviceIdentifier</key><string>{}</string></dict>", d
        )).collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>AllDisksAndPartitions</key><array>{}</array></dict></plist>"#,
            items
        )
    }

    fn container_xml(store_ids: &[&str], vols: &str, extra: &str) -> String {
        let stores: String = store_ids.iter().map(|s| format!(
            "<dict><key>DeviceIdentifier</key><string>{}</string></dict>", s
        )).collect();
        format!(
            "<dict><key>ContainerReference</key><string>disk1</string>\
             <key>PhysicalStores</key><array>{}</array>\
             <key>CapacityCeiling</key><integer>1000000</integer>\
             <key>CapacityFree</key><integer>400000</integer>\
             <key>CapacityInUse</key><integer>600000</integer>\
             <key>Volumes</key><array>{}</array>{}</dict>",
            stores, vols, extra
        )
    }

    fn apfs_list_plist(containers: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Containers</key><array>{}</array></dict></plist>"#,
            containers
        )
    }

    #[test]
    fn single_disk_single_container_attaches() {
        let disks = parse_physical_disks(&disk_list_plist(&["disk0"]), false).unwrap();
        let vol = "<dict><key>DeviceIdentifier</key><string>disk1s1</string>\
                   <key>Name</key><string>System</string>\
                   <key>Roles</key><array><string>System</string></array>\
                   <key>CapacityInUse</key><integer>15000</integer></dict>";
        let containers = parse_apfs_containers(&apfs_list_plist(&container_xml(&["disk0s2"], vol, ""))).unwrap();
        assert_eq!(containers.len(), 1);
        assert!(!containers[0].shared_pool);
        assert_eq!(containers[0].physical_stores, vec!["disk0s2"]);
        let topo = assemble_topology(disks, containers);
        assert_eq!(topo.disks.len(), 1);
        assert_eq!(topo.disks[0].containers.len(), 1);
        assert!(topo.shared_containers.is_empty());
    }

    #[test]
    fn multi_disk_shared_pool_not_attributed_to_first_disk() {
        // A09 regression: a container backed by disk0s2 + disk2s1 must NOT be
        // wholly attached to disk0. It becomes a shared pool counted once.
        let disks = parse_physical_disks(&disk_list_plist(&["disk0", "disk2"]), false).unwrap();
        let containers = parse_apfs_containers(&apfs_list_plist(&container_xml(&["disk0s2", "disk2s1"], "", ""))).unwrap();
        assert_eq!(containers[0].physical_stores.len(), 2);
        assert!(containers[0].shared_pool, "two stores => shared pool");
        let topo = assemble_topology(disks, containers);
        assert_eq!(topo.disks.len(), 2);
        assert!(topo.disks.iter().all(|d| d.containers.is_empty()),
                "shared pool must not be attributed to any single disk");
        assert_eq!(topo.shared_containers.len(), 1, "counted once as a pool");
    }

    #[test]
    fn single_disk_multiple_containers() {
        let disks = parse_physical_disks(&disk_list_plist(&["disk0"]), false).unwrap();
        let c1 = container_xml(&["disk0s2"], "", "<key>ContainerReference</key><string>disk1</string>");
        let c2 = container_xml(&["disk0s4"], "", "<key>ContainerReference</key><string>disk3</string>");
        let containers = parse_apfs_containers(&apfs_list_plist(&format!("{}{}", c1, c2))).unwrap();
        let topo = assemble_topology(disks, containers);
        assert_eq!(topo.disks[0].containers.len(), 2);
        assert!(topo.shared_containers.is_empty());
    }

    #[test]
    fn zero_physical_stores_is_unbacked_pool_not_dropped_silently() {
        // Zero PhysicalStores: no backing disk. Must not attach to anything and
        // must be surfaced as a shared/unbacked pool (not vanish, not guess disk0).
        let disks = parse_physical_disks(&disk_list_plist(&["disk0"]), false).unwrap();
        let containers = parse_apfs_containers(&apfs_list_plist(&container_xml(&[], "", ""))).unwrap();
        assert!(containers[0].physical_stores.is_empty());
        assert!(!containers[0].shared_pool, "zero stores is not a multi-disk pool");
        let topo = assemble_topology(disks, containers);
        assert!(topo.disks[0].containers.is_empty());
        assert_eq!(topo.shared_containers.len(), 1, "unbacked container surfaced, not attached to a disk");
    }

    #[test]
    fn container_with_unmatched_store_does_not_attach_to_wrong_disk() {
        // Hot-plug / synthesized: store disk9s2 has no matching disk9 in list.
        let disks = parse_physical_disks(&disk_list_plist(&["disk0"]), false).unwrap();
        let containers = parse_apfs_containers(&apfs_list_plist(&container_xml(&["disk9s2"], "", ""))).unwrap();
        let topo = assemble_topology(disks, containers);
        assert!(topo.disks[0].containers.is_empty(), "must not misattribute to disk0");
        assert_eq!(topo.shared_containers.len(), 1);
    }

    #[test]
    fn roles_array_preferred_and_multi_role_preserved() {
        let vol = "<dict><key>DeviceIdentifier</key><string>disk1s1</string>\
                   <key>Name</key><string>Data</string>\
                   <key>Roles</key><array><string>Data</string><string>System</string></array>\
                   <key>CapacityInUse</key><integer>100</integer></dict>";
        let containers = parse_apfs_containers(&apfs_list_plist(&container_xml(&["disk0s2"], vol, ""))).unwrap();
        assert_eq!(containers[0].volumes[0].roles, vec!["Data", "System"]);
        assert_eq!(containers[0].volumes[0].role, "Data");
    }

    #[test]
    fn legacy_single_role_string_falls_back() {
        let vol = "<dict><key>DeviceIdentifier</key><string>disk1s1</string>\
                   <key>Name</key><string>Recovery</string>\
                   <key>Role</key><string>Recovery</string></dict>";
        let containers = parse_apfs_containers(&apfs_list_plist(&container_xml(&["disk0s2"], vol, ""))).unwrap();
        assert_eq!(containers[0].volumes[0].role, "Recovery");
        assert_eq!(containers[0].volumes[0].roles, vec!["Recovery"]);
    }

    #[test]
    fn malformed_plist_returns_error_not_panic() {
        assert!(parse_physical_disks("not a plist", false).is_err());
        assert!(parse_apfs_containers("{ not xml").is_err());
        assert!(parse_apfs_containers(&disk_list_plist(&["disk0"])).is_err(), "missing Containers key");
    }

    #[test]
    fn partition_entries_excluded_from_physical_disk_list() {
        // diskutil list physical should yield whole disks; if a partition leaks
        // in, we skip it so it never becomes a phantom PhysicalDisk.
        let disks = parse_physical_disks(&disk_list_plist(&["disk0", "disk0s1", "disk0s2"]), false).unwrap();
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].device, "disk0");
    }
}
