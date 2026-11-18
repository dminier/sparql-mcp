//! `ingest_begin`, `ingest_status` and `ingest_abort`: the job lifecycle
//! tools (`docs/designs/sparql-mcp-batch-ingestion.md`).
//!
//! A refusal (`job_not_found`, `too_many_open_jobs`, `invalid_target`, …) is
//! raised by the service as a `ToolError::Coded`, which the dispatcher turns
//! into an `isError` result with a JSON payload. Arguments of the wrong type
//! are `invalid_params`, as for every tool.

use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::application::ingest::{
    BatchRequest, BeginRequest, Check, CommitRequest, FileRequest, IngestService, Mode,
    ValidateRequest,
};
use crate::application::limits::ToolError;

use super::sparql::{make_tool, require_str};

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_ingest_begin_def() -> Tool {
    make_tool(
        "ingest_begin",
        "Start a batch ingestion job. Returns a job_id and the limits to size batches by \
         (max_batch_bytes, max_batch_triples, ...). mode 'staged' (default) writes batches to a \
         staging graph that ingest_commit moves into the target in one step; mode 'direct' \
         writes each batch to the target at once, for large append-only loads. The target is \
         the active project graph unless target_graph names a project created with \
         project_create. Send batches with the same seq to retry safely.",
        json!({
            "type": "object",
            "properties": {
                "target_graph": { "type": "string", "description": "Graph IRI. Defaults to the active project graph." },
                "mode": { "type": "string", "enum": ["staged", "direct"], "default": "staged" },
                "replace": { "type": "boolean", "default": false, "description": "Staged only: the commit replaces the target graph's contents." },
                "format": { "type": "string", "enum": ["ntriples", "turtle"], "default": "ntriples", "description": "N-Triples is recommended: no prefix state across batches." },
                "expected_batches": { "type": "integer", "minimum": 1, "description": "If given, the commit requires every seq from 1 to this number." },
                "label": { "type": "string", "maxLength": 200 },
                "blank_nodes": { "type": "string", "enum": ["skolemize", "reject"], "default": "skolemize" }
            }
        }),
    )
}

pub fn tool_ingest_batch_def() -> Tool {
    make_tool(
        "ingest_batch",
        "Send one batch of an ingestion job. Number batches from 1 with `seq`. Re-sending the \
         same seq with the same data after a timeout or error is always safe: a batch that \
         already landed returns duplicate:true with its original counts, and the data is stored \
         once. The same seq with different data is refused (seq_conflict). A batch with a syntax \
         error, a reserved IRI or an oversized term is refused as a whole and nothing is \
         written; fix it and re-send the same seq. Blank nodes written as _:x are the same node \
         in every batch of the job. Keep batches within the limits returned by ingest_begin \
         (about 2,000-5,000 triples, at most 512 KiB, is a good size).",
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" },
                "seq": { "type": "integer", "minimum": 1 },
                "data": { "type": "string", "description": "The RDF payload, in the job's format." },
                "format": { "type": "string", "enum": ["ntriples", "turtle"], "description": "Overrides the job's format for this batch." }
            },
            "required": ["job_id", "seq", "data"]
        }),
    )
}

pub fn tool_ingest_commit_def() -> Tool {
    make_tool(
        "ingest_commit",
        "Close an ingestion job. For a staged job, the staged triples are moved into the target \
         graph in one transaction (with replace, the target's previous contents are replaced); \
         before this call nothing of the job is visible. A direct job's data is already in the \
         target, so the commit only closes it. The commit refuses a job with missing batches \
         (every seq from 1 to expected_batches, or to the highest seq sent, must be present). \
         Returns the totals, the target graph's size, and for a staged job new_subjects: the IRIs \
         of subjects that were not in the target before (up to 5,000), which can be passed to \
         viewer_show. A committed job cannot receive more batches.",
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" },
                "require_validation": { "type": "boolean", "default": false, "description": "Refuse unless the last ingest_validate passed and no batch arrived since." },
                "allow_empty_replace": { "type": "boolean", "default": false, "description": "A replace with an empty staging graph empties the target; this must be set to allow it." }
            },
            "required": ["job_id"]
        }),
    )
}

pub fn tool_ingest_status_def() -> Tool {
    make_tool(
        "ingest_status",
        "Status of one ingestion job (state, received batch numbers as ranges, triples, \
         timestamps), or, without job_id, the jobs that are not finished. Use it after a \
         restart to see which batches to re-send.",
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" }
            }
        }),
    )
}

pub fn tool_ingest_abort_def() -> Tool {
    make_tool(
        "ingest_abort",
        "Abort an open, interrupted or failed ingestion job. A staged job's staging graph is \
         dropped. A direct job's accepted batches stay in the target graph; the reply lists \
         their seqs.",
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" }
            },
            "required": ["job_id"]
        }),
    )
}

pub fn tool_ingest_validate_def() -> Tool {
    make_tool(
        "ingest_validate",
        "Check a staged job's data before committing it. Each check is a named read-only SELECT \
         or ASK in which {{staging}} and {{target}} stand for the job's two graphs (angle \
         brackets included), e.g. SELECT ?s WHERE { GRAPH {{staging}} { ?s a ?c } FILTER NOT \
         EXISTS { GRAPH {{target}} { ?c a ?k } } } LIMIT 20. `expect` is empty (default for a \
         SELECT: it looks for violations) or non_empty; true (default) or false for an ASK. \
         Up to 16 checks of 16 KiB; SERVICE and other query forms are refused (not_a_query). \
         Each check is cancelled after validate_timeout_secs (validate_timeout). Returns \
         {passed, results:[{name, passed, expect, rows (at most 20) or answer}], recorded}. \
         The outcome is recorded for ingest_commit with require_validation, unless a batch \
         arrived while the checks ran (recorded: false). A direct job has nothing to validate \
         (wrong_mode).",
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" },
                "checks": {
                    "type": "array",
                    "maxItems": 16,
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "query": { "type": "string" },
                            "expect": { "type": "string", "enum": ["empty", "non_empty", "true", "false"] }
                        },
                        "required": ["name", "query"]
                    }
                }
            },
            "required": ["job_id", "checks"]
        }),
    )
}

pub fn tool_ingest_file_def() -> Tool {
    make_tool(
        "ingest_file",
        "Load a big N-Triples file that is on the server's disk (inside [ingest] file_roots, which \
         must be set), without sending it through the conversation. The file is hashed, then \
         streamed in the background in batches; this call returns a job_id at once. Follow it with \
         ingest_status (bytes_read, percent, triples_per_sec, eta_secs). mode direct (default) \
         writes the target as it goes and the job ends committed; mode staged (small files only) \
         ends open, then use ingest_validate and ingest_commit. A file already loaded into the \
         same graph is refused (already_loaded) unless force. If the server restarted, the job is \
         interrupted: call ingest_file again with resume_job (and the same path) to continue \
         from its checkpoint; a changed file is refused (source_changed). ingest_abort on a \
         running job stops it after its current batch. Only N-Triples (.nt) is streamed.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path of the .nt file, inside file_roots." },
                "target_graph": { "type": "string", "description": "The active graph or a created project. Default: the active graph." },
                "mode": { "type": "string", "enum": ["direct", "staged"], "description": "Default direct." },
                "format": { "type": "string", "enum": ["ntriples"] },
                "resume_job": { "type": "string", "description": "The interrupted job to continue." },
                "force": { "type": "boolean", "description": "Load even if this file was already loaded into the graph." }
            },
            "required": ["path"]
        }),
    )
}

// ── Implementations ───────────────────────────────────────────────────────────

/// An optional string argument: absent is `None`, a non-string is an error.
fn opt_str<'a>(args: &'a JsonObject, key: &str) -> Result<Option<&'a str>, McpError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(McpError::invalid_params(
            format!("'{key}' must be a string"),
            None,
        )),
    }
}

fn opt_bool(args: &JsonObject, key: &str) -> Result<Option<bool>, McpError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(McpError::invalid_params(
            format!("'{key}' must be a boolean"),
            None,
        )),
    }
}

fn reply(result: anyhow::Result<Value>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(v) => Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&v).unwrap_or_default(),
        )])),
        // The refusal was recorded for the dispatcher, which turns it into an
        // `isError` result with its JSON payload.
        Err(e) if e.downcast_ref::<ToolError>().is_some() => {
            Err(McpError::invalid_params(e.to_string(), None))
        }
        Err(e) => Err(McpError::internal_error(format!("ingest: {e}"), None)),
    }
}

pub fn ingest_begin(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    let mode = match opt_str(args, "mode")? {
        None => Mode::Staged,
        Some(s) => Mode::parse(s)
            .ok_or_else(|| McpError::invalid_params("mode must be 'staged' or 'direct'", None))?,
    };
    let expected_batches = match args.get("expected_batches") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    McpError::invalid_params("'expected_batches' must be a positive integer", None)
                })?,
        ),
    };
    let request = BeginRequest {
        target_graph: opt_str(args, "target_graph")?.map(str::to_string),
        mode,
        replace: opt_bool(args, "replace")?.unwrap_or(false),
        format: opt_str(args, "format")?.unwrap_or("ntriples").to_string(),
        expected_batches,
        label: opt_str(args, "label")?.map(str::to_string),
        blank_nodes: opt_str(args, "blank_nodes")?
            .unwrap_or("skolemize")
            .to_string(),
        file: None,
    };
    reply(svc.begin(&request))
}

pub fn ingest_batch(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    let seq = args
        .get("seq")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| McpError::invalid_params("'seq' must be a positive integer", None))?;
    let request = BatchRequest {
        job_id: require_str(args, "job_id")?.to_string(),
        seq,
        data: require_str(args, "data")?.to_string(),
        format: opt_str(args, "format")?.map(str::to_string),
    };
    reply(svc.batch(&request))
}

pub fn ingest_commit(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    let request = CommitRequest {
        job_id: require_str(args, "job_id")?.to_string(),
        require_validation: opt_bool(args, "require_validation")?.unwrap_or(false),
        allow_empty_replace: opt_bool(args, "allow_empty_replace")?.unwrap_or(false),
    };
    reply(svc.commit(&request))
}

pub fn ingest_status(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    reply(svc.status(opt_str(args, "job_id")?))
}

pub fn ingest_abort(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    reply(svc.abort(require_str(args, "job_id")?))
}

pub fn ingest_file(
    svc: &Arc<IngestService>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let mode = match opt_str(args, "mode")? {
        None => Mode::Direct,
        Some(s) => Mode::parse(s)
            .ok_or_else(|| McpError::invalid_params("mode must be 'staged' or 'direct'", None))?,
    };
    reply(svc.start_file(&FileRequest {
        path: require_str(args, "path")?.to_string(),
        target_graph: opt_str(args, "target_graph")?.map(str::to_string),
        mode,
        format: opt_str(args, "format")?.map(str::to_string),
        resume_job: opt_str(args, "resume_job")?.map(str::to_string),
        force: opt_bool(args, "force")?.unwrap_or(false),
    }))
}

pub fn ingest_validate(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    let checks = args
        .get("checks")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::invalid_params("'checks' must be an array", None))?
        .iter()
        .map(|c| {
            let field = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_string);
            Ok(Check {
                name: field("name").ok_or_else(|| {
                    McpError::invalid_params("each check needs a string 'name'", None)
                })?,
                query: field("query").ok_or_else(|| {
                    McpError::invalid_params("each check needs a string 'query'", None)
                })?,
                expect: field("expect"),
            })
        })
        .collect::<Result<Vec<_>, McpError>>()?;
    reply(svc.validate(&ValidateRequest {
        job_id: require_str(args, "job_id")?.to_string(),
        checks,
    }))
}
