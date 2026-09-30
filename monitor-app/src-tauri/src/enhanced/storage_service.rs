//! User-started storage-only companion. The desktop process stays unelevated.
use super::{
    pipe,
    protocol::{receive, send},
};
use serde::{Deserialize, Serialize};
use std::{
    io, mem,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, sync::Notify};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{GetProcessId, WaitForSingleObject},
    UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
};
const FLAG: &str = "--storage-companion";
const STORAGE_PROTOCOL_VERSION: u32 = 2;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    pub number: u32,
    pub name: String,
    pub size_bytes: u64,
    pub series_uid: Option<String>,
    pub temperature_c: Option<f64>,
    pub health: Option<u16>,
    pub wear_used_percent: Option<u64>,
    pub power_on_hours: Option<u64>,
    pub state: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub state: String,
    pub observed_at: u64,
    pub disks: Vec<Disk>,
    pub history_lost: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    number: u32,
    uid: String,
    name: String,
    size_bytes: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
enum Request {
    Hello { nonce: String, version: u32 },
    Read { targets: Vec<Target> },
    Stop {},
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "reply", deny_unknown_fields)]
enum Reply {
    Ready { version: u32 },
    Sample { view: View },
    Stopped {},
}
struct Control {
    stop: AtomicBool,
    finished: AtomicBool,
    wake: Notify,
    view: Mutex<View>,
    history_lost: Arc<std::sync::atomic::AtomicU64>,
}
static CONTROL: Mutex<Option<Arc<Control>>> = Mutex::new(None);
fn view(state: &str) -> View {
    View {
        state: state.into(),
        observed_at: 0,
        disks: vec![],
        history_lost: 0,
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn checked(mut v: View) -> io::Result<View> {
    if !matches!(v.state.as_str(), "running" | "read_error")
        || v.disks.len() > 16
        || v.disks.iter().any(|d| {
            d.name.chars().count() > 256
                || d.number > 4095
                || d.temperature_c
                    .is_some_and(|t| !t.is_finite() || !(0.0..=150.0).contains(&t))
                || d.wear_used_percent.is_some_and(|w| w > 255)
        })
    {
        return Err(io::Error::other("invalid storage response"));
    }
    v.observed_at = now();
    Ok(v)
}
fn convert(raw: serde_json::Value) -> View {
    if raw["state"] != "ok" {
        return view("read_error");
    }
    let Some(rows) = raw["disks"].as_array() else {
        return view("read_error");
    };
    if rows.len() > 16 {
        return view("read_error");
    }
    let mut disks = Vec::new();
    for d in rows {
        let Some(number) = d["disk_number"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
        else {
            continue;
        };
        let r = &d["reliability"];
        let sample = r["rows"]
            .as_array()
            .filter(|r| r.len() == 1)
            .and_then(|r| r.first());
        let ok = r["state"] == "ok" && r["return_code"] == 0 && sample.is_some();
        let t = sample.and_then(|s| s["Temperature"].as_u64()).filter(|&t| {
            t <= 150
                && (t > 0
                    || sample
                        .and_then(|s| s["TemperatureMax"].as_u64())
                        .unwrap_or(0)
                        > 0)
        });
        disks.push(Disk {
            number,
            size_bytes: d["size_bytes"].as_u64().unwrap_or(0),
            series_uid: None,
            name: d["name"]
                .as_str()
                .unwrap_or("Disk")
                .chars()
                .filter(|c| !c.is_control())
                .take(256)
                .collect(),
            temperature_c: if ok { t.map(|t| t as f64) } else { None },
            health: d["health"].as_u64().and_then(|v| u16::try_from(v).ok()),
            wear_used_percent: if ok {
                sample
                    .and_then(|s| s["Wear"].as_u64())
                    .filter(|w| *w <= 255)
            } else {
                None
            },
            power_on_hours: if ok {
                sample.and_then(|s| s["PowerOnHours"].as_u64())
            } else {
                None
            },
            state: if ok { "ok" } else { "unavailable" }.into(),
        });
    }
    View {
        state: "running".into(),
        observed_at: now(),
        disks,
        history_lost: 0,
    }
}
pub fn status() -> View {
    CONTROL
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| {
            let mut view = c.view.lock().unwrap().clone();
            view.history_lost = c.history_lost.load(Ordering::Relaxed);
            view
        })
        .unwrap_or_else(|| view("disabled"))
}
pub fn stop() {
    if let Some(c) = CONTROL.lock().unwrap().as_ref() {
        c.stop.store(true, Ordering::SeqCst);
        *c.view.lock().unwrap() = view("stopping");
        c.wake.notify_one();
    }
}
pub(crate) fn launch_for(
    flag: &str,
    name: String,
    nonce: String,
) -> io::Result<(OwnedHandle, u32)> {
    let exe = std::env::current_exe()?;
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
    let args: Vec<u16> = format!("{flag} {name} {} {nonce}", std::process::id())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: args.as_ptr(),
        nShow: 0,
        ..Default::default()
    };
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.hProcess.is_null() {
        return Err(io::Error::other("missing companion process"));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let pid = unsafe { GetProcessId(handle.as_raw_handle()) };
    Ok((handle, pid))
}
pub async fn start() -> Result<(), String> {
    let c = Arc::new(Control {
        stop: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        wake: Notify::new(),
        view: Mutex::new(view("starting")),
        history_lost: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    });
    {
        let mut slot = CONTROL.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|c| !c.finished.load(Ordering::SeqCst))
        {
            return Err("enhanced collection already active".into());
        }
        *slot = Some(c.clone());
    }
    tauri::async_runtime::spawn(async move {
        let result = run(c.clone()).await;
        if let Err(error) = &result {
            log::warn!("enhanced storage session ended: {error}");
        }
        *c.view.lock().unwrap() = view(if result.is_err() { "error" } else { "disabled" });
        c.finished.store(true, Ordering::SeqCst);
    });
    Ok(())
}
async fn run(c: Arc<Control>) -> io::Result<()> {
    let (name, mut server) = pipe::create()?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let launch_nonce = nonce.clone();
    let (handle, pid) = tokio::task::spawn_blocking(move || launch_for(FLAG, name, launch_nonce))
        .await
        .map_err(io::Error::other)??;
    let result=async {
        let peer=pipe::Peer::open(pid)?;pipe::accept(&server,&peer).await?;
        send(&mut server,&Request::Hello{nonce,version:STORAGE_PROTOCOL_VERSION}).await?;
        if !matches!(receive::<Reply,_>(&mut server,Duration::from_secs(5)).await?,Reply::Ready{version} if version==STORAGE_PROTOCOL_VERSION){return Err(io::Error::other("invalid handshake"));}
        let session=uuid::Uuid::new_v4().to_string();let mut sequence=0u64;
        loop {
            if c.stop.load(Ordering::SeqCst){break;}
            sequence+=1;
            let held=crate::sampler::temperature_targets();
            let targets:Vec<_>=held.iter().map(|t|Target{number:t.number,uid:t.uid.clone(),name:t.name.clone(),size_bytes:t.size_bytes}).collect();
            send(&mut server,&Request::Read{targets}).await?;
            let reply=tokio::select!{r=receive::<Reply,_>(&mut server,Duration::from_secs(14))=>r?,_=c.wake.notified()=>break};
            let Reply::Sample{view}=reply else{return Err(io::Error::other("invalid sample"));};
            let mut next=checked(view)?;
            let mut samples=Vec::new();
            let current=crate::sampler::temperature_targets();
            for disk in &mut next.disks {
                let valid=held.iter().any(|target|Some(target.uid.as_str())==disk.series_uid.as_deref()&&target.number==disk.number&&target.name==disk.name&&target.size_bytes==disk.size_bytes&&target.lease.valid()&&current.iter().any(|t|t.number==target.number&&t.uid==target.uid&&t.lease.valid()));
                if !valid{disk.series_uid=None;}
                if let (Some(uid),Some(celsius))=(&disk.series_uid,disk.temperature_c){samples.push(super::temperature_history::Sample{uid:uid.clone(),celsius});}
            }
            if !c.stop.load(Ordering::SeqCst){
                let observed=next.observed_at as i64;*c.view.lock().unwrap()=next;
                if !samples.is_empty(){super::temperature_history::enqueue(super::temperature_history::Batch{metric:"disk.temperature",session:session.clone(),sequence,observed,samples,lost:c.history_lost.clone()});}
            }
            tokio::select!{_=tokio::time::sleep(Duration::from_secs(5))=>{},_=c.wake.notified()=>break};
        }
        Ok(())
    }.await;
    drop(server); // Companion sees EOF, even if a sample was in flight.
    let exited = tokio::task::spawn_blocking(move || unsafe {
        WaitForSingleObject(handle.as_raw_handle(), 15000) == WAIT_OBJECT_0
    })
    .await
    .unwrap_or(false);
    if !exited {
        return Err(io::Error::other("companion exit unconfirmed"));
    }
    result
}
async fn read_worker() -> View {
    let result = async {
        let mut child = tokio::process::Command::new(std::env::current_exe()?)
            .arg("--storage-companion-read")
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut bytes = Vec::new();
        let mut stdout = child.stdout.take().unwrap().take(65537);
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            stdout.read_to_end(&mut bytes).await?;
            child.wait().await
        })
        .await;
        match result {
            Ok(Ok(status)) if status.success() && bytes.len() <= 65536 => {
                serde_json::from_slice(&bytes).map_err(io::Error::other)
            }
            _ => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(io::Error::other("storage query failed"))
            }
        }
    }
    .await;
    result.unwrap_or_else(|_| view("read_error"))
}
async fn companion(args: &[String]) -> io::Result<()> {
    if args.len() != 5 || args[4].len() != 32 || !args[4].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid companion arguments"));
    }
    let pid = args[3]
        .parse()
        .map_err(|_| io::Error::other("invalid parent"))?;
    let peer = pipe::Peer::open(pid)?;
    let mut client = pipe::connect(&args[2], &peer)?;
    let hello: Request = receive(&mut client, Duration::from_secs(5)).await?;
    if !matches!(hello,Request::Hello{nonce,version} if nonce==args[4]&&version==STORAGE_PROTOCOL_VERSION)
    {
        return Err(io::Error::other("invalid handshake"));
    }
    send(
        &mut client,
        &Reply::Ready {
            version: STORAGE_PROTOCOL_VERSION,
        },
    )
    .await?;
    loop {
        match receive::<Request, _>(&mut client, Duration::from_secs(20)).await? {
            Request::Read { targets } => {
                if targets.len() > 16
                    || targets.iter().any(|t| {
                        t.number > 4095
                            || t.name.chars().count() > 256
                            || t.size_bytes == 0
                            || uuid::Uuid::parse_str(&t.uid).is_err()
                    })
                {
                    return Err(io::Error::other("invalid disk targets"));
                }
                let held: Vec<_> = targets
                    .into_iter()
                    .filter_map(|t| {
                        crate::platform::windows::storage_native::DiskLease::open(t.number)
                            .ok()
                            .map(|lease| (t, lease))
                    })
                    .collect();
                let mut snapshot = read_worker().await;
                for disk in &mut snapshot.disks {
                    if let Some((target, _)) = held.iter().find(|(t, l)| {
                        t.number == disk.number
                            && t.name == disk.name
                            && t.size_bytes == disk.size_bytes
                            && l.valid()
                    }) {
                        disk.series_uid = Some(target.uid.clone());
                    }
                }
                send(&mut client, &Reply::Sample { view: snapshot }).await?
            }
            Request::Stop {} => {
                send(&mut client, &Reply::Stopped {}).await?;
                return Ok(());
            }
            _ => return Err(io::Error::other("unexpected command")),
        }
    }
}
pub fn entry() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--storage-companion-read") {
        if args.len() != 2 {
            std::process::exit(1);
        }
        println!(
            "{}",
            serde_json::to_string(&convert(super::probe::storage_method_probe())).unwrap()
        );
        return true;
    }
    if args.get(1).map(String::as_str) == Some(FLAG) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _ = runtime.block_on(companion(&args));
        return true;
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_mapping_preserves_unknown_zero_wear_and_failure() {
        let v = convert(
            serde_json::json!({"state":"ok","disks":[{"disk_number":0,"name":"disk","health":0,"reliability":{"state":"ok","return_code":0,"rows":[{"Temperature":56,"TemperatureMax":90,"Wear":0,"PowerOnHours":null}]}},{"disk_number":2,"name":"usb","reliability":{"state":"ok","return_code":0,"rows":[{"Temperature":0,"TemperatureMax":0,"Wear":0}]}}]}),
        );
        assert_eq!(v.disks[0].temperature_c, Some(56.0));
        assert_eq!(v.disks[0].wear_used_percent, Some(0));
        assert!(v.disks[0].power_on_hours.is_none());
        assert!(v.disks[1].temperature_c.is_none());
        assert_eq!(
            convert(serde_json::json!({"state":"query_error"})).state,
            "read_error"
        );
    }
}
