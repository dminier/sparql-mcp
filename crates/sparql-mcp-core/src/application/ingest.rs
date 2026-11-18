//! Ingestion jobs (`docs/designs/sparql-mcp-batch-ingestion.md`).
//!
//! A job groups the batches an agent sends for one logical load, so a retry is
//! safe, the load can be validated before it is visible, and it survives a
//! restart. This module holds the **job ledger** (manifest and batch records
//! in the protected graph [`JOBS_GRAPH`]), the state machine, and the
//! operations that need no batch data: `begin`, `status`, `abort`.
//!
//! Every ledger change goes through [`PrivilegedStore::exclusive`], which
//! holds the write gate once: a read-then-write on a job (check the state,
//! then change it) cannot interleave with another writer. A state change and
//! its side effects are one SPARQL update request, which Oxigraph runs as one
//! transaction, so a crash leaves the job in the old state or the new one,
//! never in between.
//!
//! All text that comes from outside is put in SPARQL through
//! [`string_literal`] and [`iri`].

mod maintenance;
pub use maintenance::{Maintenance, Recovery, SweepReport};

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::application::clock::{iso, iso_now, now_secs, parse_iso};
use crate::application::event_sink::{emit_quads, EventLimits, EventSink, QuadChange, StoreEvent};
use crate::application::gated_store::PrivilegedStore;
use crate::application::guards::is_protected;
use crate::application::limits::ToolError;
use crate::application::sparql_text::{iri, string_literal};
use crate::config::IngestConfig;
use crate::domain::{
    BatchRefusal, BatchWrite, Counter, Idempotency, InputFormat, QueryResult, RdfTerm,
    RecordCounts, Replace, SparqlStore, StoredQuad,
};

/// The protected graph holding every job's manifest and batch records.
pub const JOBS_GRAPH: &str = "urn:sparql-mcp:jobs";
/// The `smj:` vocabulary namespace.
pub const SMJ: &str = "urn:sparql-mcp:jobs#";
const JOB_PREFIX: &str = "urn:sparql-mcp:job:";
const STAGING_PREFIX: &str = "urn:sparql-mcp:staging:";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
/// The predicate `project_create` uses to link a project to its graph.
const SMC_NAMED_GRAPH: &str = "https://sparql-mcp.dev/ns#namedGraph";
const META_GRAPH: &str = "urn:meta";
const MAX_LABEL_CHARS: usize = 200;
/// The most IRIs `ingest_commit` lists in `new_subjects`.
pub const MAX_NEW_SUBJECTS: usize = 5_000;
/// Once a direct job is above the event threshold, at most one `Changed`
/// event this often.
const CHANGED_EVERY: Duration = Duration::from_secs(5);
/// Anonymous blank nodes and skolem IRIs live in this reserved namespace.
const GENID: &str = "urn:sparql-mcp:genid:";
const RESERVED_PREFIX: &str = "urn:sparql-mcp:";

/// Where a job is in its life. The ledger stores it as an `smj:` IRI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Open,
    Running,
    Interrupted,
    Failed,
    Committed,
    Aborted,
    Expired,
}

impl JobState {
    const ALL: [JobState; 7] = [
        Self::Open,
        Self::Running,
        Self::Interrupted,
        Self::Failed,
        Self::Committed,
        Self::Aborted,
        Self::Expired,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Running => "running",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Committed => "committed",
            Self::Aborted => "aborted",
            Self::Expired => "expired",
        }
    }

    /// The state's IRI in the ledger.
    pub fn iri(self) -> String {
        format!("{SMJ}{}", self.name())
    }

    pub fn from_iri(s: &str) -> Option<Self> {
        let name = s.strip_prefix(SMJ)?;
        Self::ALL.into_iter().find(|st| st.name() == name)
    }

    /// Counts toward `max_open_jobs`.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Open | Self::Running | Self::Interrupted)
    }

    /// `ingest_abort` is allowed from these states.
    pub fn can_abort(self) -> bool {
        matches!(self, Self::Open | Self::Interrupted | Self::Failed)
    }

    /// `ingest_batch` is only accepted by an open job.
    pub fn accepts_batches(self) -> bool {
        self == Self::Open
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Batches land in a staging graph; the commit moves them in one step.
    Staged,
    /// Each batch lands in the target graph at once.
    Direct,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Direct => "direct",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "staged" => Some(Self::Staged),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }
}

/// What `ingest_begin` asks for, already type-checked by the tool layer.
#[derive(Debug, Clone)]
pub struct BeginRequest {
    pub target_graph: Option<String>,
    pub mode: Mode,
    pub replace: bool,
    /// `ntriples` or `turtle`.
    pub format: String,
    pub expected_batches: Option<u32>,
    pub label: Option<String>,
    /// `skolemize` or `reject`.
    pub blank_nodes: String,
}

pub fn job_iri(id: &str) -> String {
    format!("{JOB_PREFIX}{id}")
}

pub fn staging_iri(id: &str) -> String {
    format!("{STAGING_PREFIX}{id}")
}

/// A job id is a ULID: 26 characters of Crockford base 32. Anything else is
/// not a job, and is never put in a query.
pub fn is_job_id(id: &str) -> bool {
    id.len() == 26 && id.parse::<Ulid>().is_ok()
}

/// `[1,2,3,5]` as `"1-3,5"`.
pub fn seq_ranges(seqs: &[u32]) -> String {
    let sorted: BTreeSet<u32> = seqs.iter().copied().collect();
    let mut out: Vec<String> = Vec::new();
    let mut iter = sorted.into_iter().peekable();
    while let Some(start) = iter.next() {
        let mut end = start;
        while iter.peek() == Some(&(end + 1)) {
            end = iter.next().unwrap_or(end);
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    out.join(",")
}

fn coded(code: &'static str, message: impl Into<String>, job: Option<&str>) -> anyhow::Error {
    ToolError::Coded {
        code,
        message: message.into(),
        job_id: job.map(str::to_string),
    }
    .into_anyhow()
}

fn rows(store: &dyn SparqlStore, query: &str) -> Result<Vec<HashMap<String, RdfTerm>>> {
    Ok(match store.query(query)? {
        QueryResult::Solutions(s) => s.rows,
        _ => Vec::new(),
    })
}

fn value<'a>(row: &'a HashMap<String, RdfTerm>, var: &str) -> Option<&'a str> {
    row.get(var).map(RdfTerm::as_value_str)
}

/// A typed literal: `"text"^^<datatype>`.
fn typed(text: &str, datatype: &str) -> String {
    format!("{}^^<{XSD}{datatype}>", string_literal(text))
}

fn smj(local: &str) -> String {
    format!("<{SMJ}{local}>")
}

/// The ingestion operations that need no batch data.
pub struct IngestService {
    privileged: Arc<PrivilegedStore>,
    active_graph: Arc<RwLock<String>>,
    cfg: IngestConfig,
    events: Arc<dyn EventSink>,
    /// When a `Changed` event was last sent for a direct job, to send at most
    /// one every [`CHANGED_EVERY`].
    last_changed: Mutex<HashMap<String, Instant>>,
}

impl IngestService {
    pub fn new(
        privileged: Arc<PrivilegedStore>,
        active_graph: Arc<RwLock<String>>,
        cfg: IngestConfig,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            privileged,
            active_graph,
            cfg,
            events,
            last_changed: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &IngestConfig {
        &self.cfg
    }

    /// The limits an agent should size its batches by, as sent in the reply of
    /// `ingest_begin`.
    pub fn limits_json(&self) -> Value {
        let c = &self.cfg;
        json!({
            "max_batch_bytes": c.max_batch_bytes,
            "max_batch_triples": c.max_batch_triples,
            "max_job_batches": c.max_job_batches,
            "max_job_triples_staged": c.max_job_triples_staged,
            "max_job_triples_direct": c.max_job_triples_direct,
            "max_replace_triples": c.max_replace_triples,
            "max_open_jobs": c.max_open_jobs,
            "job_idle_ttl_secs": c.job_idle_ttl_secs,
            "validate_timeout_secs": c.validate_timeout_secs,
        })
    }

    fn active_graph(&self) -> String {
        self.active_graph
            .read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Whether `graph` may be the target of a job: the active graph, or a
    /// project registered in `urn:meta`. Internal graphs never are.
    fn target_is_valid(&self, store: &dyn SparqlStore, graph: &str) -> Result<bool> {
        if is_protected(graph) {
            return Ok(false);
        }
        if graph == self.active_graph() {
            return Ok(true);
        }
        let Some(graph_iri) = iri(graph) else {
            return Ok(false);
        };
        let ask =
            format!("ASK {{ GRAPH <{META_GRAPH}> {{ ?p <{SMC_NAMED_GRAPH}> {graph_iri} }} }}");
        Ok(matches!(store.query(&ask)?, QueryResult::Boolean(true)))
    }

    fn count_active(&self, store: &dyn SparqlStore) -> Result<usize> {
        let states = [JobState::Open, JobState::Running, JobState::Interrupted]
            .map(|s| format!("<{}>", s.iri()))
            .join(", ");
        let q = format!(
            "SELECT (COUNT(DISTINCT ?j) AS ?n) WHERE {{ GRAPH <{JOBS_GRAPH}> {{ \
             ?j {} ?s . FILTER(?s IN ({states})) }} }}",
            smj("state")
        );
        Ok(rows(store, &q)?
            .first()
            .and_then(|r| value(r, "n"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0))
    }

    /// Create a job. Returns its id, mode, target and the effective limits.
    pub fn begin(&self, req: &BeginRequest) -> Result<Value> {
        // Argument checks that need no store access.
        if !matches!(req.format.as_str(), "ntriples" | "turtle") {
            return Err(coded(
                "invalid_argument",
                format!("format must be ntriples or turtle, not {:?}", req.format),
                None,
            ));
        }
        if !matches!(req.blank_nodes.as_str(), "skolemize" | "reject") {
            return Err(coded(
                "invalid_argument",
                "blank_nodes must be skolemize or reject",
                None,
            ));
        }
        if req.replace && req.mode != Mode::Staged {
            return Err(coded(
                "invalid_argument",
                "replace needs mode staged: a direct job writes the target as it goes",
                None,
            ));
        }
        if let Some(n) = req.expected_batches {
            if n == 0 || n > self.cfg.max_job_batches {
                return Err(coded(
                    "invalid_argument",
                    format!(
                        "expected_batches must be between 1 and {}",
                        self.cfg.max_job_batches
                    ),
                    None,
                ));
            }
        }
        if let Some(label) = &req.label {
            if label.chars().count() > MAX_LABEL_CHARS {
                return Err(coded(
                    "invalid_argument",
                    format!("label is longer than {MAX_LABEL_CHARS} characters"),
                    None,
                ));
            }
        }
        let target = req
            .target_graph
            .clone()
            .unwrap_or_else(|| self.active_graph());

        self.privileged
            .exclusive("ingest_begin", |store| -> Result<Value> {
                if !self.target_is_valid(store.as_ref(), &target)? {
                    return Err(coded(
                        "invalid_target",
                        format!(
                            "{target} is not the active graph or a project created with \
                             project_create (internal graphs cannot be targeted)"
                        ),
                        None,
                    ));
                }
                // A replace rewrites the whole target in one transaction: refuse
                // it up front if the target and the job could not fit.
                let target_size = if req.replace {
                    let size = self.count_graph(store.as_ref(), &target)?;
                    if size.saturating_add(self.cfg.max_job_triples_staged)
                        > self.cfg.max_replace_triples
                    {
                        return Err(coded(
                            "replace_too_large",
                            format!(
                                "replacing {target} ({size} triples, plus up to {} staged) \
                                 would exceed max_replace_triples = {}",
                                self.cfg.max_job_triples_staged, self.cfg.max_replace_triples
                            ),
                            None,
                        ));
                    }
                    Some(size)
                } else {
                    None
                };
                let active = self.count_active(store.as_ref())?;
                if active >= self.cfg.max_open_jobs {
                    return Err(coded(
                        "too_many_open_jobs",
                        format!(
                            "{active} jobs are already open (max_open_jobs = {}); \
                             commit or abort one first",
                            self.cfg.max_open_jobs
                        ),
                        None,
                    ));
                }

                let id = Ulid::new().to_string();
                let job = job_iri(&id);
                let now = iso_now();
                let staging = (req.mode == Mode::Staged).then(|| staging_iri(&id));

                let mut triples = vec![
                    format!("<{job}> a {}", smj("Job")),
                    format!("<{job}> {} <{}>", smj("state"), JobState::Open.iri()),
                    format!("<{job}> {} <{target}>", smj("target")),
                    format!(
                        "<{job}> {} {}",
                        smj("mode"),
                        string_literal(req.mode.name())
                    ),
                    format!(
                        "<{job}> {} {}",
                        smj("replace"),
                        typed(&req.replace.to_string(), "boolean")
                    ),
                    format!("<{job}> {} {}", smj("format"), string_literal(&req.format)),
                    format!(
                        "<{job}> {} {}",
                        smj("blankNodes"),
                        string_literal(&req.blank_nodes)
                    ),
                    format!("<{job}> {} {}", smj("created"), typed(&now, "dateTime")),
                    format!(
                        "<{job}> {} {}",
                        smj("lastActivity"),
                        typed(&now, "dateTime")
                    ),
                    format!("<{job}> {} {}", smj("triplesTotal"), typed("0", "long")),
                    format!("<{job}> {} {}", smj("batchCount"), typed("0", "long")),
                ];
                if let Some(size) = target_size {
                    triples.push(format!(
                        "<{job}> {} {}",
                        smj("targetSizeAtBegin"),
                        typed(&size.to_string(), "long")
                    ));
                }
                if let Some(s) = &staging {
                    triples.push(format!("<{job}> {} <{s}>", smj("staging")));
                }
                if let Some(n) = req.expected_batches {
                    triples.push(format!(
                        "<{job}> {} {}",
                        smj("expectedBatches"),
                        typed(&n.to_string(), "long")
                    ));
                }
                if let Some(label) = &req.label {
                    triples.push(format!(
                        "<{job}> {} {}",
                        smj("label"),
                        string_literal(label)
                    ));
                }
                let update = format!(
                    "INSERT DATA {{ GRAPH <{JOBS_GRAPH}> {{ {} . }} }}",
                    triples.join(" . ")
                );
                store.update(&update)?;

                let mut reply = json!({
                    "job_id": id,
                    "state": JobState::Open.name(),
                    "mode": req.mode.name(),
                    "target_graph": target,
                    "replace": req.replace,
                    "format": req.format,
                    "blank_nodes": req.blank_nodes,
                    "limits": self.limits_json(),
                });
                if let Some(s) = staging {
                    reply["staging_graph"] = json!(s);
                }
                Ok(reply)
            })?
    }

    /// One job's status, or the list of jobs that are not finished.
    pub fn status(&self, job_id: Option<&str>) -> Result<Value> {
        let store: &dyn SparqlStore = self.privileged.as_ref();
        match job_id {
            Some(id) => self.job_status(store, id),
            None => {
                let q = format!(
                    "SELECT ?j ?state ?mode ?target ?label WHERE {{ GRAPH <{JOBS_GRAPH}> {{ \
                     ?j {} ?state ; {} ?mode ; {} ?target . OPTIONAL {{ ?j {} ?label }} }} }} \
                     ORDER BY ?j",
                    smj("state"),
                    smj("mode"),
                    smj("target"),
                    smj("label"),
                );
                let jobs: Vec<Value> = rows(store, &q)?
                    .iter()
                    .filter_map(|r| {
                        let state = JobState::from_iri(value(r, "state")?)?;
                        state.is_active().then(|| {
                            let id = value(r, "j")
                                .unwrap_or_default()
                                .trim_start_matches(JOB_PREFIX);
                            let mut job = json!({
                                "job_id": id,
                                "state": state.name(),
                                "mode": value(r, "mode"),
                                "target_graph": value(r, "target"),
                            });
                            if let Some(label) = value(r, "label") {
                                job["label"] = json!(label);
                            }
                            job
                        })
                    })
                    .collect();
                Ok(json!({ "jobs": jobs, "max_open_jobs": self.cfg.max_open_jobs }))
            }
        }
    }

    /// The manifest of a job as a map of predicate local name to value.
    fn manifest(
        &self,
        store: &dyn SparqlStore,
        id: &str,
    ) -> Result<Option<HashMap<String, String>>> {
        if !is_job_id(id) {
            return Ok(None);
        }
        let q = format!(
            "SELECT ?p ?o WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{}> ?p ?o }} }}",
            job_iri(id)
        );
        let rows = rows(store, &q)?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            rows.iter()
                .filter_map(|r| {
                    let p = value(r, "p")?.strip_prefix(SMJ)?;
                    Some((p.to_string(), value(r, "o")?.to_string()))
                })
                .collect(),
        ))
    }

    /// The accepted batches of a job: their sequence numbers and the triples
    /// parsed so far.
    fn batches(&self, store: &dyn SparqlStore, id: &str) -> Result<(Vec<u32>, u64)> {
        let q = format!(
            "SELECT ?seq ?parsed WHERE {{ GRAPH <{JOBS_GRAPH}> {{ ?b {} <{}> ; {} ?seq ; {} ?parsed }} }}",
            smj("job"),
            job_iri(id),
            smj("seq"),
            smj("parsed"),
        );
        let mut seqs = Vec::new();
        let mut triples = 0u64;
        for r in rows(store, &q)? {
            if let Some(seq) = value(&r, "seq").and_then(|v| v.parse().ok()) {
                seqs.push(seq);
            }
            triples += value(&r, "parsed")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
        }
        Ok((seqs, triples))
    }

    fn job_status(&self, store: &dyn SparqlStore, id: &str) -> Result<Value> {
        let Some(m) = self.manifest(store, id)? else {
            return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
        };
        let state = m
            .get("state")
            .and_then(|s| JobState::from_iri(s))
            .map_or("unknown", JobState::name);
        let (seqs, triples) = self.batches(store, id)?;
        let mut out = json!({
            "job_id": id,
            "state": state,
            "mode": m.get("mode"),
            "target_graph": m.get("target"),
            "replace": m.get("replace").map(|r| r == "true"),
            "format": m.get("format"),
            "created": m.get("created"),
            "last_activity": m.get("lastActivity"),
            "batches": seqs.len(),
            "received_seqs": seq_ranges(&seqs),
            "triples": triples,
        });
        for (key, field) in [
            ("label", "label"),
            ("staging_graph", "staging"),
            ("ended", "ended"),
        ] {
            if let Some(v) = m.get(field) {
                out[key] = json!(v);
            }
        }
        // How long an unfinished job has been idle, and how long it has left.
        let job_state = m.get("state").and_then(|s| JobState::from_iri(s));
        if matches!(job_state, Some(JobState::Open | JobState::Interrupted)) {
            if let Some(last) = m.get("lastActivity").and_then(|t| parse_iso(t)) {
                let idle = now_secs().saturating_sub(last);
                out["idle_secs"] = json!(idle);
                out["expires_in_secs"] = json!(self.cfg.job_idle_ttl_secs.saturating_sub(idle));
            }
        }
        if let Some(n) = m.get("expectedBatches").and_then(|n| n.parse::<u32>().ok()) {
            out["expected_batches"] = json!(n);
        }
        Ok(out)
    }

    /// The stored record of a batch: its hash and what it did.
    fn read_record(
        &self,
        store: &dyn SparqlStore,
        id: &str,
        seq: u32,
    ) -> Result<Option<BatchRecord>> {
        let q = format!(
            "SELECT ?sha ?parsed ?new ?dup WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{}> {} ?sha . \
             OPTIONAL {{ <{r}> {} ?parsed }} OPTIONAL {{ <{r}> {} ?new }} OPTIONAL {{ <{r}> {} ?dup }} }} }}",
            batch_iri(id, seq),
            smj("sha256"),
            smj("parsed"),
            smj("insertedNew"),
            smj("alreadyPresent"),
            r = batch_iri(id, seq),
        );
        Ok(rows(store, &q)?.first().map(|r| {
            let n = |k: &str| value(r, k).and_then(|v| v.parse().ok()).unwrap_or(0);
            BatchRecord {
                sha256: value(r, "sha").unwrap_or_default().to_string(),
                parsed: n("parsed"),
                inserted_new: n("new"),
                already_present: n("dup"),
            }
        }))
    }

    /// The job's running totals: batches accepted and triples parsed.
    fn totals(&self, m: &HashMap<String, String>) -> (u64, u64) {
        let n = |k: &str| m.get(k).and_then(|v| v.parse().ok()).unwrap_or(0);
        (n("batchCount"), n("triplesTotal"))
    }

    fn batch_reply(
        &self,
        id: &str,
        seq: u32,
        mode: Mode,
        rec: &BatchRecord,
        duplicate: bool,
        totals: (u64, u64),
    ) -> Value {
        let (batches, triples) = totals;
        let (new_key, dup_key) = match mode {
            Mode::Staged => ("new_to_staging", "already_staged"),
            Mode::Direct => ("new_to_target", "already_in_target"),
        };
        let max = self.max_triples(mode);
        let mut out = json!({
            "job_id": id,
            "seq": seq,
            "parsed": rec.parsed,
            "duplicate": duplicate,
            "job": {
                "batches": batches,
                "triples": triples,
                "remaining_triples": max.saturating_sub(triples),
            },
        });
        out[new_key] = json!(rec.inserted_new);
        out[dup_key] = json!(rec.already_present);
        out
    }

    fn max_triples(&self, mode: Mode) -> u64 {
        match mode {
            Mode::Staged => self.cfg.max_job_triples_staged,
            Mode::Direct => self.cfg.max_job_triples_direct,
        }
    }

    /// Accept one batch of a job.
    ///
    /// 1. Cheap checks and an idempotency fast path, outside the write gate.
    /// 2. Parse the whole payload: nothing is written on a syntax error.
    /// 3. Skolemize blank nodes, refuse reserved IRIs and oversized terms.
    /// 4. One transaction (`SparqlStore::write_batch`) that **re-checks** the job
    ///    is open and the batch is new, advances the job's counters against
    ///    their ceilings, and writes the data and the batch record. The fast
    ///    paths only save work; this is the check that counts, so a retry that
    ///    overlaps the original cannot be accepted twice.
    pub fn batch(&self, req: &BatchRequest) -> Result<Value> {
        let id = req.job_id.as_str();
        if !is_job_id(id) {
            return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
        }
        if req.seq == 0 || req.seq > self.cfg.max_job_batches {
            return Err(coded(
                "invalid_argument",
                format!("seq must be between 1 and {}", self.cfg.max_job_batches),
                Some(id),
            ));
        }
        if req.data.len() > self.cfg.max_batch_bytes {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_batch_bytes".to_string(),
                limit: self.cfg.max_batch_bytes as u64,
                value: req.data.len() as u64,
            }
            .into_anyhow());
        }
        let store: &dyn SparqlStore = self.privileged.as_ref();
        let Some(m) = self.manifest(store, id)? else {
            return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
        };
        let state = m.get("state").and_then(|s| JobState::from_iri(s));
        if !state.is_some_and(JobState::accepts_batches) {
            return Err(job_not_open(id, state));
        }
        let mode = m
            .get("mode")
            .and_then(|s| Mode::parse(s))
            .unwrap_or(Mode::Staged);
        let target = m.get("target").cloned().unwrap_or_default();
        let graph = match (mode, m.get("staging")) {
            (Mode::Staged, Some(staging)) => staging.clone(),
            _ => target.clone(),
        };
        let format = req
            .format
            .as_deref()
            .or_else(|| m.get("format").map(String::as_str))
            .unwrap_or("ntriples");
        let format = match format {
            "ntriples" => InputFormat::NTriples,
            "turtle" => InputFormat::Turtle,
            other => {
                return Err(coded(
                    "invalid_argument",
                    format!("format must be ntriples or turtle, not {other:?}"),
                    Some(id),
                ))
            }
        };
        let reject_blank = m.get("blankNodes").is_some_and(|b| b == "reject");
        let sha = format!("{:x}", Sha256::digest(req.data.as_bytes()));

        // Fast path: this batch was already accepted.
        if let Some(rec) = self.read_record(store, id, req.seq)? {
            return if rec.sha256 == sha {
                self.touch(id);
                // `m` was read before this record was seen. The record and the
                // job's counters are written in one transaction, so a manifest
                // read *after* the record includes it; the earlier one may not
                // (a retry that overlaps the original would report no triples).
                let fresh = self.manifest(store, id)?.unwrap_or_else(|| m.clone());
                let (batches, triples) = self.totals(&fresh);
                Ok(self.batch_reply(id, req.seq, mode, &rec, true, (batches, triples)))
            } else {
                Err(seq_conflict(id, req.seq))
            };
        }

        // Parse completely, then check and skolemize. Nothing is written yet.
        let parsed = store
            .parse_rdf_raw(req.data.as_bytes(), format)
            .map_err(|e| {
                ToolError::ParseError {
                    message: e.to_string(),
                    line: None,
                    column: None,
                }
                .into_anyhow()
            })?;
        if parsed.len() > self.cfg.max_batch_triples {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_batch_triples".to_string(),
                limit: self.cfg.max_batch_triples as u64,
                value: parsed.len() as u64,
            }
            .into_anyhow());
        }
        let parsed_count = parsed.len() as u64;
        let quads = prepare_quads(parsed, id, req.seq, reject_blank, &graph, &self.cfg)
            .map_err(ToolError::into_anyhow)?;

        // The write, with its conditions, in one transaction.
        let job = job_iri(id);
        let rec_iri = batch_iri(id, req.seq);
        let identity = ledger_quad(&rec_iri, "sha256", &string_literal(&sha));
        let write = BatchWrite {
            quads: quads.clone(),
            ledger: vec![
                ledger_quad(&rec_iri, "type_", &format!("<{SMJ}Batch>")),
                ledger_quad(&rec_iri, "job", &format!("<{job}>")),
                ledger_quad(&rec_iri, "seq", &typed(&req.seq.to_string(), "long")),
                identity.clone(),
                ledger_quad(
                    &rec_iri,
                    "parsed",
                    &typed(&parsed_count.to_string(), "long"),
                ),
                ledger_quad(&rec_iri, "accepted", &typed(&iso_now(), "dateTime")),
            ],
            require: vec![ledger_quad(
                &job,
                "state",
                &format!("<{}>", JobState::Open.iri()),
            )],
            idempotency: Some(Idempotency {
                subject: rec_iri.clone(),
                graph: JOBS_GRAPH.to_string(),
                identity,
            }),
            counters: vec![
                Counter {
                    subject: job.clone(),
                    predicate: format!("{SMJ}triplesTotal"),
                    graph: JOBS_GRAPH.to_string(),
                    add: parsed_count,
                    max: Some(self.max_triples(mode)),
                },
                Counter {
                    subject: job.clone(),
                    predicate: format!("{SMJ}batchCount"),
                    graph: JOBS_GRAPH.to_string(),
                    add: 1,
                    max: Some(u64::from(self.cfg.max_job_batches)),
                },
            ],
            replace: vec![Replace {
                subject: job.clone(),
                predicate: format!("{SMJ}lastActivity"),
                graph: JOBS_GRAPH.to_string(),
                object: typed(&iso_now(), "dateTime"),
            }],
            record_counts: Some(RecordCounts {
                subject: rec_iri,
                graph: JOBS_GRAPH.to_string(),
                inserted_new_predicate: format!("{SMJ}insertedNew"),
                already_present_predicate: format!("{SMJ}alreadyPresent"),
            }),
        };

        match self.privileged.write_batch(&write)? {
            Ok(applied) => {
                let (triples, batches) = (applied.counters[0], applied.counters[1]);
                self.after_batch(mode, &target, &quads, id, parsed_count, triples);
                let rec = BatchRecord {
                    sha256: sha,
                    parsed: parsed_count,
                    inserted_new: applied.inserted_new,
                    already_present: applied.already_present,
                };
                Ok(self.batch_reply(id, req.seq, mode, &rec, false, (batches, triples)))
            }
            Err(refusal) => self.refused_batch(store, id, req.seq, mode, &sha, refusal),
        }
    }

    /// Turn what the transaction refused into the answer the agent gets. A
    /// retry that lost the race for the same batch gets the original result.
    fn refused_batch(
        &self,
        store: &dyn SparqlStore,
        id: &str,
        seq: u32,
        mode: Mode,
        sha: &str,
        refusal: BatchRefusal,
    ) -> Result<Value> {
        match refusal {
            BatchRefusal::RequirementNotMet(_) => {
                let state = self
                    .manifest(store, id)?
                    .and_then(|m| m.get("state").and_then(|s| JobState::from_iri(s)));
                Err(job_not_open(id, state))
            }
            BatchRefusal::Duplicate => {
                let m = self.manifest(store, id)?.unwrap_or_default();
                let (batches, triples) = self.totals(&m);
                match self.read_record(store, id, seq)? {
                    Some(rec) if rec.sha256 == sha => {
                        self.touch(id);
                        Ok(self.batch_reply(id, seq, mode, &rec, true, (batches, triples)))
                    }
                    _ => Err(seq_conflict(id, seq)),
                }
            }
            BatchRefusal::Conflict => Err(seq_conflict(id, seq)),
            BatchRefusal::CounterLimit {
                counter,
                limit,
                value,
            } => {
                let key = match (counter, mode) {
                    (0, Mode::Staged) => "max_job_triples_staged",
                    (0, Mode::Direct) => "max_job_triples_direct",
                    _ => "max_job_batches",
                };
                Err(ToolError::LimitExceeded {
                    limit_key: key.to_string(),
                    limit,
                    value,
                }
                .into_anyhow())
            }
        }
    }

    /// What a direct job tells the viewer after a batch. A staged job is
    /// invisible until it commits.
    fn after_batch(
        &self,
        mode: Mode,
        target: &str,
        quads: &[StoredQuad],
        id: &str,
        parsed: u64,
        total: u64,
    ) {
        if mode != Mode::Direct {
            return;
        }
        if total as usize <= self.cfg.event_changed_threshold {
            emit_quads(
                self.events.as_ref(),
                QuadChange::Inserted,
                quads,
                EventLimits {
                    max_triples: self.cfg.event_max_triples,
                    changed_threshold: self.cfg.event_changed_threshold,
                },
            );
            return;
        }
        // Above the threshold a stream of inserts would flood a viewer, so
        // send one `Changed` now and then. Its counts are the job's totals.
        let mut last = self.last_changed.lock().unwrap_or_else(|e| e.into_inner());
        let due = last.get(id).is_none_or(|t| t.elapsed() >= CHANGED_EVERY);
        if due {
            last.insert(id.to_string(), Instant::now());
            self.events.emit(StoreEvent::Changed {
                graph: target.to_string(),
                triples_before: total.saturating_sub(parsed),
                triples_after: total,
            });
        }
    }

    /// The number of triples in `graph`.
    fn count_graph(&self, store: &dyn SparqlStore, graph: &str) -> Result<u64> {
        let Some(g) = iri(graph) else {
            return Ok(0);
        };
        let q = format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH {g} {{ ?s ?p ?o }} }}");
        Ok(rows(store, &q)?
            .first()
            .and_then(|r| value(r, "n"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0))
    }

    /// A first number from a `SELECT (COUNT(..) AS ?n)` query.
    fn count_query(&self, store: &dyn SparqlStore, query: &str) -> Result<u64> {
        Ok(rows(store, query)?
            .first()
            .and_then(|r| value(r, "n"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0))
    }

    /// Close a job: staged data moves into the target, and the job becomes
    /// `committed`, in **one transaction**.
    ///
    /// The whole commit holds the write gate, so no batch can land between the
    /// checks and the move. A batch that arrives meanwhile waits, then finds
    /// the job no longer open and is refused: it is either in the commit or
    /// not accepted, never lost in the staging graph.
    ///
    /// *   `staged`: `ADD SILENT <staging> TO <target> ; DROP SILENT GRAPH <staging>`.
    /// *   `staged` with `replace`: `MOVE SILENT <staging> TO <target>`, which
    ///     empties the target first. An empty staging graph is refused
    ///     (`empty_replace`) unless `allow_empty_replace`, so a retried commit
    ///     cannot wipe the target.
    /// *   `direct`: the data is already in the target; the commit only closes
    ///     the job.
    pub fn commit(&self, req: &CommitRequest) -> Result<Value> {
        let id = req.job_id.as_str();
        if !is_job_id(id) {
            return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
        }
        self.privileged
            .exclusive("ingest_commit", |store| -> Result<Value> {
                self.commit_locked(store.as_ref(), req)
            })?
    }

    fn commit_locked(&self, store: &dyn SparqlStore, req: &CommitRequest) -> Result<Value> {
        let id = req.job_id.as_str();
        let Some(m) = self.manifest(store, id)? else {
            return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
        };
        let state = m.get("state").and_then(|s| JobState::from_iri(s));
        if state != Some(JobState::Open) {
            return Err(job_not_open(id, state));
        }
        let mode = m
            .get("mode")
            .and_then(|s| Mode::parse(s))
            .unwrap_or(Mode::Staged);
        let replace = m.get("replace").is_some_and(|r| r == "true");
        let target = m.get("target").cloned().unwrap_or_default();
        let Some(target_iri) = iri(&target) else {
            return Err(coded(
                "invalid_target",
                format!("job {id} has no valid target"),
                Some(id),
            ));
        };

        // Every batch the agent said to expect, and every batch it sent, must be there.
        let (seqs, _) = self.batches(store, id)?;
        let expected = m
            .get("expectedBatches")
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);
        let need = expected.max(seqs.iter().copied().max().unwrap_or(0));
        let have: BTreeSet<u32> = seqs.iter().copied().collect();
        let missing: Vec<u32> = (1..=need).filter(|n| !have.contains(n)).collect();
        if !missing.is_empty() {
            return Err(coded(
                "missing_batches",
                format!(
                    "batches {} of 1-{need} have not been received; send them, or abort the job",
                    seq_ranges(&missing)
                ),
                Some(id),
            ));
        }
        if req.require_validation {
            self.check_validated(&m, id)?;
        }

        let (batches, total) = self.totals(&m);
        let before = self.count_graph(store, &target)?;
        let mut reply = json!({
            "job_id": id,
            "state": JobState::Committed.name(),
            "mode": mode.name(),
            "target_graph": target,
            "replace": replace,
            "batches": batches,
            "triples": total,
        });
        let mut staged_events: Option<Vec<StoredQuad>> = None;

        let update = match (mode, m.get("staging")) {
            (Mode::Staged, Some(staging)) => {
                let Some(staging_iri) = iri(staging) else {
                    return Err(coded(
                        "invalid_target",
                        format!("job {id} has no valid staging graph"),
                        Some(id),
                    ));
                };
                let staged = self.count_graph(store, staging)?;
                if replace && staged == 0 && !req.allow_empty_replace {
                    return Err(coded(
                        "empty_replace",
                        "the staging graph is empty, and a replace would empty the target. \
                         If that is what you want, commit with allow_empty_replace: true",
                        Some(id),
                    ));
                }

                // What this commit adds, measured before it runs.
                let new_triples = self.count_query(
                    store,
                    &format!(
                        "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH {staging_iri} {{ ?s ?p ?o }} \
                     FILTER NOT EXISTS {{ GRAPH {target_iri} {{ ?s ?p ?o }} }} }}"
                    ),
                )?;
                let new_subjects_where = format!(
                    "WHERE {{ GRAPH {staging_iri} {{ ?s ?p ?o }} FILTER(isIRI(?s)) \
                     FILTER NOT EXISTS {{ GRAPH {target_iri} {{ ?s ?x ?y }} }} }}"
                );
                let subject_count = self.count_query(
                    store,
                    &format!("SELECT (COUNT(DISTINCT ?s) AS ?n) {new_subjects_where}"),
                )?;
                let listed = rows(
                    store,
                    &format!(
                        "SELECT DISTINCT ?s {new_subjects_where} ORDER BY ?s LIMIT {}",
                        MAX_NEW_SUBJECTS + 1
                    ),
                )?;
                let mut subjects: Vec<String> = listed
                    .iter()
                    .filter_map(|r| value(r, "s").map(str::to_string))
                    .collect();
                let truncated = subjects.len() > MAX_NEW_SUBJECTS;
                subjects.truncate(MAX_NEW_SUBJECTS);
                reply["new_to_target"] = json!(new_triples);
                reply["new_subject_count"] = json!(subject_count);
                reply["new_subjects"] = json!(subjects);
                reply["new_subjects_truncated"] = json!(truncated);

                // A small append is reported triple by triple, read before the move.
                if !replace && (total as usize) <= self.cfg.event_changed_threshold {
                    let quads = rows(
                        store,
                        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH {staging_iri} {{ ?s ?p ?o }} }}"),
                    )?
                    .iter()
                    .filter_map(|r| {
                        Some(StoredQuad {
                            s: r.get("s")?.to_ntriples(),
                            p: r.get("p")?.to_ntriples(),
                            o: r.get("o")?.to_ntriples(),
                            g: Some(target.clone()),
                        })
                    })
                    .collect();
                    staged_events = Some(quads);
                }

                let mv = if replace {
                    format!("MOVE SILENT {staging_iri} TO {target_iri}")
                } else {
                    format!("ADD SILENT {staging_iri} TO {target_iri} ; DROP SILENT GRAPH {staging_iri}")
                };
                format!("{mv} ; {}", self.transition_update(id, JobState::Committed))
            }
            _ => self.transition_update(id, JobState::Committed),
        };
        store.update(&update)?;

        let after = self.count_graph(store, &target)?;
        reply["target_triples"] = json!(after);
        if replace {
            let began = m
                .get("targetSizeAtBegin")
                .and_then(|n| n.parse::<u64>().ok());
            reply["target_writes_discarded"] = json!(began.is_some_and(|b| b != before));
        }
        self.commit_events(mode, &target, staged_events, total, before, after, id);
        Ok(reply)
    }

    /// `require_validation` is satisfied by a passing validation that covers
    /// every batch received so far.
    fn check_validated(&self, m: &HashMap<String, String>, id: &str) -> Result<()> {
        let passed = m.get("validationPassed").is_some_and(|p| p == "true");
        let at = m
            .get("validatedAtBatches")
            .and_then(|n| n.parse::<u64>().ok());
        let (batches, _) = self.totals(m);
        match (passed, at) {
            (true, Some(n)) if n == batches => Ok(()),
            (true, Some(_)) => Err(coded(
                "validation_stale",
                "batches arrived after the last validation; run ingest_validate again",
                Some(id),
            )),
            _ => Err(coded(
                "validation_required",
                "no passing validation is recorded for this job (ingest_validate is not \
                 available in this build)",
                Some(id),
            )),
        }
    }

    /// Tell the viewer what the commit did.
    #[allow(clippy::too_many_arguments)]
    fn commit_events(
        &self,
        mode: Mode,
        target: &str,
        staged: Option<Vec<StoredQuad>>,
        total: u64,
        before: u64,
        after: u64,
        id: &str,
    ) {
        match (mode, staged) {
            // A small staged append: the triples, in bounded events.
            (Mode::Staged, Some(quads)) => emit_quads(
                self.events.as_ref(),
                QuadChange::Inserted,
                &quads,
                EventLimits {
                    max_triples: self.cfg.event_max_triples,
                    changed_threshold: self.cfg.event_changed_threshold,
                },
            ),
            // A large staged append, or any replace: one `Changed`, with the
            // target's real sizes, and the viewer resyncs.
            (Mode::Staged, None) => self.events.emit(StoreEvent::Changed {
                graph: target.to_string(),
                triples_before: before,
                triples_after: after,
            }),
            // A direct job already sent its batches; if it was big, the
            // throttled `Changed` events may have skipped the end.
            (Mode::Direct, _) if total as usize > self.cfg.event_changed_threshold => {
                self.events.emit(StoreEvent::Changed {
                    graph: target.to_string(),
                    triples_before: after.saturating_sub(total),
                    triples_after: after,
                })
            }
            (Mode::Direct, _) => {}
        }
        self.last_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    /// The update that moves a job to `state` and stamps it, as one request
    /// fragment: it matches only the job's current state quads.
    fn transition_update(&self, id: &str, state: JobState) -> String {
        let job = job_iri(id);
        let now = typed(&iso_now(), "dateTime");
        let (state_p, active_p, ended_p) = (smj("state"), smj("lastActivity"), smj("ended"));
        format!(
            "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }} \
             INSERT {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} <{}> ; {active_p} {now} ; {ended_p} {now} }} }} \
             WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }}",
            state.iri()
        )
    }

    /// Abort a job that is open, interrupted or failed. A staged job's staging
    /// graph is dropped in the same transaction as the state change. A direct
    /// job's accepted batches stay in the target graph: the reply lists them.
    pub fn abort(&self, job_id: &str) -> Result<Value> {
        let id = job_id.to_string();
        self.privileged
            .exclusive("ingest_abort", |store| -> Result<Value> {
                let Some(m) = self.manifest(store.as_ref(), &id)? else {
                    return Err(coded("job_not_found", format!("no job {id}"), Some(&id)));
                };
                let state = m.get("state").and_then(|s| JobState::from_iri(s));
                match state {
                    Some(s) if s.can_abort() => {}
                    other => {
                        return Err(coded(
                            "job_not_open",
                            format!(
                                "job {id} is {}: only an open, interrupted or failed job can be aborted",
                                other.map_or("in an unknown state", JobState::name)
                            ),
                            Some(&id),
                        ))
                    }
                }
                let (seqs, _) = self.batches(store.as_ref(), &id)?;
                let direct = m.get("mode").is_some_and(|mode| mode == "direct");

                let mut update = self.transition_update(&id, JobState::Aborted);
                if let Some(staging) = m.get("staging") {
                    // The staging graph goes with the state change, in one
                    // transaction. (Large staging graphs are dropped in
                    // chunks once batches exist.)
                    update.push_str(&format!(" ; DROP SILENT GRAPH <{staging}>"));
                }
                store.update(&update)?;

                let mut reply = json!({
                    "job_id": id,
                    "state": JobState::Aborted.name(),
                    "direct_data_kept": direct && !seqs.is_empty(),
                });
                if direct && !seqs.is_empty() {
                    reply["kept_seqs"] = json!(seq_ranges(&seqs));
                }
                Ok(reply)
            })?
    }
}

/// What `ingest_commit` is given.
#[derive(Debug, Clone)]
pub struct CommitRequest {
    pub job_id: String,
    pub require_validation: bool,
    pub allow_empty_replace: bool,
}

/// What `ingest_batch` is given.
#[derive(Debug, Clone)]
pub struct BatchRequest {
    pub job_id: String,
    pub seq: u32,
    pub data: String,
    /// Overrides the job's format for this batch.
    pub format: Option<String>,
}

/// A batch as recorded in the ledger.
#[derive(Debug, Clone)]
struct BatchRecord {
    sha256: String,
    parsed: u64,
    inserted_new: u64,
    already_present: u64,
}

pub fn batch_iri(id: &str, seq: u32) -> String {
    format!("{JOB_PREFIX}{id}:batch:{seq}")
}

/// A ledger quad (in the jobs graph) with an `smj:` predicate. `object` is
/// already in its N-Triples form. `type_` stands for `rdf:type`.
fn ledger_quad(subject: &str, predicate: &str, object: &str) -> StoredQuad {
    let predicate = if predicate == "type_" {
        "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".to_string()
    } else {
        format!("<{SMJ}{predicate}>")
    };
    StoredQuad {
        s: format!("<{subject}>"),
        p: predicate,
        o: object.to_string(),
        g: Some(JOBS_GRAPH.to_string()),
    }
}

fn job_not_open(id: &str, state: Option<JobState>) -> anyhow::Error {
    coded(
        "job_not_open",
        format!(
            "job {id} is {}: only an open job accepts batches",
            state.map_or("in an unknown state", JobState::name)
        ),
        Some(id),
    )
}

fn seq_conflict(id: &str, seq: u32) -> anyhow::Error {
    coded(
        "seq_conflict",
        format!(
            "batch {seq} of job {id} was already accepted with different content; \
             send corrected data under a new seq, or abort the job"
        ),
        Some(id),
    )
}

/// A blank node label the parser generated itself (a Turtle `[]` or
/// collection): 32 hexadecimal digits. Anything else was written by the agent.
fn is_generated_label(label: &str) -> bool {
    label.len() == 32 && label.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Percent-encode a blank node label so it is a safe IRI path segment.
fn pct_encode(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for b in label.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Check and rewrite a parsed batch, and put every quad in `graph`.
///
/// *   An IRI in the reserved namespace is refused (`reserved_iri`), and an IRI
///     or literal over its size limit is `limit_exceeded`.
/// *   A **labelled** blank node `_:x` becomes `<urn:sparql-mcp:genid:{job}:x>`,
///     the same IRI in every batch and every retry of the job. An
///     **anonymous** one (generated by the parser, so its label is random)
///     becomes `<urn:sparql-mcp:genid:{job}:{seq}:a{n}>`, `n` being the order of
///     first appearance in the batch, so a retried batch gets the same IRIs.
/// *   With `reject_blank`, any blank node is `blank_node_rejected`.
pub fn prepare_quads(
    quads: Vec<StoredQuad>,
    job_id: &str,
    seq: u32,
    reject_blank: bool,
    graph: &str,
    cfg: &IngestConfig,
) -> std::result::Result<Vec<StoredQuad>, ToolError> {
    let mut anonymous: HashMap<String, usize> = HashMap::new();
    let mut map = |term: &str| -> std::result::Result<String, ToolError> {
        if let Some(label) = term.strip_prefix("_:") {
            if reject_blank {
                return Err(ToolError::Coded {
                    code: "blank_node_rejected",
                    message: "this job was started with blank_nodes: reject, and the batch \
                              contains a blank node"
                        .to_string(),
                    job_id: Some(job_id.to_string()),
                });
            }
            return Ok(if is_generated_label(label) {
                let next = anonymous.len();
                let n = *anonymous.entry(label.to_string()).or_insert(next);
                format!("<{GENID}{job_id}:{seq}:a{n}>")
            } else {
                format!("<{GENID}{job_id}:{}>", pct_encode(label))
            });
        }
        if let Some(iri) = term.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
            if iri.starts_with(RESERVED_PREFIX) {
                return Err(ToolError::ReservedIri {
                    iri: iri.to_string(),
                });
            }
            if iri.len() > cfg.max_iri_bytes {
                return Err(ToolError::LimitExceeded {
                    limit_key: "max_iri_bytes".to_string(),
                    limit: cfg.max_iri_bytes as u64,
                    value: iri.len() as u64,
                });
            }
        } else if term.len() > cfg.max_literal_bytes {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_literal_bytes".to_string(),
                limit: cfg.max_literal_bytes as u64,
                value: term.len() as u64,
            });
        }
        Ok(term.to_string())
    };
    quads
        .into_iter()
        .map(|q| {
            Ok(StoredQuad {
                s: map(&q.s)?,
                p: map(&q.p)?,
                o: map(&q.o)?,
                g: Some(graph.to_string()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip_through_their_iris() {
        for s in JobState::ALL {
            assert_eq!(JobState::from_iri(&s.iri()), Some(s));
        }
        assert_eq!(JobState::from_iri("urn:other#open"), None);
        assert_eq!(JobState::from_iri(&format!("{SMJ}nonsense")), None);
    }

    #[test]
    fn the_state_machine_matches_the_design() {
        use JobState::*;
        assert!(Open.accepts_batches());
        for s in [Running, Interrupted, Failed, Committed, Aborted, Expired] {
            assert!(!s.accepts_batches(), "{s:?} must not accept batches");
        }
        for s in [Open, Interrupted, Failed] {
            assert!(s.can_abort(), "{s:?}");
        }
        for s in [Running, Committed, Aborted, Expired] {
            assert!(!s.can_abort(), "{s:?}");
        }
        // Only these count against max_open_jobs.
        let active: Vec<_> = JobState::ALL
            .into_iter()
            .filter(|s| s.is_active())
            .collect();
        assert_eq!(active, vec![Open, Running, Interrupted]);
    }

    #[test]
    fn job_ids_are_ulids_and_nothing_else() {
        assert!(is_job_id(&Ulid::new().to_string()));
        for bad in [
            "",
            "abc",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV> . DROP ALL",
            "../../etc",
        ] {
            assert!(!is_job_id(bad), "{bad:?}");
        }
        // 26 characters but not valid base 32 ('U' is excluded from the alphabet).
        assert!(!is_job_id("UUUUUUUUUUUUUUUUUUUUUUUUUU"));
    }

    #[test]
    fn sequence_numbers_are_reported_as_ranges() {
        assert_eq!(seq_ranges(&[]), "");
        assert_eq!(seq_ranges(&[1, 2, 3, 5]), "1-3,5");
        assert_eq!(seq_ranges(&[5, 3, 1, 2, 2]), "1-3,5");
        assert_eq!(seq_ranges(&[7]), "7");
        assert_eq!(
            seq_ranges(&(1..=30).chain([32]).collect::<Vec<_>>()),
            "1-30,32"
        );
    }

    #[test]
    fn the_job_and_staging_iris_are_under_the_reserved_namespace() {
        assert!(is_protected(&staging_iri("X")));
        assert!(is_protected(JOBS_GRAPH));
        assert!(job_iri("X").starts_with("urn:sparql-mcp:"));
    }
    fn quad(s: &str, p: &str, o: &str) -> StoredQuad {
        StoredQuad {
            s: s.to_string(),
            p: p.to_string(),
            o: o.to_string(),
            g: None,
        }
    }

    const JOB: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn prep(quads: Vec<StoredQuad>, seq: u32) -> std::result::Result<Vec<StoredQuad>, ToolError> {
        prepare_quads(quads, JOB, seq, false, "urn:g", &IngestConfig::default())
    }

    #[test]
    fn a_labelled_blank_node_is_the_same_iri_in_every_batch_of_the_job() {
        let q = vec![
            quad("_:a", "<urn:p>", "_:b"),
            quad("_:a", "<urn:q>", "\"x\""),
        ];
        let one = prep(q.clone(), 1).unwrap();
        let two = prep(q, 2).unwrap();
        // Same labels, different batches: the same skolem IRIs.
        assert_eq!(one[0].s, format!("<{GENID}{JOB}:a>"));
        assert_eq!(one[0].o, format!("<{GENID}{JOB}:b>"));
        assert_eq!(one[0].s, two[0].s);
        assert_eq!(one[0].s, one[1].s, "_:a is one node within a batch");
        // Everything lands in the job's graph.
        assert!(one.iter().all(|q| q.g.as_deref() == Some("urn:g")));
    }

    #[test]
    fn an_anonymous_node_is_numbered_by_first_appearance_and_stable_on_retry() {
        // Two parses of the same Turtle give different random labels, but the
        // same shape; the skolem IRIs must not depend on the random label.
        let a = "e29eb7b417983b13f1fe41d6e2793a70";
        let b = "0123456789abcdef0123456789abcdef";
        let first = prep(
            vec![quad(&format!("_:{a}"), "<urn:p>", &format!("_:{b}"))],
            3,
        )
        .unwrap();
        let again_a = "ffffffffffffffffffffffffffffffff";
        let again_b = "00000000000000000000000000000001";
        let second = prep(
            vec![quad(
                &format!("_:{again_a}"),
                "<urn:p>",
                &format!("_:{again_b}"),
            )],
            3,
        )
        .unwrap();
        assert_eq!(first, second, "a retried batch must produce the same quads");
        assert_eq!(first[0].s, format!("<{GENID}{JOB}:3:a0>"));
        assert_eq!(first[0].o, format!("<{GENID}{JOB}:3:a1>"));
    }

    #[test]
    fn labels_are_percent_encoded_so_they_cannot_break_the_iri() {
        let out = prep(vec![quad("_:we ird>", "<urn:p>", "\"x\"")], 1).unwrap();
        assert_eq!(out[0].s, format!("<{GENID}{JOB}:we%20ird%3E>"));
    }

    #[test]
    fn blank_nodes_can_be_rejected() {
        let err = prepare_quads(
            vec![quad("_:a", "<urn:p>", "\"x\"")],
            JOB,
            1,
            true,
            "urn:g",
            &IngestConfig::default(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "blank_node_rejected");
        // A batch with no blank node is fine in that mode.
        assert!(prepare_quads(
            vec![quad("<urn:a>", "<urn:p>", "\"x\"")],
            JOB,
            1,
            true,
            "urn:g",
            &IngestConfig::default()
        )
        .is_ok());
    }

    #[test]
    fn the_reserved_namespace_and_oversized_terms_are_refused() {
        for bad in [
            quad("<urn:sparql-mcp:genid:x:1>", "<urn:p>", "\"x\""),
            quad("<urn:a>", "<urn:sparql-mcp:jobs#state>", "\"x\""),
            quad("<urn:a>", "<urn:p>", "<urn:sparql-mcp:job:X>"),
        ] {
            assert_eq!(prep(vec![bad], 1).unwrap_err().code(), "reserved_iri");
        }
        let cfg = IngestConfig {
            max_iri_bytes: 20,
            max_literal_bytes: 20,
            ..IngestConfig::default()
        };
        let long_iri = quad(&format!("<urn:{}>", "x".repeat(30)), "<urn:p>", "\"x\"");
        let e = prepare_quads(vec![long_iri], JOB, 1, false, "urn:g", &cfg).unwrap_err();
        assert!(
            matches!(&e, ToolError::LimitExceeded { limit_key, .. } if limit_key == "max_iri_bytes")
        );
        let long_lit = quad("<urn:a>", "<urn:p>", &format!("\"{}\"", "x".repeat(40)));
        let e = prepare_quads(vec![long_lit], JOB, 1, false, "urn:g", &cfg).unwrap_err();
        assert!(
            matches!(&e, ToolError::LimitExceeded { limit_key, .. } if limit_key == "max_literal_bytes")
        );
    }

    #[test]
    fn generated_labels_are_told_from_written_ones() {
        assert!(is_generated_label("e29eb7b417983b13f1fe41d6e2793a70"));
        assert!(!is_generated_label("b0"));
        assert!(!is_generated_label("e29eb7b417983b13f1fe41d6e2793a7")); // 31
        assert!(!is_generated_label("g29eb7b417983b13f1fe41d6e2793a70")); // not hex
    }
}
