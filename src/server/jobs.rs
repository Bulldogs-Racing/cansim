//! Renode job table for `canlab serve` (PROMPT.md Phase 4, WS part).
//!
//! Firmware runs take tens of seconds of wall clock, so they cannot run
//! inside a request handler (one request gets one prompt reply, and the
//! session `Mutex` must stay unblocked). Instead `StartRenodeRun` records
//! a [`JobRecord`] and spawns a background thread; clients poll
//! `GetRenodeJob` / `ListRenodeJobs`, then `ImportRenodeTrace` replays a
//! finished job's observed TX frames through the session engine via
//! [`Session::import_frames`][crate::server::session::Session::import_frames].
//!
//! Jobs are independent of the loaded session: the session may hold a
//! virtual project under edit while firmware runs against a renode
//! project file. Import requires the session to contain the observed
//! senders, else it fails with the missing node names.
//!
//! Memory is bounded (§67 spirit): at most `max_jobs` (server flag
//! `--max-jobs`, default 1) run concurrently, and only the newest
//! [`MAX_KEPT_JOBS`] finished records are kept — older ones are evicted
//! and later polls/imports report [`JobError::UnknownJob`].

use crate::backends::api::{BackendError, BackendOutcome, ObservedUart};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// Finished records kept per server lifetime (running jobs are never evicted).
pub const MAX_KEPT_JOBS: usize = 32;

/// Max UART lines one log reply carries (chatty-firmware bound).
pub const MAX_LOG_LINES: usize = 200;

/// Actionable job-table errors (§68).
#[derive(Debug, Error)]
pub enum JobError {
    #[error("job table full: {running} Renode job(s) already running (max {max} via --max-jobs) — wait for one to finish, or restart the server with a higher limit")]
    TableFull { running: usize, max: u16 },
    #[error("unknown Renode job {id} (only the newest {kept} finished jobs are kept; older ones are evicted)")]
    UnknownJob { id: u64, kept: usize },
    #[error("Renode job {id} already {state} — only running jobs can be cancelled")]
    AlreadyFinished { id: u64, state: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn name(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    fn finished(self) -> bool {
        self != JobState::Running
    }
}

/// One supervised firmware run. `outcome` stays server-side (the raw
/// emulator log can be megabytes); the wire [`JobInfo`] carries counts.
#[derive(Debug)]
pub struct JobRecord {
    pub id: u64,
    pub project: String,
    pub run_secs: u64,
    pub state: JobState,
    pub transmitted: u64,
    pub received: u64,
    pub error: Option<String>,
    pub outcome: Option<BackendOutcome>,
    /// Tripped by [`JobTable::cancel`]; the emulator thread polls it every
    /// 100 ms and shuts down gracefully on the next tick.
    pub cancel: Arc<AtomicBool>,
}

/// Background firmware runs. Not thread-safe itself — the server wraps it
/// in the same `Mutex` as the session.
#[derive(Debug)]
pub struct JobTable {
    max_jobs: u16,
    next_id: u64,
    jobs: BTreeMap<u64, JobRecord>,
}

impl JobTable {
    pub fn new(max_jobs: u16) -> Self {
        JobTable {
            max_jobs,
            next_id: 1,
            jobs: BTreeMap::new(),
        }
    }

    pub fn max_jobs(&self) -> u16 {
        self.max_jobs
    }

    /// Currently executing jobs (the `--max-jobs` budget).
    pub fn running_count(&self) -> usize {
        self.jobs
            .values()
            .filter(|j| j.state == JobState::Running)
            .count()
    }

    /// Reserve an id or refuse when the running budget is exhausted. The
    /// caller spawns the emulator thread after this returns.
    pub fn try_start(&mut self, project: String, run_secs: u64) -> Result<u64, JobError> {
        let running = self.running_count();
        if running >= self.max_jobs as usize {
            return Err(JobError::TableFull {
                running,
                max: self.max_jobs,
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.insert(
            id,
            JobRecord {
                id,
                project,
                run_secs,
                state: JobState::Running,
                transmitted: 0,
                received: 0,
                error: None,
                outcome: None,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        Ok(id)
    }

    /// Request cancellation of a running job (async: the emulator thread
    /// shuts down gracefully on its next 100 ms tick, then the record
    /// becomes `cancelled`). Finished jobs fail loudly instead of
    /// pretending to cancel.
    pub fn cancel(&self, id: u64) -> Result<(), JobError> {
        let rec = self.get(id)?;
        if rec.state != JobState::Running {
            return Err(JobError::AlreadyFinished {
                id,
                state: rec.state.name(),
            });
        }
        rec.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Store a finished run's observations (or its failure). A
    /// [`BackendError::Cancelled`] run becomes `cancelled`, never `done`
    /// with partial counts. Evicts the oldest finished records beyond
    /// [`MAX_KEPT_JOBS`].
    pub fn finish(&mut self, id: u64, result: Result<BackendOutcome, BackendError>) {
        let Some(job) = self.jobs.get_mut(&id) else {
            return;
        };
        match result {
            Ok(outcome) => {
                // A cancel that lands after natural completion loses the
                // race honestly: the run really did finish.
                if job.cancel.load(Ordering::Relaxed) {
                    job.state = JobState::Cancelled;
                } else {
                    job.transmitted = outcome.tx_frames.len() as u64;
                    job.received = outcome.rx_frames.len() as u64;
                    job.state = JobState::Done;
                    job.outcome = Some(outcome);
                }
            }
            Err(BackendError::Cancelled) => {
                job.state = JobState::Cancelled;
            }
            Err(e) => {
                job.state = JobState::Failed;
                job.error = Some(e.to_string());
            }
        }
        let mut finished: Vec<u64> = self
            .jobs
            .values()
            .filter(|j| j.state.finished())
            .map(|j| j.id)
            .collect();
        finished.sort_unstable();
        while finished.len() > MAX_KEPT_JOBS {
            let evict = finished.remove(0);
            self.jobs.remove(&evict);
        }
    }

    pub fn get(&self, id: u64) -> Result<&JobRecord, JobError> {
        self.jobs.get(&id).ok_or(JobError::UnknownJob {
            id,
            kept: MAX_KEPT_JOBS,
        })
    }

    /// Last `n` UART lines of a finished job (running jobs report what
    /// they have so far; cancelled jobs have none — partials discarded).
    /// Capped server-side so a chatty firmware can never blow up a reply.
    pub fn log_tail(&self, id: u64, n: usize) -> Result<(u64, Vec<ObservedUart>), JobError> {
        let rec = self.get(id)?;
        let all: &[ObservedUart] = rec
            .outcome
            .as_ref()
            .map(|o| o.uart.as_slice())
            .unwrap_or(&[]);
        let take = n.clamp(1, MAX_LOG_LINES);
        let skip = all.len().saturating_sub(take);
        Ok((all.len() as u64, all[skip..].to_vec()))
    }

    /// All records, oldest first.
    pub fn list(&self) -> Vec<&JobRecord> {
        self.jobs.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::api::{FrameDir, ObservedFrame};
    use crate::can::frame::CanFrame;
    use crate::can::id::CanId;

    fn outcome_with(tx: usize, rx: usize) -> BackendOutcome {
        let frame = CanFrame::new(CanId::new_standard(0x123).unwrap(), &[1]).unwrap();
        let obs = |dir| ObservedFrame {
            node: "n".into(),
            dir,
            frame: frame.clone(),
        };
        BackendOutcome {
            uart: Vec::new(),
            tx_frames: (0..tx).map(|_| obs(FrameDir::Tx)).collect(),
            rx_frames: (0..rx).map(|_| obs(FrameDir::Rx)).collect(),
            raw_log: String::new(),
        }
    }

    #[test]
    fn max_jobs_bounds_concurrent_runs() {
        let mut t = JobTable::new(1);
        let a = t.try_start("a.canlab".into(), 30).unwrap();
        assert_eq!(a, 1);
        assert_eq!(t.running_count(), 1);
        let err = t.try_start("b.canlab".into(), 30).unwrap_err();
        assert!(matches!(err, JobError::TableFull { .. }));
        assert!(err.to_string().contains("--max-jobs"));
        // A finished job frees its slot.
        t.finish(a, Ok(outcome_with(2, 1)));
        assert_eq!(t.running_count(), 0);
        let b = t.try_start("b.canlab".into(), 30).unwrap();
        assert_eq!(b, 2);
        let rec = t.get(b).unwrap();
        assert_eq!(rec.state, JobState::Running);
    }

    #[test]
    fn finish_records_counts_and_failures() {
        let mut t = JobTable::new(4);
        let a = t.try_start("a.canlab".into(), 30).unwrap();
        t.finish(a, Ok(outcome_with(3, 2)));
        let rec = t.get(a).unwrap();
        assert_eq!(rec.state, JobState::Done);
        assert_eq!((rec.transmitted, rec.received), (3, 2));
        assert!(rec.outcome.is_some());
        let b = t.try_start("b.canlab".into(), 30).unwrap();
        t.finish(b, Err(BackendError::Io("boom".into())));
        let rec = t.get(b).unwrap();
        assert_eq!(rec.state, JobState::Failed);
        assert_eq!(
            rec.error.as_deref(),
            Some("I/O error during backend run: boom")
        );
    }

    #[test]
    fn cancel_trips_the_flag_and_finish_honors_it() {
        let mut t = JobTable::new(4);
        let a = t.try_start("a.canlab".into(), 30).unwrap();
        t.cancel(a).unwrap();
        assert!(t.get(a).unwrap().cancel.load(Ordering::Relaxed));
        // Cancellation before completion: no Done with partial counts.
        t.finish(a, Ok(outcome_with(3, 2)));
        let rec = t.get(a).unwrap();
        assert_eq!(rec.state, JobState::Cancelled);
        assert_eq!((rec.transmitted, rec.received), (0, 0));
        assert!(rec.outcome.is_none());
        // Cancelling a finished job fails loudly.
        let err = t.cancel(a).unwrap_err();
        assert!(matches!(err, JobError::AlreadyFinished { .. }));
        assert!(err.to_string().contains("cancelled"));
        assert!(matches!(t.cancel(999), Err(JobError::UnknownJob { .. })));
        // The backend's own Cancelled error maps the same way.
        let b = t.try_start("b.canlab".into(), 30).unwrap();
        t.finish(b, Err(BackendError::Cancelled));
        assert_eq!(t.get(b).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn unknown_and_evicted_jobs_name_themselves() {
        let mut t = JobTable::new(100);
        assert!(matches!(t.get(7), Err(JobError::UnknownJob { .. })));
        // Fill past the keep limit: oldest finished records go.
        for _ in 0..(MAX_KEPT_JOBS + 5) {
            let id = t.try_start("p.canlab".into(), 1).unwrap();
            t.finish(id, Ok(outcome_with(0, 0)));
        }
        assert_eq!(t.list().len(), MAX_KEPT_JOBS);
        assert!(matches!(t.get(1), Err(JobError::UnknownJob { .. })));
        let err = t.get(1).unwrap_err();
        assert!(err.to_string().contains("evicted"));
    }

    #[test]
    fn log_tail_caps_and_reports_totals() {
        use crate::backends::api::ObservedUart;
        let mut t = JobTable::new(4);
        let a = t.try_start("a.canlab".into(), 30).unwrap();
        // Running job, nothing observed yet: empty tail, total 0.
        assert_eq!(t.log_tail(a, 10).unwrap(), (0, vec![]));
        let mut out = outcome_with(0, 0);
        out.uart = (0..5)
            .map(|i| ObservedUart {
                machine: "m".into(),
                message: format!("line{i}"),
            })
            .collect();
        t.finish(a, Ok(out));
        let (total, tail) = t.log_tail(a, 2).unwrap();
        assert_eq!(total, 5);
        assert_eq!(
            tail.iter().map(|u| u.message.as_str()).collect::<Vec<_>>(),
            vec!["line3", "line4"]
        );
        // Oversized requests clamp, they don't error.
        assert_eq!(t.log_tail(a, 10_000).unwrap().1.len(), 5);
        assert!(t.log_tail(999, 5).is_err());
    }
}
