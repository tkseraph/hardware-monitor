//! Explicit one-shot source capability probe. Never called by the desktop UI.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    mem, ptr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Diagnostics::Etw::*;
use wmi::WMIConnection;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Disk {
    number: u32,
    health_status: Option<u16>,
    bus_type: Option<u16>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Reliability {
    #[serde(
        rename(serialize = "DeviceSlot", deserialize = "DeviceId"),
        deserialize_with = "slot"
    )]
    device_slot: Option<u32>,
    temperature: Option<u64>,
    temperature_max: Option<u64>,
    wear: Option<u64>,
    power_on_hours: Option<u64>,
}
fn slot<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(raw.parse().ok())
}
fn query<T: serde::de::DeserializeOwned + Serialize>(conn: &WMIConnection, q: &str) -> Value {
    match conn.exec_query(q) {
        Err(wmi::WMIError::HResultError { hres }) => {
            json!({"state":"query_error","hresult":format!("{:08x}",hres as u32)})
        }
        Err(_) => json!({"state":"query_error"}),
        Ok(rows) => {
            let mut output = Vec::new();
            for item in rows.take(33) {
                if output.len() == 32 {
                    return json!({"state":"row_limit"});
                }
                let object = match item {
                    Ok(value) => value,
                    Err(wmi::WMIError::HResultError { hres }) => {
                        return json!({"state":"query_error","hresult":format!("{:08x}",hres as u32)})
                    }
                    Err(_) => return json!({"state":"query_error"}),
                };
                match object.into_desr::<T>() {
                    Ok(row) => output.push(row),
                    Err(_) => return json!({"state":"decode_error"}),
                }
            }
            json!({"state":"ok","rows":output})
        }
    }
}
fn failure(error: wmi::WMIError) -> Value {
    match error {
        wmi::WMIError::HResultError { hres } => {
            json!({"state":"query_error","hresult":format!("{:08x}",hres as u32)})
        }
        _ => json!({"state":"decode_or_provider_error"}),
    }
}
fn method_report(code: u32, rows: Vec<Reliability>) -> Value {
    if code != 0 {
        return json!({"state":"provider_error","return_code":code});
    }
    if rows.len() > 32 {
        return json!({"state":"row_limit"});
    }
    json!({"state":"ok","return_code":code,"rows":rows})
}
fn counters_for_disk(conn: &WMIConnection, disk: wmi::IWbemClassWrapper) -> wmi::WMIResult<Value> {
    let signature = conn
        .get_object("PS_StorageCmdlets")?
        .get_method("GetStorageReliabilityCounter")?;
    let Some(signature) = signature else {
        return Ok(json!({"state":"method_signature_missing"}));
    };
    let input = signature.spawn_instance()?;
    // Only constructs the read method's argument object; no provider property is written.
    input.put_property("Disk", wmi::Variant::Object(disk))?;
    let output = conn.exec_method(
        "PS_StorageCmdlets",
        "GetStorageReliabilityCounter",
        Some(&input),
    )?;
    let Some(output) = output else {
        return Ok(json!({"state":"method_output_missing"}));
    };
    let code: u32 = output.get_property("ReturnValue")?.try_into()?;
    if code != 0 {
        return Ok(method_report(code, vec![]));
    }
    let raw = output.get_property("StorageReliabilityCounter")?;
    let objects = match raw {
        wmi::Variant::Object(value) => vec![value],
        wmi::Variant::Array(values) => {
            if values.len() > 32 {
                return Ok(json!({"state":"row_limit"}));
            }
            let mut result = Vec::new();
            for value in values {
                if let wmi::Variant::Object(value) = value {
                    result.push(value);
                } else {
                    return Ok(json!({"state":"unexpected_output_type"}));
                }
            }
            result
        }
        wmi::Variant::Null | wmi::Variant::Empty => vec![],
        _ => return Ok(json!({"state":"unexpected_output_type"})),
    };
    let mut rows = Vec::new();
    for object in objects {
        rows.push(object.into_desr::<Reliability>()?);
    }
    Ok(method_report(code, rows))
}
/// Same fixed read method used by Windows StorageCmdlets.cdxml; no arbitrary methods.
pub fn storage_method_probe() -> Value {
    let conn = match WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage") {
        Ok(c) => c,
        Err(e) => return failure(e),
    };
    let disks = match conn
        .exec_query("SELECT Number,ObjectId,FriendlyName,HealthStatus,BusType,Size FROM MSFT_Disk")
    {
        Ok(d) => d,
        Err(e) => return failure(e),
    };
    let mut result = Vec::new();
    for disk in disks.take(17) {
        if result.len() == 16 {
            return json!({"state":"disk_limit"});
        }
        let disk = match disk {
            Ok(d) => d,
            Err(e) => return failure(e),
        };
        let number = match disk.get_property("Number").and_then(u32::try_from) {
            Ok(n) => n,
            Err(e) => return failure(e),
        };
        let name = disk
            .get_property("FriendlyName")
            .ok()
            .and_then(|v| String::try_from(v).ok())
            .unwrap_or_default();
        let health = disk
            .get_property("HealthStatus")
            .ok()
            .and_then(|v| u16::try_from(v).ok());
        let bus = disk
            .get_property("BusType")
            .ok()
            .and_then(|v| u16::try_from(v).ok());
        let size = disk
            .get_property("Size")
            .ok()
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);
        let reading = match counters_for_disk(&conn, disk) {
            Ok(value) => value,
            Err(e) => failure(e),
        };
        result.push(json!({"disk_number":number,"name":name,"health":health,"bus":bus,"size_bytes":size,"reliability":reading}));
    }
    json!({"state":"ok","disks":result,"method":"PS_StorageCmdlets.GetStorageReliabilityCounter","identity_mapping":"current query only; not persisted hardware identity"})
}

pub fn storage_probe() -> Value {
    match WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage") {
        Ok(conn) => json!({
            "disk_health":query::<Disk>(&conn,"SELECT Number,HealthStatus,BusType FROM MSFT_Disk"),
            "reliability":query::<Reliability>(&conn,"SELECT DeviceId,Temperature,TemperatureMax,Wear,PowerOnHours FROM MSFT_StorageReliabilityCounter"),
            "scope":"provider fields only; no identity association or sensor correctness claim"
        }),
        Err(_) => json!({"state":"connection_error"}),
    }
}

#[repr(C)]
pub(super) struct Properties {
    pub(super) properties: EVENT_TRACE_PROPERTIES,
    pub(super) name: [u16; 128],
}
impl Properties {
    pub(super) fn new(name: &str) -> Self {
        let mut p = Self {
            properties: EVENT_TRACE_PROPERTIES::default(),
            name: [0; 128],
        };
        for (slot, value) in p.name.iter_mut().zip(name.encode_utf16()) {
            *slot = value;
        }
        p.properties.Wnode.BufferSize = mem::size_of::<Self>() as u32;
        p.properties.Wnode.ClientContext = 1;
        p.properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.properties.BufferSize = 64;
        p.properties.MinimumBuffers = 2;
        p.properties.MaximumBuffers = 4;
        p.properties.FlushTimer = 1;
        p.properties.LogFileMode = EVENT_TRACE_SYSTEM_LOGGER_MODE
            | EVENT_TRACE_REAL_TIME_MODE
            | EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING;
        p.properties.EnableFlags = EVENT_TRACE_FLAG_DISK_IO;
        p.properties.LoggerNameOffset = mem::offset_of!(Self, name) as u32;
        p
    }
}
static READS: AtomicU64 = AtomicU64::new(0);
static WRITES: AtomicU64 = AtomicU64::new(0);
unsafe extern "system" fn event(record: *mut EVENT_RECORD) {
    if record.is_null() {
        return;
    }
    let header = &(*record).EventHeader;
    let guid = header.ProviderId;
    if guid.data1 == 0x3d6fa8d4
        && guid.data2 == 0xfe05
        && guid.data3 == 0x11d0
        && guid.data4 == [0x9d, 0xda, 0, 0xc0, 0x4f, 0xd7, 0xba, 0x7c]
    {
        match header.EventDescriptor.Opcode {
            10 => {
                READS.fetch_add(1, Ordering::Relaxed);
            }
            11 => {
                WRITES.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    // Never access UserData: no file path, PID or payload leaves this callback.
}
pub(super) struct Session {
    pub(super) handle: CONTROLTRACE_HANDLE,
    pub(super) properties: Properties,
    pub(super) stopped: bool,
}
impl Session {
    pub(super) fn stop(&mut self) -> u32 {
        if self.stopped {
            return 0;
        }
        let code = unsafe {
            ControlTraceW(
                self.handle,
                ptr::null(),
                &mut self.properties.properties,
                EVENT_TRACE_CONTROL_STOP,
            )
        };
        self.stopped = code == 0;
        code
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.stop();
        }
    }
}
/// Only for the coordinator's own random probe session, if its worker times out.
pub fn cleanup_session(name: &str) -> Result<u32, &'static str> {
    let suffix = name
        .strip_prefix("HardwareMonitor-E2-")
        .ok_or("invalid session name")?;
    if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid session name");
    }
    let mut p = Properties::new(name);
    Ok(unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            p.name.as_ptr(),
            &mut p.properties,
            EVENT_TRACE_CONTROL_STOP,
        )
    })
}
pub fn etw_probe(name: &str) -> Value {
    if !name
        .strip_prefix("HardwareMonitor-E2-")
        .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return json!({"state":"invalid_name"});
    }
    READS.store(0, Ordering::Relaxed);
    WRITES.store(0, Ordering::Relaxed);
    let mut properties = Properties::new(name);
    let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
    let code = unsafe {
        StartTraceW(
            &mut handle,
            properties.name.as_ptr(),
            &mut properties.properties,
        )
    };
    if code != 0 {
        return json!({"start_code":code,"started":false,"events_consumed":0});
    }
    let mut session = Session {
        handle,
        properties,
        stopped: false,
    };
    let mut logfile = EVENT_TRACE_LOGFILEW {
        LoggerName: session.properties.name.as_mut_ptr(),
        ..Default::default()
    };
    logfile.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    logfile.Anonymous2.EventRecordCallback = Some(event);
    let trace = unsafe { OpenTraceW(&mut logfile) };
    if trace.Value == u64::MAX {
        let error = std::io::Error::last_os_error().raw_os_error();
        let stopped = session.stop();
        return json!({"started":true,"consumer_error":error,"stop_code":stopped});
    }
    let worker =
        std::thread::spawn(move || unsafe { ProcessTrace(&trace, 1, ptr::null(), ptr::null()) });
    std::thread::sleep(Duration::from_secs(5));
    let stop = session.stop();
    let close = unsafe { CloseTrace(trace) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let consumer = if worker.is_finished() {
        worker.join().ok()
    } else {
        None
    };
    json!({"started":true,"start_code":0,"stop_code":stop,"close_code":close,"consumer_code":consumer,"consumer_joined":consumer.is_some(),"read_events":READS.load(Ordering::Relaxed),"write_events":WRITES.load(Ordering::Relaxed),"events_lost":session.properties.properties.EventsLost,"buffers_lost":session.properties.properties.RealTimeBuffersLost,"window_seconds":5,"file_logging":false,"per_process_attribution":false})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn method_errors_never_publish_counter_values_and_unknown_ids_are_removed() {
        let row:Reliability=serde_json::from_value(json!({"DeviceId":"private-identifier","Temperature":42,"TemperatureMax":null,"Wear":null,"PowerOnHours":null})).unwrap();
        let value = method_report(5, vec![row]);
        assert_eq!(value["state"], "provider_error");
        assert!(value.get("rows").is_none());
        let row:Reliability=serde_json::from_value(json!({"DeviceId":"private-identifier","Temperature":42,"TemperatureMax":null,"Wear":null,"PowerOnHours":null})).unwrap();
        let value = method_report(0, vec![row]);
        assert!(value["rows"][0]["DeviceSlot"].is_null());
        assert!(!value.to_string().contains("private-identifier"));
        assert_eq!(method_report(0, vec![])["rows"], json!([]));
    }

    #[test]
    fn report_fields_preserve_unknown_and_zero_without_serializing_extra_identifiers() {
        let r:Reliability=serde_json::from_value(json!({"DeviceId":"0","Temperature":null,"TemperatureMax":80,"Wear":0,"PowerOnHours":70000,"SerialNumber":"MUST_NOT_LEAK"})).unwrap();
        let v = serde_json::to_value(r).unwrap();
        assert!(v["Temperature"].is_null());
        assert_eq!(v["Wear"], 0);
        assert_eq!(v["PowerOnHours"], 70000);
        assert!(v.get("SerialNumber").is_none());
    }
    #[test]
    fn trace_layout_is_bounded_and_cleanup_name_restricted() {
        let p = Properties::new("HardwareMonitor-E2-0123456789abcdef0123456789abcdef");
        assert_eq!(p.properties.LogFileNameOffset, 0);
        assert_eq!(p.properties.MaximumBuffers, 4);
        assert_eq!(p.properties.EnableFlags, EVENT_TRACE_FLAG_DISK_IO);
        assert!(cleanup_session("NT Kernel Logger").is_err());
    }
}
