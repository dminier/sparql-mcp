//! `ingest_file`: stream an N-Triples file into a job, in batches, on a
//! background thread.
//!
//! *   The file must be inside `[ingest] file_roots` (refused when unset). It
//!     is hashed first (SHA-256, at most `max_file_bytes`); a file already
//!     loaded into the same graph is refused (`already_loaded`) unless `force`.
//! *   The runner cuts the file at line boundaries, every `file_batch_triples`
//!     triples or at most `max_batch_bytes`, and writes each cut as an ordinary
//!     batch through [`IngestService::accept`]. The same transaction moves the
//!     job's checkpoint (`smj:checkpointBytes`, the offset just after the last
//!     line written), so the data and the checkpoint can never disagree.
//! *   The write gate is held per batch only, so other writers interleave.
//! *   A restart turns a `running` job into `interrupted`; `ingest_file` with
//!     `resume_job` re-hashes the file, refuses a changed one
//!     (`source_changed`), and continues from the checkpoint.
//! *   `ingest_abort` on a running job asks the runner to stop after its
//!     current batch (the job becomes `interrupted`; abort it again to close it).
//!
//! Only N-Triples is streamed: its lines are independent, so a byte offset is a
//! valid checkpoint. Turtle files are refused (`invalid_argument`).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::application::path_guard;
use crate::domain::loaded_files::{META_FILE, META_GRAPH, META_PATH, META_SHA256, META_TARGET};

/// What `ingest_file` is asked.
#[derive(Debug, Clone)]
pub struct FileRequest {
    pub path: String,
    pub target_graph: Option<String>,
    pub mode: Mode,
    /// Only `ntriples` is streamed; the default follows the extension.
    pub format: Option<String>,
    pub resume_job: Option<String>,
    pub force: bool,
}

/// The live part of a running file job, shared with its runner thread.
pub(super) struct RunState {
    pub(super) cancel: AtomicBool,
    started: Instant,
    base_triples: u64,
    base_bytes: u64,
}

/// How a run ended.
enum RunEnd {
    Done,
    Stopped,
}

/// Everything a runner needs, fixed when it starts.
struct RunSpec {
    id: String,
    path: PathBuf,
    sha256: String,
    mode: Mode,
    target: String,
    start_offset: u64,
    start_seq: u32,
}

fn hash_file(path: &Path, max_bytes: u64, job: Option<&str>) -> Result<(String, u64)> {
    let mut file = File::open(path).map_err(|e| {
        coded(
            "path_rejected",
            format!("cannot open {}: {e}", path.display()),
            job,
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max_bytes {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_file_bytes".into(),
                limit: max_bytes,
                value: total,
            }
            .into_anyhow());
        }
        hasher.update(&buf[..n]);
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// A line that carries a triple: not blank, not a comment.
fn is_triple_line(line: &[u8]) -> bool {
    line.iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b != b'#')
}

fn ledger_quad_in(graph: &str, s: &str, p: &str, o: String) -> StoredQuad {
    StoredQuad {
        s: format!("<{s}>"),
        p: format!("<{p}>"),
        o,
        g: Some(graph.to_string()),
    }
}

/// The loaded-file ledger entry for `(hash, target)`, as the store adapter
/// writes it for a whole-file load.
fn loaded_file_entry(hash: &str, target: &str, path: &str) -> Vec<StoredQuad> {
    let target_hash = format!("{:x}", Sha256::digest(target.as_bytes()));
    let subject = format!("urn:hkb:meta:file:{hash}:{target_hash}");
    let q = |p: &str, o: String| ledger_quad_in(META_GRAPH, &subject, p, o);
    vec![
        q(
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type",
            format!("<{META_FILE}>"),
        ),
        q(META_SHA256, string_literal(hash)),
        q(META_PATH, string_literal(path)),
        q(META_TARGET, string_literal(target)),
    ]
}

impl IngestService {
    /// Start (or resume) a file job and return at once; the file is read in
    /// the background. Follow it with `ingest_status`.
    pub fn start_file(self: &Arc<Self>, req: &FileRequest) -> Result<Value> {
        let Some(roots) = &self.cfg.file_roots else {
            return Err(coded(
                "file_roots_unset",
                "ingest_file reads server-side files, so [ingest] file_roots must be set to \
                 the directories it may read",
                None,
            ));
        };
        let roots = path_guard::resolve_roots(roots)?;
        let path = path_guard::check_read(Path::new(&req.path), &roots).map_err(|e| {
            // A refusal by the guard keeps its code; a path that cannot be
            // resolved (it does not exist) becomes one too.
            if e.downcast_ref::<ToolError>().is_some() {
                e
            } else {
                coded("file_not_found", format!("{}: {e}", req.path), None)
            }
        })?;

        let format = req.format.clone().or_else(|| {
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            (ext == "nt").then(|| "ntriples".to_string())
        });
        if format.as_deref() != Some("ntriples") {
            return Err(coded(
                "invalid_argument",
                "ingest_file streams N-Triples (a .nt file, or format: ntriples): its lines \
                 are independent, so a byte offset is a safe checkpoint. Convert Turtle files, \
                 or load a small one with load_rdf_file",
                None,
            ));
        }
        let meta = std::fs::metadata(&path)
            .map_err(|e| coded("path_rejected", format!("{}: {e}", path.display()), None))?;
        if !meta.is_file() {
            return Err(ToolError::PathRejected {
                path: req.path.clone(),
            }
            .into_anyhow());
        }
        if meta.len() > self.cfg.max_file_bytes {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_file_bytes".into(),
                limit: self.cfg.max_file_bytes,
                value: meta.len(),
            }
            .into_anyhow());
        }
        let (sha256, bytes) = hash_file(&path, self.cfg.max_file_bytes, None)?;
        let path_text = path.to_string_lossy().to_string();

        if let Some(job) = &req.resume_job {
            return self.resume_file(job, &path, &path_text, &sha256);
        }

        let target = req
            .target_graph
            .clone()
            .unwrap_or_else(|| self.active_graph());
        if !req.force && self.file_already_loaded(&sha256, &target)? {
            return Err(coded(
                "already_loaded",
                format!(
                    "this file (sha256 {sha256}) was already loaded into {target}; pass \
                     force: true to load it again"
                ),
                None,
            ));
        }
        // A staged job is committed in one transaction: refuse a file that
        // cannot fit, by a rough estimate of 100 bytes per triple. The job's
        // own counter enforces the real ceiling as it runs.
        if req.mode == Mode::Staged && bytes / 100 > self.cfg.max_job_triples_staged {
            return Err(coded(
                "staged_too_large",
                format!(
                    "a {bytes}-byte file is about {} triples, above max_job_triples_staged = {}; \
                     use mode direct",
                    bytes / 100,
                    self.cfg.max_job_triples_staged
                ),
                None,
            ));
        }
        let begun = self.begin(&BeginRequest {
            target_graph: Some(target.clone()),
            mode: req.mode,
            replace: false,
            format: "ntriples".into(),
            expected_batches: None,
            label: Some(
                path.file_name()
                    .map(|n| n.to_string_lossy().chars().take(MAX_LABEL_CHARS).collect())
                    .unwrap_or_default(),
            ),
            blank_nodes: "skolemize".into(),
            file: Some(FileMeta {
                path: path_text,
                sha256: sha256.clone(),
                bytes,
            }),
        })?;
        let id = begun["job_id"].as_str().unwrap_or_default().to_string();
        self.spawn_run(RunSpec {
            id,
            path,
            sha256,
            mode: req.mode,
            target,
            start_offset: 0,
            start_seq: 1,
        });
        let mut reply = begun;
        reply["file"] = json!({ "bytes": bytes });
        Ok(reply)
    }

    fn resume_file(
        self: &Arc<Self>,
        id: &str,
        path: &Path,
        path_text: &str,
        sha256: &str,
    ) -> Result<Value> {
        let spec = self
            .privileged
            .exclusive("ingest_file", |s| -> Result<RunSpec> {
                let Some(m) = self.manifest(s.as_ref(), id)? else {
                    return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
                };
                let Some(recorded) = m.get("fileSha256") else {
                    return Err(coded(
                        "invalid_argument",
                        format!("job {id} is not a file job"),
                        Some(id),
                    ));
                };
                let state = m.get("state").and_then(|x| JobState::from_iri(x));
                if state != Some(JobState::Interrupted) {
                    return Err(coded(
                        "job_not_interrupted",
                        format!(
                            "job {id} is {}: only an interrupted file job can be resumed",
                            state.map_or("in an unknown state", JobState::name)
                        ),
                        Some(id),
                    ));
                }
                if recorded != sha256 {
                    return Err(coded(
                        "source_changed",
                        format!(
                            "{path_text} is not the file this job was reading (sha256 differs); \
                             abort the job and start a new one"
                        ),
                        Some(id),
                    ));
                }
                let (batches, _) = self.totals(&m);
                let mode = m
                    .get("mode")
                    .and_then(|x| Mode::parse(x))
                    .unwrap_or(Mode::Direct);
                let job = job_iri(id);
                let (state_p, active_p) = (smj("state"), smj("lastActivity"));
                s.update(&format!(
                    "DELETE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }} \
                     INSERT {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} <{}> ; {active_p} {} }} }} \
                     WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {state_p} ?s ; {active_p} ?t }} }}",
                    JobState::Running.iri(),
                    typed(&iso_now(), "dateTime"),
                ))?;
                // The failure reason of an earlier run no longer applies.
                s.update(&format!(
                    "DELETE WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {} ?e }} }}",
                    smj("error")
                ))?;
                Ok(RunSpec {
                    id: id.to_string(),
                    path: path.to_path_buf(),
                    sha256: sha256.to_string(),
                    mode,
                    target: m.get("target").cloned().unwrap_or_default(),
                    start_offset: m
                        .get("checkpointBytes")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                    start_seq: u32::try_from(batches).unwrap_or(u32::MAX).saturating_add(1),
                })
            })??;
        let reply = json!({
            "job_id": id,
            "state": JobState::Running.name(),
            "resumed_from_byte": spec.start_offset,
            "next_seq": spec.start_seq,
        });
        self.spawn_run(spec);
        Ok(reply)
    }

    /// Whether `hash` was already loaded into `target`.
    fn file_already_loaded(&self, hash: &str, target: &str) -> Result<bool> {
        let store: &dyn SparqlStore = self.privileged.as_ref();
        let q = format!(
            "ASK {{ GRAPH <{META_GRAPH}> {{ ?f <{META_SHA256}> {} ; <{META_TARGET}> {} }} }}",
            string_literal(hash),
            string_literal(target)
        );
        Ok(matches!(store.query(&q)?, QueryResult::Boolean(true)))
    }

    fn spawn_run(self: &Arc<Self>, spec: RunSpec) {
        let (triples, _) = {
            let store: &dyn SparqlStore = self.privileged.as_ref();
            self.manifest(store, &spec.id)
                .ok()
                .flatten()
                .map_or((0, 0), |m| self.totals(&m))
        };
        let state = Arc::new(RunState {
            cancel: AtomicBool::new(false),
            started: Instant::now(),
            base_triples: triples,
            base_bytes: spec.start_offset,
        });
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(spec.id.clone(), state.clone());
        let svc = self.clone();
        let spawned = std::thread::Builder::new()
            .name("sparql-mcp-ingest-file".into())
            .spawn({
                let spec = RunSpec { ..spec };
                move || svc.run_file(spec, state)
            });
        if let Err(e) = spawned {
            tracing::error!(error = %e, "could not start the file runner thread");
        }
    }

    fn run_file(self: Arc<Self>, spec: RunSpec, state: Arc<RunState>) {
        let outcome = self.run_loop(&spec, &state);
        let (to, error) = match &outcome {
            Ok(RunEnd::Done) if spec.mode == Mode::Direct => (JobState::Committed, None),
            Ok(RunEnd::Done) => (JobState::Open, None),
            Ok(RunEnd::Stopped) => (JobState::Interrupted, None),
            Err(e) => (JobState::Failed, Some(e.to_string())),
        };
        if let Err(e) = self.finish_run(&spec.id, to, error.as_deref()) {
            tracing::warn!(job = %spec.id, error = %e, "could not record the end of a file job");
        }
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&spec.id);
        if matches!(outcome, Ok(RunEnd::Done)) && spec.mode == Mode::Direct {
            self.final_changed(&spec);
        }
        // A big load leaves much of its data in write buffers, which the store
        // keeps until they fill. Writing them out gives the memory back.
        if let Err(e) = self.privileged.flush() {
            tracing::warn!(job = %spec.id, error = %e, "could not flush the store after a file job");
        }
    }

    /// One `Changed` for a large direct load, so a viewer refreshes.
    fn final_changed(&self, spec: &RunSpec) {
        let store: &dyn SparqlStore = self.privileged.as_ref();
        let total = self
            .manifest(store, &spec.id)
            .ok()
            .flatten()
            .map_or(0, |m| self.totals(&m).1);
        if total as usize > self.cfg.event_changed_threshold {
            self.events.emit(StoreEvent::Changed {
                graph: spec.target.clone(),
                triples_before: 0,
                triples_after: total,
            });
        }
    }

    /// Record how a run ended, unless the job is no longer `running`.
    fn finish_run(&self, id: &str, to: JobState, error: Option<&str>) -> Result<()> {
        self.privileged
            .exclusive("ingest_file_end", |s| -> Result<()> {
                let running = self
                    .manifest(s.as_ref(), id)?
                    .and_then(|m| m.get("state").and_then(|x| JobState::from_iri(x)))
                    == Some(JobState::Running);
                if !running {
                    return Ok(());
                }
                let mut update = self.transition_update(id, to);
                if to == JobState::Open {
                    // Back to open for a staged job: it is not finished, so it
                    // gets no `ended` time and goes on being swept for idleness.
                    let job = job_iri(id);
                    update.push_str(&format!(
                        " ; DELETE WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {} ?e }} }}",
                        smj("ended")
                    ));
                }
                if let Some(message) = error {
                    update.push_str(&format!(
                        " ; INSERT DATA {{ GRAPH <{JOBS_GRAPH}> {{ <{}> {} {} }} }}",
                        job_iri(id),
                        smj("error"),
                        string_literal(message)
                    ));
                }
                s.update(&update)
            })?
    }

    fn run_loop(&self, spec: &RunSpec, state: &RunState) -> Result<RunEnd> {
        let mut file = File::open(&spec.path)
            .map_err(|e| anyhow::anyhow!("cannot open {}: {e}", spec.path.display()))?;
        file.seek(SeekFrom::Start(spec.start_offset))?;
        let mut reader = BufReader::with_capacity(1 << 20, file);

        let max_lines = self
            .cfg
            .file_batch_triples
            .min(self.cfg.max_batch_triples)
            .max(1);
        let max_bytes = self.cfg.max_batch_bytes;
        let mut offset = spec.start_offset;
        let mut seq = spec.start_seq;
        // A line read but not yet sent because the batch it would join was full.
        let mut carry: Vec<u8> = Vec::new();

        loop {
            if state.cancel.load(Ordering::Relaxed) {
                return Ok(RunEnd::Stopped);
            }
            let mut chunk = std::mem::take(&mut carry);
            let mut lines = usize::from(is_triple_line(&chunk));
            while lines < max_lines {
                let mut line = Vec::new();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                if line.len() > max_bytes {
                    anyhow::bail!(
                        "a line at byte {} is {} bytes, above max_batch_bytes = {max_bytes}",
                        offset + chunk.len() as u64,
                        line.len()
                    );
                }
                if chunk.len() + line.len() > max_bytes {
                    carry = line;
                    break;
                }
                if is_triple_line(&line) {
                    lines += 1;
                }
                chunk.extend_from_slice(&line);
            }
            if chunk.is_empty() {
                return Ok(RunEnd::Done);
            }
            let eof = carry.is_empty() && reader.fill_buf()?.is_empty();
            let after = offset + chunk.len() as u64;

            let job = job_iri(&spec.id);
            let mut how = Accept {
                required: JobState::Running,
                replace: vec![Replace {
                    subject: job,
                    predicate: format!("{SMJ}checkpointBytes"),
                    graph: JOBS_GRAPH.to_string(),
                    object: typed(&after.to_string(), "long"),
                }],
                ledger: Vec::new(),
            };
            // The last batch also records the file as loaded (a staged job's
            // data is not in the target yet, so it records nothing).
            if eof && spec.mode == Mode::Direct {
                how.ledger =
                    loaded_file_entry(&spec.sha256, &spec.target, &spec.path.to_string_lossy());
            }
            self.accept(&spec.id, seq, &chunk, Some("ntriples"), how)?;
            offset = after;
            seq = seq.saturating_add(1);
            if eof {
                return Ok(RunEnd::Done);
            }
        }
    }

    /// Progress of a file job, added to `ingest_status`.
    pub(super) fn file_status(
        &self,
        m: &HashMap<String, String>,
        id: &str,
        triples: u64,
        out: &mut Value,
    ) {
        let Some(path) = m.get("filePath") else {
            return;
        };
        let parse = |k: &str| m.get(k).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        let (size, done) = (parse("fileBytes"), parse("checkpointBytes"));
        let mut file = json!({
            "path": path,
            "bytes": size,
            "bytes_read": done,
            "percent": if size == 0 { 100.0 } else { (done as f64 * 1000.0 / size as f64).round() / 10.0 },
        });
        let run = self
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned();
        if let Some(run) = run {
            let secs = run.started.elapsed().as_secs_f64().max(0.001);
            let rate = triples.saturating_sub(run.base_triples) as f64 / secs;
            file["triples_per_sec"] = json!(rate.round());
            let byte_rate = done.saturating_sub(run.base_bytes) as f64 / secs;
            if byte_rate > 0.0 {
                file["eta_secs"] = json!(((size.saturating_sub(done)) as f64 / byte_rate).ceil());
            }
        }
        out["file"] = file;
        if let Some(e) = m.get("error") {
            out["error"] = json!(e);
        }
    }
}
