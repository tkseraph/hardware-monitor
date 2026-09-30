//! Fixed CPU temperature reader, not registered with the UI until the source is verified.
//! Independent PawnIO device-IOCTL client; no arbitrary module, offset or write commands.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::Read,
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{GetLastError, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0},
    Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        Registry::{
            RegGetValueW, HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
        },
        SystemInformation::GetSystemDirectoryW,
        Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        IO::DeviceIoControl,
    },
};
const MODULE: &[u8] = include_bytes!("../../resources/pawnio/AMDFamily17.bin");
const MODULE_HASH: &str = "099DC01D6DB97EA997FEC4A461E191CC64B9D7CE47C9D2153C451C56C2ADCF50";
const DRIVER_HASH: &str = "FCA6E7D58B0CF38DBB913A2B9E532F48629145D395F454B16A9F58E97B8D3940";
const VERSION: u32 = 0xA1B22184;
const LOAD: u32 = 0xA1B22084;
const EXECUTE: u32 = 0xA1B22104;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub state: String,
    pub cpu_name: String,
    pub family: u32,
    pub model: u32,
    pub sensor: String,
    pub celsius: Option<f64>,
    pub error_code: Option<u32>,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
#[cfg(target_arch = "x86_64")]
#[allow(unused_unsafe)] // Rust 1.89 requires unsafe here; current Rust makes CPUID safe.
fn identity() -> (String, u32, u32, bool) {
    unsafe {
        let vendor = std::arch::x86_64::__cpuid(0);
        let mut name = Vec::new();
        for v in [vendor.ebx, vendor.edx, vendor.ecx] {
            name.extend(v.to_le_bytes());
        }
        let amd = name == b"AuthenticAMD";
        let fms = std::arch::x86_64::__cpuid(1).eax;
        let base = (fms >> 8) & 15;
        let family = base + if base == 15 { (fms >> 20) & 255 } else { 0 };
        let model = ((fms >> 4) & 15)
            | if base == 6 || base == 15 {
                ((fms >> 16) & 15) << 4
            } else {
                0
            };
        let mut bytes = Vec::new();
        if std::arch::x86_64::__cpuid(0x80000000).eax >= 0x80000004 {
            for leaf in 0x80000002..=0x80000004 {
                let v = std::arch::x86_64::__cpuid(leaf);
                for n in [v.eax, v.ebx, v.ecx, v.edx] {
                    bytes.extend(n.to_le_bytes());
                }
            }
        }
        let brand = String::from_utf8_lossy(&bytes)
            .trim_matches(['\0', ' '])
            .to_string();
        (brand, family, model, amd)
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn identity() -> (String, u32, u32, bool) {
    (String::new(), 0, 0, false)
}
/// Report the control-temperature field in Celsius, never a fabricated Tdie value.
fn decode(raw: u32) -> Option<f64> {
    if raw == 0 || raw == u32::MAX {
        return None;
    }
    let c = (raw >> 21) as f64 * 0.125
        - if raw & (1 << 19) != 0 || raw & (3 << 16) == 3 << 16 {
            49.0
        } else {
            0.0
        };
    (c > 0.0 && c <= 150.0).then_some(c)
}
struct Fault(&'static str, Option<u32>);
fn driver_image() -> Result<PathBuf, Fault> {
    let mut buffer = [0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    let key = wide("SYSTEM\\CurrentControlSet\\Services\\PawnIO");
    let value = wide("ImagePath");
    let code = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if code != 0 {
        return Err(Fault(
            if code == 2 {
                "component_missing"
            } else {
                "driver_unverified"
            },
            Some(code),
        ));
    }
    let text = String::from_utf16_lossy(
        &buffer[..buffer
            .iter()
            .position(|v| *v == 0)
            .ok_or(Fault("driver_unverified", None))?],
    );
    let mut sys = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(sys.as_mut_ptr(), 260) } as usize;
    if len == 0 || len >= 260 {
        return Err(Fault("driver_unverified", None));
    }
    let system = PathBuf::from(String::from_utf16_lossy(&sys[..len]));
    let root = system.parent().ok_or(Fault("driver_unverified", None))?;
    let path = if let Some(suffix) = text.strip_prefix("\\SystemRoot\\") {
        root.join(suffix)
    } else {
        PathBuf::from(
            text.trim_matches('"')
                .strip_prefix("\\??\\")
                .unwrap_or(text.trim_matches('"')),
        )
    };
    let path = path
        .canonicalize()
        .map_err(|_| Fault("driver_unverified", None))?;
    let repository = system
        .join("DriverStore\\FileRepository")
        .canonicalize()
        .map_err(|_| Fault("driver_unverified", None))?;
    if !path.starts_with(repository)
        || !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("PawnIO.sys"))
    {
        return Err(Fault("driver_unverified", None));
    }
    Ok(path)
}
fn ioctl(handle: &OwnedHandle, code: u32, input: &[u8], output: &mut [u8]) -> Result<(), Fault> {
    let mut written = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            code,
            if input.is_empty() {
                ptr::null()
            } else {
                input.as_ptr().cast()
            },
            input.len() as u32,
            if output.is_empty() {
                ptr::null_mut()
            } else {
                output.as_mut_ptr().cast()
            },
            output.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(Fault("read_error", Some(unsafe { GetLastError() })));
    }
    if written as usize != output.len() {
        return Err(Fault("invalid_response_size", None));
    }
    Ok(())
}
struct PciLock(OwnedHandle);
impl Drop for PciLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.as_raw_handle());
        }
    }
}
fn temperature() -> Result<f64, Fault> {
    if format!("{:X}", Sha256::digest(MODULE)) != MODULE_HASH {
        return Err(Fault("module_unverified", None));
    }
    let path = driver_image()?;
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|_| Fault("driver_unverified", None))?;
    if file
        .metadata()
        .map_err(|_| Fault("driver_unverified", None))?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(Fault("driver_unverified", None));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| Fault("driver_unverified", None))?;
    if format!("{:X}", Sha256::digest(&bytes)) != DRIVER_HASH {
        return Err(Fault("driver_unverified", None));
    }
    // Only this one package/model is in the first machine's supported scope.
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Package {
        number_of_cores: u32,
    }
    let connection = wmi::WMIConnection::new().map_err(|_| Fault("package_unverified", None))?;
    let mut packages = connection
        .exec_query("SELECT NumberOfCores FROM Win32_Processor")
        .map_err(|_| Fault("package_unverified", None))?;
    let first = packages
        .next()
        .ok_or(Fault("package_unverified", None))?
        .map_err(|_| Fault("package_unverified", None))?
        .into_desr::<Package>()
        .map_err(|_| Fault("package_unverified", None))?;
    if first.number_of_cores == 0 || packages.next().is_some() {
        return Err(Fault("package_unverified", None));
    }
    let name = wide("\\\\?\\GLOBALROOT\\Device\\PawnIO");
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            0x80000000 | 0x40000000,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        let code = unsafe { GetLastError() };
        return Err(Fault(
            if code == 5 {
                "permission_required"
            } else {
                "driver_unavailable"
            },
            Some(code),
        ));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut version = [0u8; 4];
    ioctl(&handle, VERSION, &[], &mut version)?;
    if u32::from_le_bytes(version) != 0x00020200 {
        return Err(Fault("driver_version_unverified", None));
    }
    ioctl(&handle, LOAD, MODULE, &mut [])?;
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, wide("Global\\Access_PCI").as_ptr()) };
    if mutex.is_null() {
        return Err(Fault("mutex_unavailable", Some(unsafe { GetLastError() })));
    }
    let mutex = unsafe { OwnedHandle::from_raw_handle(mutex) };
    let wait = unsafe { WaitForSingleObject(mutex.as_raw_handle(), 100) };
    if wait == WAIT_ABANDONED {
        unsafe {
            ReleaseMutex(mutex.as_raw_handle());
        }
        return Err(Fault("mutex_abandoned", None));
    }
    if wait != WAIT_OBJECT_0 {
        return Err(Fault("mutex_unavailable", None));
    }
    let _lock = PciLock(mutex);
    let mut input = [0u8; 40];
    input[..14].copy_from_slice(b"ioctl_read_smn");
    input[32..].copy_from_slice(&0x59800u64.to_le_bytes());
    let mut output = [0u8; 8];
    ioctl(&handle, EXECUTE, &input, &mut output)?;
    let raw = u64::from_le_bytes(output);
    let raw = u32::try_from(raw).map_err(|_| Fault("invalid_register", None))?;
    decode(raw).ok_or(Fault("invalid_temperature", None))
}
pub fn sample() -> Observation {
    let (cpu_name, family, model, amd) = identity();
    let mut value = Observation {
        state: "unsupported_cpu".into(),
        cpu_name,
        family,
        model,
        sensor: "Tctl".into(),
        celsius: None,
        error_code: None,
    };
    if !amd || family != 0x1A || model != 0x24 {
        return value;
    }
    match temperature() {
        Ok(t) => {
            value.state = "ok".into();
            value.celsius = Some(t);
        }
        Err(Fault(state, code)) => {
            value.state = state.into();
            value.error_code = code;
        }
    }
    value
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn register_units_range_flags_and_invalid_values_are_distinct() {
        assert_eq!(decode(0x40000000), Some(64.0));
        assert_eq!(decode(0x68080000), Some(55.0));
        assert_eq!(decode(0x68030000), Some(55.0));
        assert_eq!(decode(0x680B0000), Some(55.0));
        assert_eq!(decode(0), None);
        assert_eq!(decode(u32::MAX), None);
    }
}
