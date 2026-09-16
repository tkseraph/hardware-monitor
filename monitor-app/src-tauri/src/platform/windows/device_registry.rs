//! Persistent anonymous series, with live-session continuity supplied by a held device handle.
//! Reopening the registry NEVER guesses identity from model, capacity or disk number.
use crate::storage_model::DiskHistorySeries;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_RECORDS: usize = 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    version: u32,
    records: Vec<DiskHistorySeries>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub name: String,
    pub size_bytes: u64,
}
pub struct IdentityInput {
    pub number: u32,
    pub descriptor: Descriptor,
    pub continuous: bool,
}
pub struct Registry {
    path: PathBuf,
    catalog: Catalog,
    // Runtime addresses are never serialized.
    active: BTreeMap<u32, (Descriptor, String)>,
}
impl Registry {
    pub fn open(root: &Path) -> Result<Self, &'static str> {
        Self::open_file(root, "device-registry.json")
    }
    pub fn open_gpu(root: &Path) -> Result<Self, &'static str> {
        Self::open_file(root, "gpu-registry.json")
    }
    fn open_file(root: &Path, name: &str) -> Result<Self, &'static str> {
        let path = root.join(name);
        let catalog = match std::fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Catalog {
                version: 1,
                records: Vec::new(),
            },
            Err(_) => return Err("registry_unreadable"),
            Ok(meta) => {
                if meta.len() > MAX_BYTES {
                    return Err("registry_too_large");
                }
                let mut data = Vec::new();
                std::fs::File::open(&path)
                    .map_err(|_| "registry_unreadable")?
                    .take(MAX_BYTES + 1)
                    .read_to_end(&mut data)
                    .map_err(|_| "registry_unreadable")?;
                if data.len() as u64 > MAX_BYTES {
                    return Err("registry_too_large");
                }
                let catalog: Catalog =
                    serde_json::from_slice(&data).map_err(|_| "registry_invalid")?;
                let mut ids = HashSet::new();
                if catalog.version != 1
                    || catalog.records.len() > MAX_RECORDS
                    || catalog.records.iter().any(|r| {
                        Uuid::parse_str(&r.uid).is_err()
                            || !ids.insert(&r.uid)
                            || r.name.len() > 1024
                            || r.size_bytes == 0
                    })
                {
                    return Err("registry_invalid");
                }
                catalog
            }
        };
        Ok(Self {
            path,
            catalog,
            active: BTreeMap::new(),
        })
    }

    pub fn reconcile(
        &mut self,
        input: &[IdentityInput],
        now: i64,
    ) -> Result<BTreeMap<u32, String>, &'static str> {
        let mut next = self.catalog.clone();
        let mut active = BTreeMap::new();
        for device in input {
            if active.contains_key(&device.number)
                || device.descriptor.name.len() > 1024
                || device.descriptor.size_bytes == 0
            {
                return Err("invalid_identity_input");
            }
            let previous = self
                .active
                .get(&device.number)
                .filter(|(d, _)| device.continuous && d == &device.descriptor);
            let uid = match previous {
                Some((_, uid)) => uid.clone(),
                None => {
                    if next.records.len() >= MAX_RECORDS {
                        return Err("registry_full");
                    }
                    let uid = Uuid::new_v4().to_string();
                    next.records.push(DiskHistorySeries {
                        uid: uid.clone(),
                        name: device.descriptor.name.clone(),
                        size_bytes: device.descriptor.size_bytes,
                        created_at: now,
                    });
                    uid
                }
            };
            active.insert(device.number, (device.descriptor.clone(), uid));
        }
        if next.records.len() != self.catalog.records.len() {
            self.save(&next)?;
        }
        self.catalog = next;
        self.active = active;
        Ok(self
            .active
            .iter()
            .map(|(&number, (_, uid))| (number, uid.clone()))
            .collect())
    }

    pub fn records(&self) -> Vec<DiskHistorySeries> {
        self.catalog.records.clone()
    }

    fn save(&self, catalog: &Catalog) -> Result<(), &'static str> {
        let data = serde_json::to_vec(catalog).map_err(|_| "registry_encode_failed")?;
        if data.len() as u64 > MAX_BYTES {
            return Err("registry_too_large");
        }
        let temp = self
            .path
            .with_file_name(format!(".device-registry-{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(|_| "registry_save_failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("monitor-registry-{}", Uuid::new_v4()));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn input(number: u32, continuous: bool) -> IdentityInput {
        IdentityInput {
            number,
            continuous,
            descriptor: Descriptor {
                name: "synthetic identical disk".into(),
                size_bytes: 1000,
            },
        }
    }
    #[test]
    fn identical_disks_are_distinct_and_reordering_preserves_live_handles() {
        let root = Temp::new();
        let mut registry = Registry::open(&root.0).unwrap();
        let first = registry
            .reconcile(&[input(1, false), input(2, false)], 1)
            .unwrap();
        assert_ne!(first[&1], first[&2]);
        let second = registry
            .reconcile(&[input(2, true), input(1, true)], 2)
            .unwrap();
        assert_eq!(first, second);
        let text = std::fs::read_to_string(root.0.join("device-registry.json")).unwrap();
        assert!(!text.contains("number"));
        assert!(!text.contains("continuous"));
    }
    #[test]
    fn restart_preserves_archive_but_never_reuses_ids_by_guessing() {
        let root = Temp::new();
        let old = Registry::open(&root.0)
            .unwrap()
            .reconcile(&[input(0, false)], 1)
            .unwrap()[&0]
            .clone();
        let mut registry = Registry::open(&root.0).unwrap();
        assert_eq!(registry.records()[0].uid, old);
        let new = registry.reconcile(&[input(0, true)], 2).unwrap()[&0].clone();
        assert_ne!(old, new);
        assert_eq!(registry.records().len(), 2);
    }
    #[test]
    fn removal_or_address_reuse_starts_a_new_series() {
        let root = Temp::new();
        let mut registry = Registry::open(&root.0).unwrap();
        let first = registry.reconcile(&[input(0, false)], 1).unwrap()[&0].clone();
        let replaced = registry.reconcile(&[input(0, false)], 2).unwrap()[&0].clone();
        assert_ne!(first, replaced);
        registry.reconcile(&[], 3).unwrap();
        let reattached = registry.reconcile(&[input(0, true)], 4).unwrap()[&0].clone();
        assert_ne!(replaced, reattached);
    }
    #[test]
    fn corruption_and_save_failure_do_not_reset_or_overwrite_identity() {
        let root = Temp::new();
        let path = root.0.join("device-registry.json");
        std::fs::write(&path, b"corrupt synthetic file").unwrap();
        assert!(Registry::open(&root.0).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt synthetic file");
        std::fs::remove_file(path).unwrap();
        let mut registry = Registry::open(&root.0).unwrap();
        registry.path = root.0.join("missing-parent").join("registry.json");
        assert!(registry.reconcile(&[input(0, false)], 1).is_err());
        assert!(registry.records().is_empty());
    }
}
