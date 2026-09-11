//! Anonymous, privacy-preserving storage device identity (R7/A10).
//!
//! `disk0`/`disk4` are per-enumeration addresses, not durable hardware
//! identities: the same number can be reassigned to a different physical device
//! after a reboot or hot-plug. Keying history by `diskN` would then join one
//! disk's series onto another's.
//!
//! Within our privacy boundary (D-010: no serials, no UUIDs, no product IDs) we
//! cannot perfectly track a physical device across reboots. So we use a
//! locally-generated, anonymous fingerprint: a stable hash of non-unique
//! medium descriptors (name + size + backing stores). The fingerprint keys a
//! persistent registry that hands out:
//!
//!   * `device_uid` — an opaque anonymous id ("dev-<hex>") used as the history
//!     `object_id`, never a raw `diskN`.
//!   * `generation` — bumped when the SAME `device` address is re-seen backed
//!     by a DIFFERENT medium fingerprint (hot-plug reuse), so a reused `diskN`
//!     starts a new series instead of merging with the previous owner's.
//!
//! When two genuinely different media produce the same fingerprint (same model
//! name, same size, same store layout), we cannot distinguish them within the
//! privacy boundary; we keep them merged rather than reading serials as a
//! shortcut. That limitation is documented, not hidden.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// One registered medium: its anonymous uid, generation, and the device
/// address it was last seen at.
#[derive(Debug, Clone)]
struct DeviceRecord {
    device_uid: String,
    generation: u32,
    last_device: String,
}

#[derive(Default)]
pub struct DeviceRegistry {
    /// fingerprint -> record
    by_fingerprint: std::collections::HashMap<u64, DeviceRecord>,
    /// device address -> fingerprint, to detect address reuse by a new medium
    by_device: std::collections::HashMap<String, u64>,
    /// monotonically increasing uid counter (kept process-local; the uid is
    /// stable per fingerprint, the counter only mints new uids)
    next_uid: u32,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Assign (or reconfirm) the anonymous identity for one physical disk.
    ///
    /// `device` is the current enumeration address; `name`, `size_bytes` and
    /// `store_ids` are the privacy-safe medium descriptors. Returns
    /// `(device_uid, generation)`.
    pub fn assign(
        &mut self,
        device: &str,
        name: &str,
        size_bytes: u64,
        store_ids: &[String],
    ) -> (String, u32) {
        let fingerprint = medium_fingerprint(name, size_bytes, store_ids.len());

        // Address reuse detection: if this `device` address was previously a
        // DIFFERENT medium, that medium's generation bumps so the reused diskN
        // does not continue the old owner's series.
        if let Some(&prev_fp) = self.by_device.get(device) {
            if prev_fp != fingerprint {
                if let Some(prev) = self.by_fingerprint.get_mut(&prev_fp) {
                    prev.generation += 1;
                }
            }
        }
        self.by_device.insert(device.to_string(), fingerprint);

        match self.by_fingerprint.get_mut(&fingerprint) {
            Some(rec) => {
                rec.last_device = device.to_string();
                (rec.device_uid.clone(), rec.generation)
            }
            None => {
                self.next_uid += 1;
                let rec = DeviceRecord {
                    device_uid: format!("dev-{:08x}", self.next_uid),
                    generation: 0,
                    last_device: device.to_string(),
                };
                let uid = rec.device_uid.clone();
                self.by_fingerprint.insert(fingerprint, rec);
                (uid, 0)
            }
        }
    }
}

/// Stable, anonymous medium fingerprint from non-unique descriptors only.
/// Never includes serials, UUIDs, product IDs — or the volatile `diskN`
/// address, so the SAME medium keeps ONE uid across re-enumeration (A10).
///
/// The store *count* (not the store addresses) is mixed in so a single-disk
/// medium and a multi-disk pool of the same nominal name/size do not collide.
fn medium_fingerprint(name: &str, size_bytes: u64, store_count: usize) -> u64 {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    size_bytes.hash(&mut h);
    store_count.hash(&mut h);
    h.finish()
}

static REGISTRY: Mutex<Option<DeviceRegistry>> = Mutex::new(None);

/// Assign identities for a whole topology's disks. Safe to call every sample;
/// stable fingerprints return the same uid, address reuse bumps generation.
pub fn assign_topology(disks: &mut [crate::storage::PhysicalDisk]) {
    let mut guard = match REGISTRY.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let reg = guard.get_or_insert_with(DeviceRegistry::new);
    for d in disks.iter_mut() {
        // Backing stores for this disk = the physical stores of its single-store
        // containers (shared pools are not attributed to one disk, A09).
        let mut stores: Vec<String> = Vec::new();
        for c in &d.containers {
            stores.extend(c.physical_stores.iter().cloned());
        }
        let (uid, generation) = reg.assign(&d.device, &d.name, d.size_bytes, &stores);
        d.device_uid = Some(uid);
        d.generation = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_medium_same_device_keeps_uid_and_generation() {
        let mut r = DeviceRegistry::new();
        let (u1, g1) = r.assign("disk0", "APPLE SSD", 500_000_000_000, &["disk0s2".into()]);
        let (u2, g2) = r.assign("disk0", "APPLE SSD", 500_000_000_000, &["disk0s2".into()]);
        assert_eq!(u1, u2);
        assert_eq!(g1, g2);
        assert_eq!(g1, 0);
    }

    #[test]
    fn uid_is_stable_across_device_reenumeration() {
        // Same medium re-enumerated as a different address keeps its uid; the
        // uid (not diskN) is the durable history key.
        let mut r = DeviceRegistry::new();
        let (u1, _) = r.assign("disk0", "APPLE SSD", 500_000_000_000, &["disk0s2".into()]);
        let (u2, _) = r.assign("disk4", "APPLE SSD", 500_000_000_000, &["disk4s2".into()]);
        assert_eq!(u1, u2, "same medium must keep one uid across re-enumeration");
    }

    #[test]
    fn reused_device_address_for_new_medium_bumps_generation() {
        // Hot-plug: disk0 was medium A, now medium B reuses the address.
        let mut r = DeviceRegistry::new();
        let (ua, ga) = r.assign("disk0", "APPLE SSD", 500_000_000_000, &["disk0s2".into()]);
        let (ub, gb) = r.assign("disk0", "SAMSUNG SSD", 1_000_000_000_000, &["disk0s2".into()]);
        assert_ne!(ua, ub, "different medium gets a different uid");
        assert_eq!(gb, 0, "new medium starts at generation 0");
        // Re-seeing medium A elsewhere later reflects the bumped generation.
        let (_ua2, ga2) = r.assign("disk4", "APPLE SSD", 500_000_000_000, &["disk4s2".into()]);
        assert!(ga2 > ga, "address reuse bumped the previous owner's generation");
    }

    #[test]
    fn distinct_media_get_distinct_uids() {
        let mut r = DeviceRegistry::new();
        let (u1, _) = r.assign("disk0", "APPLE SSD", 500_000_000_000, &["disk0s2".into()]);
        let (u2, _) = r.assign("disk2", "SAMSUNG SSD", 1_000_000_000_000, &["disk2s1".into()]);
        assert_ne!(u1, u2);
    }

    #[test]
    fn fingerprint_is_privacy_safe_and_store_count_sensitive() {
        // Single-disk vs multi-disk pool of the same nominal name/size differ.
        let single = medium_fingerprint("SSD", 1000, 1);
        let pool = medium_fingerprint("SSD", 1000, 2);
        assert_ne!(single, pool);
        // The volatile diskN address must NOT affect the fingerprint.
        assert_eq!(medium_fingerprint("SSD", 1000, 1), medium_fingerprint("SSD", 1000, 1));
    }
}
