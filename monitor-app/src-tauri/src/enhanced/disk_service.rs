//! Explicitly enabled, authenticated ETW companion. No process paths or history writes.
use super::{
    disk_snapshot::{Assembly, Chunk, Reason, Snapshot, State, View},
    pipe,
    protocol::{receive, send},
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{GetExitCodeProcess, WaitForSingleObject},
};
const FLAG: &str = "--process-disk-companion";
const VERSION: u32 = 1;
const FRESHNESS: Duration = Duration::from_secs(5);
#[derive(Serialize, Deserialize)]
#[serde(tag = "command", deny_unknown_fields)]
enum Body {
    Hello {
        nonce: String,
        version: u32,
        validation: bool,
    },
    Read {},
    Stop {},
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    sequence: u64,
    body: Body,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "reply", deny_unknown_fields)]
enum Answer {
    Ready { version: u32 },
    Snapshot { chunk: Chunk },
    Stopped { clean: bool },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    sequence: u64,
    body: Answer,
}
struct Cache {
    view: View,
    snapshot: Option<Snapshot>,
    received: Option<Instant>,
}
struct Control {
    stop: AtomicBool,
    finished: AtomicBool,
    wake: Notify,
    cache: Mutex<Cache>,
}
static CONTROL: Mutex<Option<Arc<Control>>> = Mutex::new(None);
pub fn status() -> View {
    current()
        .map(|c| effective(&c.cache.lock().unwrap()))
        .unwrap_or_else(|| View::state(State::Disabled, Reason::None))
}
fn current() -> Option<Arc<Control>> {
    CONTROL.lock().unwrap().clone()
}
fn effective(cache: &Cache) -> View {
    let mut v = cache.view.clone();
    if matches!(v.state, State::Ready | State::Incomplete) {
        let age = cache.received.map(|t| t.elapsed());
        v.age_ms = age.map(|a| a.as_millis().min(u64::MAX as u128) as u64);
        if age.is_none_or(|a| a > FRESHNESS) {
            v.state = State::Stale;
            v.known_processes = 0;
        }
    }
    v
}
pub fn stop() {
    if let Some(c) = current() {
        if c.finished.load(Ordering::SeqCst) {
            return;
        }
        c.stop.store(true, Ordering::SeqCst);
        let mut cache = c.cache.lock().unwrap();
        cache.view = View::state(State::Stopping, Reason::None);
        cache.snapshot = None;
        cache.received = None;
        c.wake.notify_one();
    }
}
pub async fn start(validation: bool) -> Result<(), String> {
    let c = Arc::new(Control {
        stop: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        wake: Notify::new(),
        cache: Mutex::new(Cache {
            view: View::state(State::Starting, Reason::None),
            snapshot: None,
            received: None,
        }),
    });
    {
        let mut slot = CONTROL.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|c| !c.finished.load(Ordering::SeqCst))
        {
            return Err("Process disk collection is already active".into());
        }
        *slot = Some(c.clone());
    }
    tauri::async_runtime::spawn(async move {
        let result = run(c.clone(), validation).await;
        let mut cache = c.cache.lock().unwrap();
        cache.snapshot = None;
        cache.received = None;
        match result {
            Ok(()) if cache.view.state != State::Error => {
                cache.view = View::state(State::Disabled, Reason::None)
            }
            Ok(()) => {}
            Err((error, reason, exited)) => {
                log::warn!("Process disk companion ended: {error}");
                cache.view = View::state(State::Error, reason);
                // Refuse a second elevated source if cleanup/exit was not confirmed.
                c.finished.store(exited, Ordering::SeqCst);
                return;
            }
        }
        c.finished.store(true, Ordering::SeqCst);
    });
    Ok(())
}
fn install(cache: &mut Cache, snapshot: Snapshot) -> io::Result<()> {
    snapshot.validate()?;
    if let Some(old) = &cache.snapshot {
        if snapshot.header.session != old.header.session
            || snapshot.header.sequence < old.header.sequence
        {
            return Err(io::Error::other("process disk snapshot replay"));
        }
        if snapshot.header.sequence == old.header.sequence {
            if snapshot != *old {
                return Err(io::Error::other(
                    "process disk snapshot changed within sequence",
                ));
            }
            return Ok(()); // Polling the same snapshot must not refresh its age.
        }
    }
    let h = &snapshot.header;
    cache.view = View {
        state: h.state,
        reason: h.reason,
        verified_subset_only: true,
        window_ms: (h.end_ns > h.start_ns).then(|| ((h.end_ns - h.start_ns) / 1_000_000) as u32),
        age_ms: Some(0),
        known_processes: snapshot
            .rows
            .iter()
            .filter(|r| r.read_bps.is_some())
            .count(),
        quality: h.quality,
        identities: h.identities.clone(),
    };
    cache.received = Some(Instant::now());
    cache.snapshot = Some(snapshot);
    Ok(())
}
/// Called on the FULL basic process scan, before filtering, ranking or pagination.
pub fn merge(page: &mut crate::processes::ProcessPage) {
    for p in &mut page.processes {
        p.disk_read_bps = None;
        p.disk_write_bps = None;
        p.io_ok = false;
    }
    let Some(c) = current() else {
        page.windows_disk_io = Some(View::state(State::Disabled, Reason::None));
        return;
    };
    let cache = c.cache.lock().unwrap();
    let view = effective(&cache);
    if view.state == State::Ready {
        if let Some(s) = &cache.snapshot {
            merge_rows(&mut page.processes, &s.rows);
        }
    }
    page.windows_disk_io = Some(view);
}
fn merge_rows(processes: &mut [crate::processes::ProcessInfo], rows: &[super::disk_snapshot::Row]) {
    let map: std::collections::BTreeMap<_, _> = rows
        .iter()
        .map(|r| ((r.pid, r.creation.as_str()), r))
        .collect();
    for p in processes {
        if let Some(r) = p
            .start_marker
            .as_deref()
            .and_then(|marker| map.get(&(p.pid, marker)))
        {
            p.disk_read_bps = r.read_bps;
            p.disk_write_bps = r.write_bps;
            p.io_ok = r.read_bps.is_some() && r.write_bps.is_some();
        }
    }
}
async fn run(c: Arc<Control>, validation: bool) -> Result<(), (io::Error, Reason, bool)> {
    let (name, mut server) = pipe::create().map_err(|e| (e, Reason::StartFailed, true))?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let launch_nonce = nonce.clone();
    let (handle, pid): (OwnedHandle, u32) = tokio::task::spawn_blocking(move || {
        super::storage_service::launch_for(FLAG, name, launch_nonce)
    })
    .await
    .map_err(|e| (io::Error::other(e), Reason::StartFailed, true))?
    .map_err(|e| (e, Reason::PermissionRequired, true))?;
    let result=async {
        let peer=pipe::Peer::open(pid)?;pipe::accept(&server,&peer).await?;
        send(&mut server,&Request {sequence:0,body:Body::Hello {nonce:nonce.clone(),version:VERSION,validation}}).await?;
        let reply:Reply=receive(&mut server,Duration::from_secs(5)).await?;
        if reply.sequence!=0||!matches!(reply.body,Answer::Ready {version} if version==VERSION) {return Err(io::Error::other("process disk handshake mismatch"));}
        let mut sequence=0u64;
        loop {
            if c.stop.load(Ordering::SeqCst) {break;}
            sequence=sequence.checked_add(1).ok_or_else(||io::Error::other("process disk sequence exhausted"))?;
            send(&mut server,&Request {sequence,body:Body::Read {}}).await?;
            let mut assembly=Assembly::default();
            let deadline=tokio::time::Instant::now()+Duration::from_secs(4);
            let snapshot=loop {
                let reply:Reply=tokio::time::timeout_at(deadline,receive(&mut server,Duration::from_secs(4))).await.map_err(|_|io::Error::other("process disk transfer timeout"))??;
                if reply.sequence!=sequence {return Err(io::Error::other("process disk reply sequence mismatch"));}
                let Answer::Snapshot {chunk}=reply.body else {return Err(io::Error::other("unexpected process disk reply"));};
                if let Some(value)=assembly.push(chunk)? {break value;}
            };
            if snapshot.header.session!=uuid::Uuid::parse_str(&nonce).unwrap().to_string() {return Err(io::Error::other("process disk session mismatch"));}
            let state=snapshot.header.state;
            { let mut cache=c.cache.lock().unwrap();if !c.stop.load(Ordering::SeqCst) {install(&mut cache,snapshot)?;} }
            if matches!(state,State::Error|State::Disabled) {break;}
            tokio::select! { _=tokio::time::sleep(Duration::from_secs(1))=>{}, _=c.wake.notified()=>break }
        }
        sequence=sequence.checked_add(1).ok_or_else(||io::Error::other("process disk sequence exhausted"))?;
        send(&mut server,&Request {sequence,body:Body::Stop {}}).await?;
        let reply:Reply=receive(&mut server,Duration::from_secs(8)).await?;
        if reply.sequence!=sequence||!matches!(reply.body,Answer::Stopped {clean:true}) {return Err(io::Error::other("process disk cleanup unconfirmed"));}
        Ok(())
    }.await;
    drop(server); // EOF also cancels the collector if any operation above failed.
    let (exited, clean_exit) = tokio::task::spawn_blocking(move || unsafe {
        let exited = WaitForSingleObject(handle.as_raw_handle(), 10000) == WAIT_OBJECT_0;
        let mut code = u32::MAX;
        let queried = GetExitCodeProcess(handle.as_raw_handle(), &mut code) != 0;
        (exited, exited && queried && code == 0)
    })
    .await
    .unwrap_or((false, false));
    if !exited {
        return Err((
            io::Error::other("process disk companion exit unconfirmed"),
            Reason::CleanupUnconfirmed,
            false,
        ));
    }
    if !clean_exit {
        return Err((
            io::Error::other("process disk cleanup exit unconfirmed"),
            Reason::CleanupUnconfirmed,
            false,
        ));
    }
    result.map_err(|e| (e, Reason::SourceEnded, true))
}
async fn companion(args: &[String]) -> io::Result<()> {
    if args.len() != 5 || args[4].len() != 32 || !args[4].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid process disk arguments"));
    }
    let pid = args[3]
        .parse()
        .map_err(|_| io::Error::other("invalid process disk parent"))?;
    let parent = pipe::Peer::open(pid)?;
    let mut client = pipe::connect(&args[2], &parent)?;
    let hello: Request = receive(&mut client, Duration::from_secs(5)).await?;
    let Body::Hello {
        nonce,
        version,
        validation,
    } = hello.body
    else {
        return Err(io::Error::other("invalid process disk hello"));
    };
    if hello.sequence != 0 || version != VERSION || nonce != args[4] {
        return Err(io::Error::other("process disk handshake mismatch"));
    }
    let id = uuid::Uuid::parse_str(&nonce)
        .map_err(io::Error::other)?
        .to_string();
    send(
        &mut client,
        &Reply {
            sequence: 0,
            body: Answer::Ready { version: VERSION },
        },
    )
    .await?;
    let mut initial = Snapshot::empty(&id, State::WarmingUp, Reason::None);
    initial.header.sequence = 1;
    let latest = Arc::new(Mutex::new(initial));
    let stopped = Arc::new(AtomicBool::new(false));
    let cancelled = stopped.clone();
    let samples = latest.clone();
    let worker = std::thread::spawn(move || {
        super::trace_run::collect(
            &format!("HardwareMonitor-ProcessDisk-{nonce}"),
            &cancelled,
            validation.then_some(Duration::from_secs(20)),
            |snapshot| *samples.lock().unwrap() = snapshot,
        )
    });
    let mut expected = 1u64;
    let result = async {
        loop {
            // The lease is independent of event activity and still expires when the GUI disappears.
            let request: Request = receive(&mut client, Duration::from_secs(10)).await?;
            if request.sequence != expected {
                return Err(io::Error::other("process disk request replay"));
            }
            expected = expected
                .checked_add(1)
                .ok_or_else(|| io::Error::other("process disk sequence exhausted"))?;
            match request.body {
                Body::Read {} => {
                    let chunks = latest.lock().unwrap().chunks();
                    for chunk in chunks {
                        send(
                            &mut client,
                            &Reply {
                                sequence: request.sequence,
                                body: Answer::Snapshot { chunk },
                            },
                        )
                        .await?;
                    }
                }
                Body::Stop {} => return Ok(request.sequence),
                Body::Hello { .. } => {
                    return Err(io::Error::other("unexpected process disk command"))
                }
            }
        }
    }
    .await;
    stopped.store(true, Ordering::SeqCst);
    let report = tokio::task::spawn_blocking(move || worker.join())
        .await
        .map_err(io::Error::other)?
        .map_err(|_| io::Error::other("process disk source panicked"))?;
    let clean = report["started"] != true
        || (report["stop_code"] == 0 && report["consumer_joined"] == true);
    if let Ok(sequence) = result {
        send(
            &mut client,
            &Reply {
                sequence,
                body: Answer::Stopped { clean },
            },
        )
        .await?;
    }
    if !clean {
        return Err(io::Error::other("ETW cleanup unconfirmed"));
    }
    Ok(())
}
pub fn entry() -> bool {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(FLAG) {
        return false;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(companion(&args));
    if result.is_err() {
        std::process::exit(1);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_requires_creation_and_repeated_snapshots_do_not_refresh_freshness() {
        let row = super::super::disk_snapshot::Row {
            pid: 1,
            creation: "99".into(),
            read_bps: Some(0.0),
            write_bps: Some(12.0),
        };
        let make = |marker: &str| crate::processes::ProcessInfo {
            pid: 1,
            start_marker: Some(marker.into()),
            name: "synthetic".into(),
            memory_bytes: None,
            cpu_usage: None,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            disk_read_bps: None,
            disk_write_bps: None,
            io_ok: false,
        };
        let mut processes = vec![make("99"), make("100")];
        merge_rows(&mut processes, std::slice::from_ref(&row));
        assert_eq!(processes[0].disk_read_bps, Some(0.0));
        assert!(processes[0].io_ok);
        assert_eq!(processes[1].disk_read_bps, None);
        let mut s = Snapshot::empty(
            "01234567-89ab-cdef-0123-456789abcdef",
            State::Ready,
            Reason::None,
        );
        s.header.sequence = 1;
        s.header.end_ns = 1_000_000_000;
        s.header.total = 1;
        s.rows = vec![row];
        let mut cache = Cache {
            view: View::state(State::Disabled, Reason::None),
            snapshot: None,
            received: None,
        };
        install(&mut cache, s.clone()).unwrap();
        cache.received = Some(Instant::now() - Duration::from_secs(6));
        let age = cache.received;
        install(&mut cache, s.clone()).unwrap();
        assert_eq!(cache.received, age);
        assert_eq!(effective(&cache).state, State::Stale);
        s.header.sequence = 2;
        s.header.state = State::Incomplete;
        for r in &mut s.rows {
            r.read_bps = None;
            r.write_bps = None;
        }
        install(&mut cache, s).unwrap();
        assert_eq!(effective(&cache).state, State::Incomplete);
        assert_eq!(cache.view.known_processes, 0);
    }
}
