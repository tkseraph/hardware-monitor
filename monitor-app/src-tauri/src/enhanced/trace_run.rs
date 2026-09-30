//! Limited diagnostic collector. No automatic privilege change, UI attachment or history writes.
use super::{
    clock_bridge::HostClock,
    disk_io::Limits,
    enumeration,
    etw_decode::{self, Event, QpcClock},
    pipeline::{Fault, Phase, Pipeline},
    probe::{Properties, Session},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Diagnostics::Etw::*;
const CAPACITY: usize = 8192;
struct Callback {
    clock: QpcClock,
    sender: SyncSender<Event>,
    accepting: AtomicBool,
    shutdown_ignored: AtomicU64,
    rundown_ignored: AtomicU64,
    decoded: AtomicU64,
    errors: AtomicU64,
    dropped: AtomicU64,
    error_schemas: Mutex<BTreeMap<(u32, u8, u8), u64>>,
}
impl Callback {
    fn forward(&self, event: Event) {
        if !self.accepting.load(Ordering::SeqCst) {
            self.shutdown_ignored.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.decoded.fetch_add(1, Ordering::Relaxed);
        if matches!(
            event.body,
            etw_decode::Body::Process {
                phase: etw_decode::Lifecycle::PresentAtStart | etw_decode::Lifecycle::PresentAtEnd,
                ..
            } | etw_decode::Body::Thread {
                phase: etw_decode::Lifecycle::PresentAtStart | etw_decode::Lifecycle::PresentAtEnd,
                ..
            }
        ) {
            self.rundown_ignored.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.sender.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
unsafe extern "system" fn callback(record: *mut EVENT_RECORD) {
    if record.is_null() || (*record).UserContext.is_null() {
        return;
    }
    let context = &*(*record).UserContext.cast::<Callback>();
    if !context.accepting.load(Ordering::SeqCst) {
        context.shutdown_ignored.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // Nothing may unwind through Windows. Only normalized fields enter the bounded queue.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        etw_decode::decode_record(&*record, &context.clock)
    }));
    match result {
        Ok(Ok(Some(event))) => context.forward(event),
        Ok(Ok(None)) => {}
        _ => {
            context.errors.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut schemas) = context.error_schemas.try_lock() {
                let h = &(*record).EventHeader;
                let key = (
                    h.ProviderId.data1,
                    h.EventDescriptor.Opcode,
                    h.EventDescriptor.Version,
                );
                if schemas.contains_key(&key) || schemas.len() < 16 {
                    let value = schemas.entry(key).or_default();
                    *value = value.saturating_add(1);
                }
            }
        }
    }
}
fn transfer(receiver: &mpsc::Receiver<Event>, pipeline: &mut Pipeline, epoch: u64) -> usize {
    let mut count = 0;
    // Bound each drain so a busy disk cannot starve clock checks or the stop deadline.
    for event in receiver.try_iter().take(CAPACITY) {
        let _ = pipeline.enqueue(epoch, event);
        count += 1;
    }
    count
}
fn final_loss(
    loop_loss: bool,
    dropped: u64,
    decode_errors: u64,
    events_lost: u32,
    buffers_lost: u32,
) -> bool {
    loop_loss || dropped > 0 || decode_errors > 0 || events_lost > 0 || buffers_lost > 0
}
pub fn run(name: &str) -> Value {
    if !name
        .strip_prefix("HardwareMonitor-E2-")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return json!({"state":"invalid_name"});
    }
    let mut clock = match HostClock::new() {
        Ok(c) => c,
        Err(_) => return json!({"state":"clock_error"}),
    };
    let mut properties = Properties::new(name);
    properties.properties.EnableFlags = EVENT_TRACE_FLAG_DISK_IO
        | EVENT_TRACE_FLAG_DISK_IO_INIT
        | EVENT_TRACE_FLAG_PROCESS
        | EVENT_TRACE_FLAG_THREAD;
    let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
    let start = unsafe {
        StartTraceW(
            &mut handle,
            properties.name.as_ptr(),
            &mut properties.properties,
        )
    };
    if start != 0 {
        return json!({"started":false,"start_code":start});
    }
    let mut session = Session {
        handle,
        properties,
        stopped: false,
    };
    let (sender, receiver) = mpsc::sync_channel(CAPACITY);
    let context = Arc::new(Callback {
        clock: *clock.qpc(),
        sender,
        accepting: AtomicBool::new(true),
        shutdown_ignored: AtomicU64::new(0),
        rundown_ignored: AtomicU64::new(0),
        decoded: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        error_schemas: Mutex::new(BTreeMap::new()),
    });
    let mut log = EVENT_TRACE_LOGFILEW {
        LoggerName: session.properties.name.as_mut_ptr(),
        Context: Arc::as_ptr(&context) as *mut _,
        ..Default::default()
    };
    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME
        | PROCESS_TRACE_MODE_EVENT_RECORD
        | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    log.Anonymous2.EventRecordCallback = Some(callback);
    let trace = unsafe { OpenTraceW(&mut log) };
    if trace.Value == u64::MAX {
        return json!({"started":true,"state":"open_trace_error","stop_code":session.stop()});
    }
    let keep_alive = context.clone();
    let worker = std::thread::spawn(move || {
        let code = unsafe { ProcessTrace(&trace, 1, ptr::null(), ptr::null()) };
        drop(keep_alive);
        code
    });
    let mut pipeline = Pipeline::new(Limits::default(), CAPACITY);
    let at = clock.now_ns().unwrap_or(0);
    let epoch = pipeline.begin_baseline(at).unwrap();
    let candidate = enumeration::capture(&mut clock, Limits::default());
    let coverage = candidate
        .as_ref()
        .ok()
        .map(|c| serde_json::to_value(&c.coverage).unwrap());
    let mut pending = candidate.ok();
    if pending.is_none() {
        let _ = pipeline.report_fault(epoch, Fault::BaselineInvalid);
    }
    let mut windows = 0u64;
    let mut rejected_windows = 0u64;
    let mut read_bytes = 0u64;
    let mut write_bytes = 0u64;
    let mut last_end = 0;
    let mut loss_seen = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if clock.confirm().is_err() {
            let _ = pipeline.report_fault(epoch, Fault::Clock);
            break;
        }
        let now = match clock.now_ns() {
            Ok(n) => n,
            Err(_) => {
                let _ = pipeline.report_fault(epoch, Fault::Clock);
                break;
            }
        };
        transfer(&receiver, &mut pipeline, epoch);
        let query = unsafe {
            ControlTraceW(
                session.handle,
                ptr::null(),
                &mut session.properties.properties,
                EVENT_TRACE_CONTROL_QUERY,
            )
        };
        if query != 0
            || session.properties.properties.EventsLost > 0
            || session.properties.properties.RealTimeBuffersLost > 0
            || context.errors.load(Ordering::Relaxed) > 0
            || context.dropped.load(Ordering::Relaxed) > 0
        {
            loss_seen = true;
            let _ = pipeline.report_fault(epoch, Fault::EventsLost);
        }
        if pending
            .as_ref()
            .is_some_and(|c| now.saturating_sub(c.end_ns) >= 2_000_000_000)
        {
            let c = pending.take().unwrap();
            last_end = c.end_ns;
            let _ = pipeline.install_candidate(epoch, &c);
        }
        let watermark = now.saturating_sub(2_000_000_000);
        if watermark >= last_end.saturating_add(1_000_000_000) {
            if pipeline.phase() == Phase::Ready {
                match pipeline.window(watermark) {
                    Ok(output) => {
                        windows += 1;
                        for row in output.window.rows {
                            read_bytes = read_bytes.saturating_add(row.read_bytes);
                            write_bytes = write_bytes.saturating_add(row.write_bytes);
                        }
                    }
                    Err(_) => rejected_windows += 1,
                }
            }
            last_end = watermark;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let phase = format!("{:?}", pipeline.phase());
    context.accepting.store(false, Ordering::SeqCst);
    let prebaseline_ignored = pipeline.prebaseline_ignored();
    pipeline.stop();
    let mut shutdown_drained = receiver.try_iter().take(CAPACITY).count();
    let stop = session.stop();
    let close = unsafe { CloseTrace(trace) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while !worker.is_finished() && Instant::now() < deadline {
        shutdown_drained += receiver.try_iter().take(CAPACITY).count();
        std::thread::sleep(Duration::from_millis(20));
    }
    let consumer = if worker.is_finished() {
        worker.join().ok()
    } else {
        None
    };
    shutdown_drained += receiver.try_iter().take(CAPACITY).count();
    let final_loss = final_loss(
        loss_seen,
        context.dropped.load(Ordering::Relaxed),
        context.errors.load(Ordering::Relaxed),
        session.properties.properties.EventsLost,
        session.properties.properties.RealTimeBuffersLost,
    );
    let schemas:Vec<_>=context.error_schemas.lock().map(|s|s.iter().map(|(&(provider,opcode,version),&count)|json!({"provider_family":provider,"opcode":opcode,"version":version,"count":count})).collect()).unwrap_or_default();
    json!({"started":true,"start_code":start,"decode_error_schemas":schemas,"stop_code":stop,"close_code":close,"consumer_code":consumer,"consumer_joined":consumer.is_some(),"phase_before_stop":phase,"coverage":coverage,"decoded_events":context.decoded.load(Ordering::Relaxed),"decode_errors":context.errors.load(Ordering::Relaxed),"queue_dropped":context.dropped.load(Ordering::Relaxed),"events_lost":session.properties.properties.EventsLost,"buffers_lost":session.properties.properties.RealTimeBuffersLost,"observed_ready_windows":windows,"rejected_windows":rejected_windows,"observed_subset_read_bytes":read_bytes,"observed_subset_write_bytes":write_bytes,"loss_observed":final_loss,"loss_observed_during_loop":loss_seen,"final_counters_complete":consumer.is_some(),"shutdown_ignored":context.shutdown_ignored.load(Ordering::Relaxed),"shutdown_drained":shutdown_drained,"rundown_ignored":context.rundown_ignored.load(Ordering::Relaxed),"prebaseline_ignored":prebaseline_ignored,"systemwide_stream_ready":false,"pipeline_stopped":true,"scope":"diagnostic subset only; no per-process output or UI publication"})
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::enhanced::{disk_io::Direction, etw_decode::Body};
    fn event() -> Event {
        Event {
            at_ns: 1,
            body: Body::Complete {
                request: 1,
                bytes: 1,
                direction: Direction::Read,
            },
        }
    }
    #[test]
    fn callback_ingress_is_bounded_and_drops_are_visible() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let c = Callback {
            clock: QpcClock::new(0, 1).unwrap(),
            sender,
            accepting: AtomicBool::new(true),
            shutdown_ignored: AtomicU64::new(0),
            rundown_ignored: AtomicU64::new(0),
            decoded: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            error_schemas: Mutex::new(BTreeMap::new()),
        };
        c.forward(event());
        c.forward(event());
        assert_eq!(c.decoded.load(Ordering::Relaxed), 2);
        assert_eq!(c.dropped.load(Ordering::Relaxed), 1);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn shutdown_and_rundown_are_counted_without_overflowing_the_queue() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let c = Callback {
            clock: QpcClock::new(0, 1).unwrap(),
            sender,
            accepting: AtomicBool::new(true),
            shutdown_ignored: AtomicU64::new(0),
            rundown_ignored: AtomicU64::new(0),
            decoded: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            error_schemas: Mutex::new(BTreeMap::new()),
        };
        for _ in 0..10000 {
            c.forward(Event {
                at_ns: 0,
                body: Body::Thread {
                    pid: 1,
                    tid: 2,
                    phase: etw_decode::Lifecycle::PresentAtEnd,
                },
            });
        }
        c.forward(event());
        c.accepting.store(false, Ordering::SeqCst);
        for _ in 0..10000 {
            c.forward(event());
        }
        assert_eq!(c.dropped.load(Ordering::Relaxed), 0);
        assert_eq!(c.rundown_ignored.load(Ordering::Relaxed), 10000);
        assert_eq!(c.shutdown_ignored.load(Ordering::Relaxed), 10000);
        assert_eq!(receiver.try_iter().count(), 1);
    }
    #[test]
    fn final_loss_includes_events_reported_after_the_last_loop_check() {
        assert!(final_loss(false, 3189, 0, 0, 0));
        assert!(final_loss(false, 0, 1, 0, 0));
        assert!(final_loss(false, 0, 0, 1, 0));
        assert!(!final_loss(false, 0, 0, 0, 0));
    }

    #[test]
    fn unknown_provider_callback_does_not_read_payload_or_emit_data() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let c = Callback {
            clock: QpcClock::new(0, 1).unwrap(),
            sender,
            accepting: AtomicBool::new(true),
            shutdown_ignored: AtomicU64::new(0),
            rundown_ignored: AtomicU64::new(0),
            decoded: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            error_schemas: Mutex::new(BTreeMap::new()),
        };
        let mut r = EVENT_RECORD {
            UserContext: (&c as *const Callback) as *mut _,
            ..Default::default()
        };
        unsafe { callback(&mut r) };
        assert!(receiver.try_recv().is_err());
        assert_eq!(c.errors.load(Ordering::Relaxed), 0);
    }
}
