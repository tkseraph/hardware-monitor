//! Read-only device/volume queries. Raw volume addresses never leave this module.
use crate::model::SourceState;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{
    DISK_EXTENT, IOCTL_STORAGE_CHECK_VERIFY2, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    STORAGE_DEVICE_NUMBER, VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

fn last_state() -> SourceState {
    match unsafe { GetLastError() } {
        5 => SourceState::PermissionRequired,
        1 | 50 => SourceState::Unsupported,
        _ => SourceState::Error,
    }
}
fn open(path: &str) -> Result<OwnedHandle, SourceState> {
    let wide: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        Err(last_state())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
pub struct DiskLease {
    handle: OwnedHandle,
    number: u32,
}
impl DiskLease {
    pub fn open(number: u32) -> Result<Self, SourceState> {
        if number > 4095 {
            return Err(SourceState::Unsupported);
        }
        let lease = Self {
            handle: open(&format!("\\\\.\\PhysicalDrive{number}"))?,
            number,
        };
        if !lease.valid() {
            return Err(SourceState::Unverified);
        }
        Ok(lease)
    }
    pub fn valid(&self) -> bool {
        let mut data: STORAGE_DEVICE_NUMBER = unsafe { std::mem::zeroed() };
        let mut bytes = 0;
        let success = unsafe {
            DeviceIoControl(
                self.handle.as_raw_handle(),
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                std::ptr::null(),
                0,
                (&mut data as *mut STORAGE_DEVICE_NUMBER).cast(),
                std::mem::size_of_val(&data) as u32,
                &mut bytes,
                std::ptr::null_mut(),
            )
        };
        if success == 0
            || bytes < std::mem::size_of_val(&data) as u32
            || data.DeviceNumber != self.number
        {
            return false;
        }
        let mut media_change = 0u32;
        unsafe {
            DeviceIoControl(
                self.handle.as_raw_handle(),
                IOCTL_STORAGE_CHECK_VERIFY2,
                std::ptr::null(),
                0,
                (&mut media_change as *mut u32).cast(),
                4,
                &mut bytes,
                std::ptr::null_mut(),
            ) != 0
        }
    }
}

fn volume_device(path: &str) -> Option<String> {
    let id = path.strip_prefix("\\\\?\\Volume{")?.strip_suffix("}\\")?;
    let id = uuid::Uuid::parse_str(id).ok()?;
    Some(format!("\\\\?\\Volume{{{id}}}"))
}

pub fn volume_disks(path: &str) -> Result<Vec<u32>, SourceState> {
    let path = volume_device(path).ok_or(SourceState::Unverified)?;
    let handle = open(&path)?;
    // Bounded extent array. ERROR_MORE_DATA does not become a partial mapping.
    let mut storage = vec![0u64; 2048];
    let mut bytes = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            std::ptr::null(),
            0,
            storage.as_mut_ptr().cast(),
            (storage.len() * 8) as u32,
            &mut bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_state());
    }
    if bytes as usize > storage.len() * 8 {
        return Err(SourceState::Error);
    }
    let raw = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), bytes as usize) };
    parse_extents(raw)
}

fn parse_extents(bytes: &[u8]) -> Result<Vec<u32>, SourceState> {
    let offset = std::mem::offset_of!(VOLUME_DISK_EXTENTS, Extents);
    if bytes.len() < offset {
        return Err(SourceState::Error);
    }
    let count = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
    if count == 0
        || count > 256
        || offset + count * std::mem::size_of::<DISK_EXTENT>() > bytes.len()
    {
        return Err(SourceState::Error);
    }
    let mut result = Vec::new();
    for i in 0..count {
        let extent = unsafe {
            bytes
                .as_ptr()
                .add(offset + i * std::mem::size_of::<DISK_EXTENT>())
                .cast::<DISK_EXTENT>()
                .read_unaligned()
        };
        if extent.StartingOffset < 0 || extent.ExtentLength <= 0 {
            return Err(SourceState::Error);
        }
        result.push(extent.DiskNumber);
    }
    result.sort_unstable();
    result.dedup();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_addresses_cannot_be_arbitrary_paths() {
        assert!(volume_device("C:\\private").is_none());
        assert!(volume_device("\\\\?\\Volume{invalid}\\").is_none());
        assert!(volume_device("\\\\?\\Volume{12345678-1234-4234-8234-123456789abc}\\").is_some());
    }
    #[test]
    fn spans_are_deduplicated_without_losing_cross_disk_mapping() {
        let offset = std::mem::offset_of!(VOLUME_DISK_EXTENTS, Extents);
        let mut bytes = vec![0; offset + 3 * std::mem::size_of::<DISK_EXTENT>()];
        bytes[..4].copy_from_slice(&3u32.to_ne_bytes());
        for (i, number) in [2, 1, 2].into_iter().enumerate() {
            let extent = DISK_EXTENT {
                DiskNumber: number,
                StartingOffset: 4096,
                ExtentLength: 8192,
            };
            unsafe {
                bytes
                    .as_mut_ptr()
                    .add(offset + i * std::mem::size_of::<DISK_EXTENT>())
                    .cast::<DISK_EXTENT>()
                    .write_unaligned(extent);
            }
        }
        assert_eq!(parse_extents(&bytes).unwrap(), vec![1, 2]);
        assert!(parse_extents(&bytes[..offset + 1]).is_err());
    }
}
