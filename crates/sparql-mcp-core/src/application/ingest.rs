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

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use serde_json::{json, Value};
use ulid::Ulid;

use crate::application::gated_store::PrivilegedStore;
use crate::application::guards::is_protected;
use crate::application::limits::ToolError;
use crate::application::sparql_text::{iri, string_literal};
use crate::application::tools::project::iso_now;
use crate::config::IngestConfig;
use crate::domain::{QueryResult, RdfTerm, SparqlStore};

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
}

impl IngestService {
    pub fn new(
        privileged: Arc<PrivilegedStore>,
        active_graph: Arc<RwLock<String>>,
        cfg: IngestConfig,
    ) -> Self {
        Self {
            privileged,
            active_graph,
            cfg,
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
                ];
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
        if let Some(n) = m.get("expectedBatches").and_then(|n| n.parse::<u32>().ok()) {
            out["expected_batches"] = json!(n);
        }
        Ok(out)
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

                let job = job_iri(&id);
                let now = typed(&iso_now(), "dateTime");
                let (state_p, active_p, ended_p) =
                    (smj("state"), smj("lastActivity"), smj("ended"));
                let mut update = format!(
                    "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }} \
                     INSERT {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} <{}> ; {active_p} {now} ; {ended_p} {now} }} }} \
                     WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }}",
                    JobState::Aborted.iri()
                );
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
}
