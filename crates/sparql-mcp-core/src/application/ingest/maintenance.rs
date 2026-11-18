//! Keeping the job ledger healthy: heartbeat, recovery after a restart, expiry
//! of abandoned jobs, and purging of old finished ones.
//!
//! *   **Idle time only counts while a core is running.** The core is spawned
//!     per agent session, so it is often off for days. A job that was idle for
//!     ten minutes when the core stopped must not be expired because the next
//!     session started a week later. The sweeper writes a *heartbeat* every
//!     sweep; on start, [`IngestService::recover`] reads it, takes the time
//!     since as downtime, and moves every active job's `smj:lastActivity`
//!     forward by that much.
//! *   **Expiry.** A job that is `open` or `interrupted` and idle for longer
//!     than `job_idle_ttl_secs` becomes `expired`, and its staging graph is
//!     dropped in the same transaction. A direct job's data stays in the target.
//! *   **Retention.** The ledger entries of a finished job (`committed`,
//!     `aborted`, `expired`, `failed`) are deleted `job_retention_secs` after it
//!     ended, so the jobs graph does not grow without bound.
//!
//! Every function takes the current time as a parameter, so the logic is tested
//! with any clock; [`IngestService::start_maintenance`] supplies the real one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use super::*;

/// The ledger subject that carries the heartbeat.
const SERVICE: &str = "urn:sparql-mcp:jobs:service";

/// What [`IngestService::recover`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Seconds between the last heartbeat and now (0 on a first start).
    pub downtime_secs: u64,
    /// Jobs whose `lastActivity` was moved forward by the downtime.
    pub shifted: Vec<String>,
    /// Jobs that were `running` when the core stopped and are now `interrupted`.
    pub interrupted: Vec<String>,
}

/// What [`IngestService::sweep`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub expired: Vec<String>,
    pub purged: Vec<String>,
}

/// One job as the sweeper sees it.
struct Row {
    id: String,
    state: Option<JobState>,
    last_activity: Option<u64>,
    ended: Option<u64>,
}

impl IngestService {
    fn job_rows(&self, store: &dyn SparqlStore) -> Result<Vec<Row>> {
        let q = format!(
            "SELECT ?j ?state ?t ?ended WHERE {{ GRAPH <{JOBS_GRAPH}> {{ ?j {} ?state . \
             OPTIONAL {{ ?j {} ?t }} OPTIONAL {{ ?j {} ?ended }} }} }}",
            smj("state"),
            smj("lastActivity"),
            smj("ended"),
        );
        Ok(rows(store, &q)?
            .iter()
            .filter_map(|r| {
                let id = value(r, "j")?.strip_prefix(JOB_PREFIX)?.to_string();
                // Batch records are `...:batch:N` subjects without a state.
                is_job_id(&id).then(|| Row {
                    id,
                    state: value(r, "state").and_then(JobState::from_iri),
                    last_activity: value(r, "t").and_then(parse_iso),
                    ended: value(r, "ended").and_then(parse_iso),
                })
            })
            .collect())
    }

    fn read_heartbeat(&self, store: &dyn SparqlStore) -> Result<Option<u64>> {
        let q = format!(
            "SELECT ?t WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{SERVICE}> {} ?t }} }}",
            smj("heartbeat")
        );
        Ok(rows(store, &q)?
            .first()
            .and_then(|r| value(r, "t"))
            .and_then(parse_iso))
    }

    /// Record that the core is alive at `now`.
    pub fn heartbeat(&self, now: u64) -> Result<()> {
        let hb = smj("heartbeat");
        let stamp = typed(&iso(now), "dateTime");
        self.privileged.update(&format!(
            "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ <{SERVICE}> {hb} ?t }} }} \
             INSERT {{ GRAPH <{JOBS_GRAPH}> {{ <{SERVICE}> {hb} {stamp} }} }} \
             WHERE {{ OPTIONAL {{ GRAPH <{JOBS_GRAPH}> {{ <{SERVICE}> {hb} ?t }} }} }}"
        ))
    }

    /// Set a job's state and `lastActivity` in one request that matches only
    /// its current quads.
    fn restamp_update(&self, id: &str, state: JobState, last_activity: u64) -> String {
        let job = job_iri(id);
        let (state_p, active_p) = (smj("state"), smj("lastActivity"));
        let stamp = typed(&iso(last_activity), "dateTime");
        format!(
            "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }} \
             INSERT {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} <{}> ; {active_p} {stamp} }} }} \
             WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }}",
            state.iri()
        )
    }

    /// Run when a core starts. Moves the idle clock of every active job
    /// forward by the downtime, turns jobs that were `running` into
    /// `interrupted` (their task died with the core), and writes a heartbeat.
    pub fn recover(&self, now: u64) -> Result<Recovery> {
        let store: &dyn SparqlStore = self.privileged.as_ref();
        let downtime = self
            .read_heartbeat(store)?
            .map_or(0, |beat| now.saturating_sub(beat));
        let mut report = Recovery {
            downtime_secs: downtime,
            ..Recovery::default()
        };
        for row in self.job_rows(store)? {
            let (Some(state), Some(last)) = (row.state, row.last_activity) else {
                continue;
            };
            if !state.is_active() {
                continue;
            }
            // Never later than now, whatever the clocks did.
            let shifted = last.saturating_add(downtime).min(now);
            let new_state = if state == JobState::Running {
                JobState::Interrupted
            } else {
                state
            };
            if new_state == state && shifted == last {
                continue;
            }
            self.privileged
                .exclusive("ingest_recover", |s| -> Result<()> {
                    // Re-read under the gate: only touch a job still in the
                    // state we saw.
                    if self
                        .manifest(s.as_ref(), &row.id)?
                        .and_then(|m| m.get("state").and_then(|x| JobState::from_iri(x)))
                        != Some(state)
                    {
                        return Ok(());
                    }
                    s.update(&self.restamp_update(&row.id, new_state, shifted))
                })??;
            if state == JobState::Running {
                report.interrupted.push(row.id.clone());
            }
            if shifted != last {
                report.shifted.push(row.id);
            }
        }
        self.heartbeat(now)?;
        Ok(report)
    }

    /// Expire idle jobs, purge old finished ones, and write a heartbeat.
    pub fn sweep(&self, now: u64) -> Result<SweepReport> {
        let store: &dyn SparqlStore = self.privileged.as_ref();
        let mut report = SweepReport::default();
        for row in self.job_rows(store)? {
            let Some(state) = row.state else { continue };
            match state {
                JobState::Open | JobState::Interrupted => {
                    let idle = row.last_activity.map(|t| now.saturating_sub(t));
                    if idle.is_some_and(|i| i > self.cfg.job_idle_ttl_secs)
                        && self.expire(&row.id, now)?
                    {
                        report.expired.push(row.id);
                    }
                }
                JobState::Committed | JobState::Aborted | JobState::Expired | JobState::Failed => {
                    let age = row.ended.map(|t| now.saturating_sub(t));
                    if age.is_some_and(|a| a > self.cfg.job_retention_secs) {
                        self.purge(&row.id)?;
                        report.purged.push(row.id);
                    }
                }
                JobState::Running => {}
            }
        }
        self.heartbeat(now)?;
        Ok(report)
    }

    /// Expire one job if it is still idle and unfinished once the write gate
    /// is held (a batch may have arrived since the sweep listed it).
    fn expire(&self, id: &str, now: u64) -> Result<bool> {
        self.privileged
            .exclusive("ingest_expire", |s| -> Result<bool> {
                let Some(m) = self.manifest(s.as_ref(), id)? else {
                    return Ok(false);
                };
                let state = m.get("state").and_then(|x| JobState::from_iri(x));
                let idle = m
                    .get("lastActivity")
                    .and_then(|t| parse_iso(t))
                    .map(|t| now.saturating_sub(t));
                if !matches!(state, Some(JobState::Open | JobState::Interrupted))
                    || !idle.is_some_and(|i| i > self.cfg.job_idle_ttl_secs)
                {
                    return Ok(false);
                }
                let mut update = self.transition_update(id, JobState::Expired);
                if let Some(staging) = m.get("staging") {
                    update.push_str(&format!(" ; DROP SILENT GRAPH <{staging}>"));
                }
                s.update(&update)?;
                Ok(true)
            })?
    }

    /// Delete a finished job's manifest and batch records.
    fn purge(&self, id: &str) -> Result<()> {
        let job = job_iri(id);
        self.privileged
            .exclusive("ingest_purge", |s| -> Result<()> {
                s.update(&format!(
                    "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ ?b ?p ?o }} }} \
                 WHERE {{ GRAPH <{JOBS_GRAPH}> {{ ?b {} <{job}> . ?b ?p ?o }} }} ; \
                 DELETE WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> ?p ?o }} }}",
                    smj("job")
                ))
            })?
    }

    /// Mark an open job as active now. A retry that finds its batch already
    /// stored does not write anything else, but it does show the agent is
    /// still there. Best effort: a failure only means the idle clock was not
    /// refreshed.
    pub(super) fn touch(&self, id: &str) {
        let job = job_iri(id);
        let active_p = smj("lastActivity");
        let stamp = typed(&iso_now(), "dateTime");
        let _ = self.privileged.update(&format!(
            "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {active_p} ?t }} }} \
             INSERT {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {active_p} {stamp} }} }} \
             WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {} <{}> ; {active_p} ?t }} }}",
            smj("state"),
            JobState::Open.iri()
        ));
    }

    /// Start the background maintenance of this service: recover once, then
    /// sweep every `sweep_interval_secs`. Dropping the handle stops it.
    pub fn start_maintenance(self: &Arc<Self>) -> Maintenance {
        let stop = Arc::new(AtomicBool::new(false));
        let (svc, flag) = (self.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("sparql-mcp-ingest-maintenance".into())
            .spawn(move || {
                match svc.recover(now_secs()) {
                    Ok(r) if r.shifted.is_empty() && r.interrupted.is_empty() => {}
                    Ok(r) => tracing::info!(
                        downtime_secs = r.downtime_secs,
                        shifted = r.shifted.len(),
                        interrupted = r.interrupted.len(),
                        "ingestion jobs recovered after a restart"
                    ),
                    Err(e) => tracing::warn!(error = %e, "ingestion recovery failed"),
                }
                let interval = Duration::from_secs(svc.cfg.sweep_interval_secs.max(1));
                let mut waited = Duration::ZERO;
                while !flag.load(Ordering::Relaxed) {
                    // Sleep in short slices so that dropping the handle is prompt.
                    std::thread::sleep(Duration::from_millis(100));
                    waited += Duration::from_millis(100);
                    if waited < interval {
                        continue;
                    }
                    waited = Duration::ZERO;
                    match svc.sweep(now_secs()) {
                        Ok(r) if r.expired.is_empty() && r.purged.is_empty() => {}
                        Ok(r) => tracing::info!(
                            expired = r.expired.len(),
                            purged = r.purged.len(),
                            "ingestion jobs swept"
                        ),
                        Err(e) => tracing::warn!(error = %e, "ingestion sweep failed"),
                    }
                }
            })
            .ok();
        Maintenance { stop, thread }
    }
}

/// The running maintenance thread. Dropping it stops the thread.
pub struct Maintenance {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Maintenance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
