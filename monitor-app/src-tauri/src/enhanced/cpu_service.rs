//! User-started CPU-only companion; neither installation nor main-UI elevation occurs here.
use super::{
    cpu_read::Observation,
    pipe,
    protocol::{receive, send},
    temperature_history,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    os::windows::io::{AsRawHandle, OwnedHandle},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;
use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};
const FLAG: &str = "--cpu-companion";
const VERSION: u32 = 1;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub uid: String,
    pub name: String,
    pub sensor: String,
    pub created_at: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    version: u32,
    records: Vec<Series>,
}
struct Registry {
    path: PathBuf,
    catalog: Option<Catalog>,
    current: Option<Series>,
}
static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
#[derive(Clone, Serialize)]
pub struct View {
    pub state: String,
    pub observed_at: u64,
    pub celsius: Option<f64>,
    pub reason: String,
    pub history_uid: Option<String>,
    pub history_series: Vec<Series>,
    pub history_lost: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
enum Request {
    Hello { nonce: String, version: u32 },
    Read {},
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "reply", deny_unknown_fields)]
enum Reply {
    Ready { version: u32 },
    Sample { reading: Observation },
}
struct Control {
    stop: AtomicBool,
    finished: AtomicBool,
    wake: Notify,
    view: Mutex<View>,
    lost: Arc<AtomicU64>,
}
static CONTROL: Mutex<Option<Arc<Control>>> = Mutex::new(None);
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
fn view(state: &str) -> View {
    View {
        state: state.into(),
        observed_at: 0,
        celsius: None,
        reason: String::new(),
        history_uid: None,
        history_series: vec![],
        history_lost: 0,
    }
}
pub fn initialize(root: PathBuf) {
    let path = root.join("cpu-temperature-series.json");
    let catalog = (|| -> Option<Catalog> {
        let mut bytes = Vec::new();
        match std::fs::File::open(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Some(Catalog {
                    version: 1,
                    records: vec![],
                })
            }
            Ok(file) => {
                file.take(131073).read_to_end(&mut bytes).ok()?;
            }
            Err(_) => return None,
        }
        if bytes.len() > 131072 {
            return None;
        }
        let catalog: Catalog = serde_json::from_slice(&bytes).ok()?;
        if catalog.version != 1
            || catalog.records.len() > 512
            || catalog.records.iter().enumerate().any(|(i, r)| {
                uuid::Uuid::parse_str(&r.uid).is_err()
                    || r.name.len() > 256
                    || r.sensor != "Tctl"
                    || r.created_at == 0
                    || catalog.records[..i].iter().any(|p| p.uid == r.uid)
            })
        {
            return None;
        }
        Some(catalog)
    })();
    *REGISTRY.lock().unwrap() = Some(Registry {
        path,
        catalog,
        current: None,
    });
}
fn series(name: &str) -> Option<String> {
    let mut slot = REGISTRY.lock().unwrap();
    let registry = slot.as_mut()?;
    if let Some(s) = &registry.current {
        if s.name == name {
            return Some(s.uid.clone());
        }
    }
    let catalog = registry.catalog.as_mut()?;
    if catalog.records.len() >= 512 {
        return None;
    }
    let record = Series {
        uid: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        sensor: "Tctl".into(),
        created_at: now(),
    };
    if record.created_at == 0 {
        return None;
    }
    catalog.records.push(record.clone());
    let written = (|| -> io::Result<()> {
        let bytes = serde_json::to_vec(catalog)?;
        if bytes.len() > 131072 {
            return Err(io::Error::other("CPU series capacity exceeded"));
        }
        let temporary = registry
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        let result = (|| -> io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let from: Vec<_> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<_> = registry
                .path
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    })();
    if written.is_err() {
        catalog.records.pop();
        return None;
    }
    registry.current = Some(record.clone());
    Some(record.uid)
}
pub fn status() -> View {
    let mut current = CONTROL
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| {
            let mut v = c.view.lock().unwrap().clone();
            v.history_lost = c.lost.load(Ordering::Relaxed);
            v
        })
        .unwrap_or_else(|| view("disabled"));
    if let Some(registry) = REGISTRY.lock().unwrap().as_ref() {
        current.history_uid = registry.current.as_ref().map(|s| s.uid.clone());
        current.history_series = registry
            .catalog
            .as_ref()
            .map(|c| c.records.clone())
            .unwrap_or_default();
    }
    current
}
pub fn stop() {
    if let Some(c) = CONTROL.lock().unwrap().as_ref() {
        c.stop.store(true, Ordering::SeqCst);
        *c.view.lock().unwrap() = view("stopping");
        c.wake.notify_one();
    }
}
pub async fn start() -> Result<(), String> {
    let c = Arc::new(Control {
        stop: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        wake: Notify::new(),
        view: Mutex::new(view("starting")),
        lost: Arc::new(AtomicU64::new(0)),
    });
    {
        let mut slot = CONTROL.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|c| !c.finished.load(Ordering::SeqCst))
        {
            return Err("CPU collection is already active".into());
        }
        *slot = Some(c.clone());
    }
    tauri::async_runtime::spawn(async move {
        let result = run(c.clone()).await;
        if let Err(e) = &result {
            log::warn!("CPU companion ended: {e}");
        }
        *c.view.lock().unwrap() = view(if result.is_ok() { "disabled" } else { "error" });
        c.finished.store(true, Ordering::SeqCst);
    });
    Ok(())
}
fn validate(reading: &Observation) -> io::Result<()> {
    if reading.cpu_name.len() > 256
        || reading.sensor != "Tctl"
        || reading.family != 0x1A
        || reading.model != 0x24
        || reading.state.len() > 64
        || (reading.state == "ok" && (reading.error_code.is_some() || reading.celsius.is_none()))
        || reading
            .celsius
            .is_some_and(|t| reading.state != "ok" || !t.is_finite() || t <= 0.0 || t > 150.0)
    {
        return Err(io::Error::other("invalid CPU sample"));
    }
    Ok(())
}
async fn run(c: Arc<Control>) -> io::Result<()> {
    let (name, mut server) = pipe::create()?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let other_nonce = nonce.clone();
    let (handle, _): (OwnedHandle, u32) = tokio::task::spawn_blocking(move || {
        super::storage_service::launch_for(FLAG, name, other_nonce)
    })
    .await
    .map_err(io::Error::other)??;
    let result=async{
        let pid=unsafe{windows_sys::Win32::System::Threading::GetProcessId(handle.as_raw_handle())};
        let peer=pipe::Peer::open(pid)?;pipe::accept(&server,&peer).await?;
        send(&mut server,&Request::Hello{nonce,version:VERSION}).await?;
        if !matches!(receive::<Reply,_>(&mut server,Duration::from_secs(5)).await?,Reply::Ready{version} if version==VERSION){return Err(io::Error::other("CPU handshake mismatch"));}
        let session=uuid::Uuid::new_v4().to_string();let mut sequence=0u64;
        loop{
            if c.stop.load(Ordering::SeqCst){break;}
            sequence=sequence.checked_add(1).ok_or_else(||io::Error::other("CPU sequence exhausted"))?;
            send(&mut server,&Request::Read{}).await?;
            let reply=tokio::select!{r=receive::<Reply,_>(&mut server,Duration::from_secs(6))=>r?,_=c.wake.notified()=>break};
            let Reply::Sample{reading}=reply else{return Err(io::Error::other("unexpected CPU reply"));};validate(&reading)?;
            let mut next=view(if reading.state=="ok"{"running"}else{"read_error"});next.reason=reading.state.clone();
            let verified=crate::sampler::latest_snapshot().and_then(|s|s.cpu).is_some_and(|cpu|cpu.name.trim()==reading.cpu_name.trim());
            if reading.state=="ok"&&verified{
                next.celsius=reading.celsius;next.observed_at=now();
                if let (Some(uid),Some(celsius))=(series(&reading.cpu_name),reading.celsius){
                    if !c.stop.load(Ordering::SeqCst){temperature_history::enqueue(temperature_history::Batch{metric:"cpu.temperature.tctl",session:session.clone(),sequence,observed:next.observed_at as i64,samples:vec![temperature_history::Sample{uid,celsius}],lost:c.lost.clone()});}
                }else{c.lost.fetch_add(1,Ordering::Relaxed);}
            }else if reading.state=="ok"{next.state="read_error".into();next.reason="identity_unverified".into();}
            if !c.stop.load(Ordering::SeqCst){*c.view.lock().unwrap()=next;}
            let period=if crate::sampler::window_visible(){2}else{5};
            tokio::select!{_=tokio::time::sleep(Duration::from_secs(period))=>{},_=c.wake.notified()=>break};
        }Ok(())
    }.await;
    drop(server);
    let exited = tokio::task::spawn_blocking(move || unsafe {
        WaitForSingleObject(handle.as_raw_handle(), 15000) == WAIT_OBJECT_0
    })
    .await
    .unwrap_or(false);
    if !exited {
        return Err(io::Error::other("CPU companion exit unconfirmed"));
    }
    result
}
async fn companion(args: &[String]) -> io::Result<()> {
    if args.len() != 5 || args[4].len() != 32 || !args[4].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid CPU arguments"));
    }
    let pid = args[3]
        .parse()
        .map_err(|_| io::Error::other("invalid CPU parent"))?;
    let parent = pipe::Peer::open(pid)?;
    let mut client = pipe::connect(&args[2], &parent)?;
    if !matches!(receive::<Request,_>(&mut client,Duration::from_secs(5)).await?,Request::Hello{nonce,version} if nonce==args[4]&&version==VERSION)
    {
        return Err(io::Error::other("CPU handshake mismatch"));
    }
    send(&mut client, &Reply::Ready { version: VERSION }).await?;
    let mut reader = super::gpu_temperature::Collector::default();
    loop {
        if !matches!(
            receive::<Request, _>(&mut client, Duration::from_secs(20)).await?,
            Request::Read {}
        ) {
            return Err(io::Error::other("unexpected CPU command"));
        }
        let (next, result) = tokio::task::spawn_blocking(move || {
            let result = reader.read_cpu_worker();
            (reader, result)
        })
        .await
        .map_err(io::Error::other)?;
        reader = next;
        let reading = result.map_err(|_| io::Error::other("CPU worker failed"))?;
        send(&mut client, &Reply::Sample { reading }).await?;
    }
}
pub fn entry() -> bool {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--cpu-companion-read") {
        if args.len() != 2 {
            std::process::exit(1);
        }
        println!(
            "{}",
            serde_json::to_string(&super::cpu_read::sample()).unwrap()
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
