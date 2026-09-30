//! A limited ADLX reader in an ordinary short-lived child. No SDK headers or controls.
//! Read-only vtable prefixes adapted from AMD's MIT licensed Rust bindings.
//! Copyright (c) 2021-2025 Advanced Micro Devices, Inc.
//! Source commit 32b5a740d42295c5dfe9026b9f52683da0f3af91; see docs/licenses/amd-adlx-rust-MIT.txt.
use crate::model::SourceState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{ffi::c_void, fs::OpenOptions, io::Read, mem, os::windows::fs::OpenOptionsExt, ptr};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, LUID},
    System::{
        LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
        SystemInformation::GetSystemDirectoryW,
    },
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    pub luid: [u32; 2],
    pub edge_c: Option<f64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub state: SourceState,
    pub readings: Vec<Reading>,
}
#[repr(C)]
struct Interface {
    vtable: *const Base,
}
#[repr(C)]
struct Base {
    acquire: usize,
    release: unsafe extern "system" fn(*mut Interface) -> i32,
    query: unsafe extern "system" fn(*mut Interface, *const u16, *mut *mut Interface) -> i32,
}
// Unused slots preserve the published ABI; no tuning or write function is exposed.
type GetOne = unsafe extern "system" fn(*mut Interface, *mut *mut Interface) -> i32;
#[repr(C)]
struct SystemVtable {
    hybrid: usize,
    gpus: GetOne,
    unused: [usize; 7],
    performance: GetOne,
}
#[repr(C)]
struct ListVtable {
    base: Base,
    size: unsafe extern "system" fn(*mut Interface) -> u32,
    empty: usize,
    begin: unsafe extern "system" fn(*mut Interface) -> u32,
    end: unsafe extern "system" fn(*mut Interface) -> u32,
    unused: [usize; 4],
    at: unsafe extern "system" fn(*mut Interface, u32, *mut *mut Interface) -> i32,
}
#[repr(C)]
struct Gpu2Vtable {
    base: Base,
    unused: [usize; 31],
    luid: unsafe extern "system" fn(*mut Interface, *mut LUID) -> i32,
}
#[repr(C)]
struct PerformanceVtable {
    base: Base,
    unused: [usize; 15],
    current: unsafe extern "system" fn(*mut Interface, *mut Interface, *mut *mut Interface) -> i32,
    unused2: [usize; 2],
    support: unsafe extern "system" fn(*mut Interface, *mut Interface, *mut *mut Interface) -> i32,
}
#[repr(C)]
struct SupportVtable {
    base: Base,
    unused: [usize; 3],
    edge: unsafe extern "system" fn(*mut Interface, *mut u8) -> i32,
}
#[repr(C)]
struct MetricsVtable {
    base: Base,
    unused: [usize; 4],
    edge: unsafe extern "system" fn(*mut Interface, *mut f64) -> i32,
}
struct Reference(*mut Interface);
impl Reference {
    fn output(f: impl FnOnce(*mut *mut Interface) -> i32) -> Result<Self, SourceState> {
        let mut pointer = ptr::null_mut();
        let result = f(&mut pointer);
        if pointer.is_null() {
            return Err(SourceState::Unsupported);
        }
        let reference = Self(pointer);
        if !success(result) || unsafe { (*pointer).vtable }.is_null() {
            return Err(SourceState::Error);
        }
        Ok(reference)
    }
}
impl Drop for Reference {
    fn drop(&mut self) {
        unsafe {
            if !(*self.0).vtable.is_null() {
                ((*(*self.0).vtable).release)(self.0);
            }
        }
    }
}
struct Library(*mut c_void);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
type Terminate = unsafe extern "C" fn() -> i32;
struct Context(Terminate);
impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            (self.0)();
        }
    }
}
fn success(code: i32) -> bool {
    (0..=2).contains(&code)
}
unsafe fn collect(system: *mut Interface) -> Result<Vec<Reading>, SourceState> {
    unsafe {
        let sys = &*(*system).vtable.cast::<SystemVtable>();
        let performance = Reference::output(|p| (sys.performance)(system, p))?;
        let list = Reference::output(|p| (sys.gpus)(system, p))?;
        let ls = &*(*list.0).vtable.cast::<ListVtable>();
        let begin = (ls.begin)(list.0);
        let end = (ls.end)(list.0);
        if (ls.size)(list.0) > 32 || end < begin || end - begin > 32 {
            return Err(SourceState::Error);
        }
        let perf = &*(*performance.0).vtable.cast::<PerformanceVtable>();
        let iid: Vec<_> = "IADLXGPU2".encode_utf16().chain(Some(0)).collect();
        let mut readings = Vec::new();
        for i in begin..end {
            let gpu = Reference::output(|p| (ls.at)(list.0, i, p))?;
            let gpu2 = Reference::output(|p| ((*(*gpu.0).vtable).query)(gpu.0, iid.as_ptr(), p))?;
            let extended = &*(*gpu2.0).vtable.cast::<Gpu2Vtable>();
            let mut luid = LUID {
                LowPart: 0,
                HighPart: 0,
            };
            if !success((extended.luid)(gpu2.0, &mut luid)) {
                continue;
            }
            let support = Reference::output(|p| (perf.support)(performance.0, gpu.0, p))?;
            let supported = &*(*support.0).vtable.cast::<SupportVtable>();
            let mut edge = 0u8;
            let is_supported = success((supported.edge)(support.0, &mut edge)) && edge == 1;
            let value = if is_supported {
                let metrics = Reference::output(|p| (perf.current)(performance.0, gpu.0, p))?;
                let metric = &*(*metrics.0).vtable.cast::<MetricsVtable>();
                let mut celsius = 0.0;
                (success((metric.edge)(metrics.0, &mut celsius))
                    && celsius.is_finite()
                    && celsius > 0.0
                    && celsius <= 150.0)
                    .then_some(celsius)
            } else {
                None
            };
            readings.push(Reading {
                luid: [luid.HighPart as u32, luid.LowPart],
                edge_c: value,
            });
        }
        if readings
            .iter()
            .enumerate()
            .any(|(i, r)| readings[..i].iter().any(|p| p.luid == r.luid))
        {
            return Err(SourceState::Unverified);
        }
        Ok(readings)
    }
}
fn read() -> Result<Vec<Reading>, SourceState> {
    if !cfg!(target_arch = "x86_64") {
        return Err(SourceState::Unsupported);
    }
    let mut system = [0u16; 260];
    let length = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), 260) } as usize;
    if length == 0 || length >= system.len() {
        return Err(SourceState::Error);
    }
    let path =
        std::path::PathBuf::from(String::from_utf16_lossy(&system[..length])).join("amdadlx64.dll");
    // This approved driver build is the first supported runtime. Keep the file open
    // without write/delete sharing across hash verification, loading and all calls.
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(|_| SourceState::Unsupported)?;
    if file.metadata().map_err(|_| SourceState::Error)?.len() > 32 * 1024 * 1024 {
        return Err(SourceState::Unverified);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| SourceState::Error)?;
    if format!("{:X}", Sha256::digest(&bytes))
        != "13C4628222B07E812C755EEE53308ED4AA1B2FA25E1CE96D4E5FABD88309FD60"
    {
        return Err(SourceState::Unverified);
    }
    let wide: Vec<_> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let library = Library(unsafe {
        LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32)
    });
    if library.0.is_null() {
        return Err(SourceState::Error);
    }
    macro_rules! symbol {
        ($name:literal,$ty:ty) => {{
            let address = unsafe { GetProcAddress(library.0, concat!($name, "\0").as_ptr()) }
                .ok_or(SourceState::Unsupported)?;
            unsafe { mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(address) }
        }};
    }
    let version = symbol!(
        "ADLXQueryFullVersion",
        unsafe extern "C" fn(*mut u64) -> i32
    );
    let initialize = symbol!(
        "ADLXInitialize",
        unsafe extern "C" fn(u64, *mut *mut Interface) -> i32
    );
    let terminate = symbol!("ADLXTerminate", Terminate);
    let expected = (1u64 << 48) | (5u64 << 32) | 124;
    let mut current = 0;
    if !success(unsafe { version(&mut current) }) || current != expected {
        return Err(SourceState::Unverified);
    }
    let mut system = ptr::null_mut();
    if !success(unsafe { initialize(expected, &mut system) }) {
        return Err(SourceState::Error);
    }
    let context = Context(terminate);
    if system.is_null() {
        return Err(SourceState::Error);
    }
    let readings = unsafe { collect(system) }?;
    let code = unsafe { terminate() };
    mem::forget(context);
    if !success(code) {
        return Err(SourceState::Error);
    }
    Ok(readings)
}
pub fn entry() -> bool {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--gpu-temperature-read") {
        return false;
    }
    if args.len() != 2 {
        std::process::exit(1);
    }
    let reply = match read() {
        Ok(readings) => Reply {
            state: SourceState::Ok,
            readings,
        },
        Err(state) => Reply {
            state,
            readings: vec![],
        },
    };
    println!("{}", serde_json::to_string(&reply).unwrap());
    true
}
