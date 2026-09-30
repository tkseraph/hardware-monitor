//! One fixed, bounded history writer; real-time storage readings never wait on SQLite.
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, OnceLock,
    },
};
#[derive(Clone)]
pub struct Sample {
    pub uid: String,
    pub celsius: f64,
}
pub struct Batch {
    pub metric: &'static str,
    pub session: String,
    pub sequence: u64,
    pub observed: i64,
    pub samples: Vec<Sample>,
    pub lost: Arc<AtomicU64>,
}
static WRITER: OnceLock<SyncSender<Batch>> = OnceLock::new();
#[derive(Default)]
struct Continuity {
    session: String,
    saved: BTreeMap<String, (u64, i64, String)>,
}
impl Continuity {
    fn segment(&mut self, session: &str, sequence: u64, observed: i64, uid: &str) -> String {
        if self.session != session {
            self.saved.clear();
            self.session = session.into();
        }
        match self.saved.get(uid) {
            Some((previous, last, segment))
                if previous.checked_add(1) == Some(sequence)
                    && observed > *last
                    && observed - *last <= 30 =>
            {
                segment.clone()
            }
            _ => uuid::Uuid::new_v4().to_string(),
        }
    }
    fn saved(&mut self, uid: String, sequence: u64, observed: i64, segment: String) {
        self.saved.insert(uid, (sequence, observed, segment));
    }
}
pub fn enqueue(batch: Batch) {
    let sender = WRITER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Batch>(8);
        let spawned = std::thread::Builder::new()
            .name("monitor-temperature-history".into())
            .spawn(move || {
                let mut sources = BTreeMap::<&'static str, Continuity>::new();
                while let Ok(batch) = receiver.recv() {
                    let continuity = sources.entry(batch.metric).or_default();
                    for sample in batch.samples {
                        let segment = continuity.segment(
                            &batch.session,
                            batch.sequence,
                            batch.observed,
                            &sample.uid,
                        );
                        if crate::history::record_segment_batch(
                            batch.observed,
                            &[(batch.metric, sample.uid.as_str(), sample.celsius, "°C")],
                            Some(&segment),
                        )
                        .is_ok()
                        {
                            continuity.saved(sample.uid, batch.sequence, batch.observed, segment);
                        } else {
                            batch.lost.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        if spawned.is_err() {
            log::error!("could not start temperature history writer");
        }
        sender
    });
    let lost = batch.lost.clone();
    if sender.try_send(batch).is_err() {
        lost.fetch_add(1, Ordering::Relaxed);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successful_samples_share_a_segment_but_missing_writes_and_new_sessions_do_not() {
        let mut c = Continuity::default();
        let first = c.segment("session", 1, 1000, "disk-a");
        c.saved("disk-a".into(), 1, 1000, first.clone());
        assert_eq!(c.segment("session", 2, 1005, "disk-a"), first);
        assert_ne!(c.segment("session", 3, 1010, "disk-a"), first);
        assert_ne!(c.segment("session", 2, 1005, "disk-b"), first);
        assert_ne!(c.segment("restarted", 2, 1005, "disk-a"), first);
        // The CPU and disk share one queue but keep independent continuity state.
        let mut sources = BTreeMap::<&'static str, Continuity>::new();
        for metric in ["disk.temperature", "cpu.temperature.tctl"] {
            let c = sources.entry(metric).or_default();
            let segment = c.segment(metric, 1, 1000, "same-object");
            c.saved("same-object".into(), 1, 1000, segment);
        }
        let disk = sources["disk.temperature"].saved["same-object"].2.clone();
        let cpu = sources["cpu.temperature.tctl"].saved["same-object"]
            .2
            .clone();
        assert_ne!(disk, cpu);
        assert_eq!(
            sources.get_mut("disk.temperature").unwrap().segment(
                "disk.temperature",
                2,
                1005,
                "same-object"
            ),
            disk
        );
        assert_eq!(
            sources.get_mut("cpu.temperature.tctl").unwrap().segment(
                "cpu.temperature.tctl",
                2,
                1002,
                "same-object"
            ),
            cpu
        );
    }
}
