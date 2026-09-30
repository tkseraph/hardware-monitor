//! Bounded, atomic process-disk snapshots. A partial window never carries usable rates.
use super::disk_io::{Quality, Window};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub state: State,
    pub reason: Reason,
    pub verified_subset_only: bool,
    pub window_ms: Option<u32>,
    pub age_ms: Option<u64>,
    pub known_processes: usize,
    pub quality: Quality,
    pub identities: IdentityQuality,
}
impl View {
    pub fn state(state: State, reason: Reason) -> Self {
        Self {
            state,
            reason,
            verified_subset_only: true,
            window_ms: None,
            age_ms: None,
            known_processes: 0,
            quality: Default::default(),
            identities: Default::default(),
        }
    }
}

pub const ROWS_PER_CHUNK: usize = 128;
pub const MAX_PROCESSES: usize = 4096;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Disabled,
    Starting,
    WarmingUp,
    Ready,
    Incomplete,
    Stale,
    Stopping,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    None,
    PermissionRequired,
    Clock,
    StartFailed,
    OpenTrace,
    Baseline,
    Loss,
    Pipeline,
    SourceEnded,
    CleanupUnconfirmed,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityQuality {
    pub resolved: u64,
    pub unavailable: u64,
    pub open_failed: u64,
    pub lifetime_unverified: u64,
    pub clock_unverified: u64,
    pub capacity_rejected: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub session: String,
    pub sequence: u64,
    pub state: State,
    pub reason: Reason,
    pub verified_subset_only: bool,
    pub start_ns: u64,
    pub end_ns: u64,
    pub quality: Quality,
    /// Cumulative source diagnostics, not percentages of system-wide coverage.
    pub identities: IdentityQuality,
    pub total: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub pid: u32,
    /// Decimal FILETIME; never pass this through JavaScript's number type.
    pub creation: String,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub header: Header,
    pub rows: Vec<Row>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub header: Header,
    pub offset: usize,
    pub rows: Vec<Row>,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid process disk snapshot")
}
impl Snapshot {
    pub fn empty(session: &str, state: State, reason: Reason) -> Self {
        Self {
            header: Header {
                session: session.into(),
                sequence: 0,
                state,
                reason,
                verified_subset_only: true,
                start_ns: 0,
                end_ns: 0,
                quality: Quality::default(),
                identities: IdentityQuality::default(),
                total: 0,
            },
            rows: vec![],
        }
    }
    pub fn window(session: &str, window: &Window, identities: IdentityQuality) -> Self {
        Self {
            header: Header {
                session: session.into(),
                sequence: 0,
                state: if window.ready {
                    State::Ready
                } else {
                    State::Incomplete
                },
                reason: Reason::None,
                verified_subset_only: true,
                start_ns: window.start_ns,
                end_ns: window.end_ns,
                quality: window.quality,
                identities,
                total: window.rows.len(),
            },
            rows: window
                .rows
                .iter()
                .map(|r| Row {
                    pid: r.process.pid,
                    creation: r.process.creation.to_string(),
                    read_bps: window.ready.then_some(r.read_bps).flatten(),
                    write_bps: window.ready.then_some(r.write_bps).flatten(),
                })
                .collect(),
        }
    }
    pub fn chunks(&self) -> Vec<Chunk> {
        if self.rows.is_empty() {
            return vec![Chunk {
                header: self.header.clone(),
                offset: 0,
                rows: vec![],
            }];
        }
        self.rows
            .chunks(ROWS_PER_CHUNK)
            .enumerate()
            .map(|(i, rows)| Chunk {
                header: self.header.clone(),
                offset: i * ROWS_PER_CHUNK,
                rows: rows.to_vec(),
            })
            .collect()
    }
    pub fn validate(&self) -> io::Result<()> {
        let h = &self.header;
        let sampled = matches!(h.state, State::Ready | State::Incomplete);
        if uuid::Uuid::parse_str(&h.session).is_err()
            || h.sequence == 0
            || !h.verified_subset_only
            || h.total > MAX_PROCESSES
            || self.rows.len() != h.total
            || (sampled
                && (!(100_000_000..=15_000_000_000).contains(&h.end_ns.saturating_sub(h.start_ns))
                    || h.end_ns <= h.start_ns
                    || h.reason != Reason::None))
            || (!sampled && (h.total != 0 || h.start_ns != 0 || h.end_ns != 0))
            || (h.state == State::Ready && !h.quality.complete())
            || h.identities
                .open_failed
                .checked_add(h.identities.lifetime_unverified)
                .and_then(|n| n.checked_add(h.identities.clock_unverified))
                .and_then(|n| n.checked_add(h.identities.capacity_rejected))
                != Some(h.identities.unavailable)
        {
            return Err(invalid());
        }
        let mut keys = BTreeSet::new();
        for row in &self.rows {
            if row.pid == 0
                || row.creation.len() > 20
                || !row
                    .creation
                    .parse::<u64>()
                    .is_ok_and(|v| v > 0 && v.to_string() == row.creation)
                || !keys.insert((row.pid, &row.creation))
                || row.read_bps.is_some() != row.write_bps.is_some()
                || [row.read_bps, row.write_bps]
                    .iter()
                    .flatten()
                    .any(|v| !v.is_finite() || *v < 0.0 || *v > 2e20)
                || (h.state != State::Ready && (row.read_bps.is_some() || row.write_bps.is_some()))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
/// No row becomes visible until all chunks have arrived and passed validation.
#[derive(Default)]
pub struct Assembly {
    header: Option<Header>,
    rows: Vec<Row>,
}
impl Assembly {
    pub fn push(&mut self, chunk: Chunk) -> io::Result<Option<Snapshot>> {
        if chunk.header.total > MAX_PROCESSES
            || chunk.rows.len() > ROWS_PER_CHUNK
            || chunk.offset != self.rows.len()
            || self.header.as_ref().is_some_and(|h| *h != chunk.header)
            || chunk.offset.saturating_add(chunk.rows.len()) > chunk.header.total
            || (chunk.rows.is_empty() && chunk.header.total != 0)
        {
            return Err(invalid());
        }
        self.header = Some(chunk.header);
        self.rows.extend(chunk.rows);
        if self.rows.len() != self.header.as_ref().unwrap().total {
            return Ok(None);
        }
        let snapshot = Snapshot {
            header: self.header.take().unwrap(),
            rows: std::mem::take(&mut self.rows),
        };
        snapshot.validate()?;
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_are_atomic_and_reject_mixed_duplicate_or_partial_rates() {
        let mut s = Snapshot::empty(
            "01234567-89ab-cdef-0123-456789abcdef",
            State::Ready,
            Reason::None,
        );
        s.header.sequence = 1;
        s.header.end_ns = 1_000_000_000;
        s.rows = (1..=257)
            .map(|pid| Row {
                pid,
                creation: "99".into(),
                read_bps: Some(0.0),
                write_bps: Some(1.0),
            })
            .collect();
        s.header.total = s.rows.len();
        let chunks = s.chunks();
        let mut a = Assembly::default();
        assert!(a.push(chunks[0].clone()).unwrap().is_none());
        let mut mixed = chunks[1].clone();
        mixed.header.sequence = 2;
        assert!(a.push(mixed).is_err());
        let mut a = Assembly::default();
        for c in chunks.iter().take(2) {
            assert!(a.push(c.clone()).unwrap().is_none());
        }
        assert_eq!(a.push(chunks[2].clone()).unwrap(), Some(s.clone()));
        s.rows[1] = s.rows[0].clone();
        assert!(s.validate().is_err());
        s.rows.remove(1);
        s.header.total = s.rows.len();
        s.header.state = State::Incomplete;
        assert!(s.validate().is_err());
        for r in &mut s.rows {
            r.read_bps = None;
            r.write_bps = None;
        }
        assert!(s.validate().is_ok());
        s.header.verified_subset_only = false;
        assert!(s.validate().is_err());
    }
}
