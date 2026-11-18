//! `ingest_begin`, `ingest_status` and `ingest_abort`: the job lifecycle
//! tools (`docs/designs/sparql-mcp-batch-ingestion.md`).
//!
//! A refusal (`job_not_found`, `too_many_open_jobs`, `invalid_target`, …) is
//! raised by the service as a `ToolError::Coded`, which the dispatcher turns
//! into an `isError` result with a JSON payload. Arguments of the wrong type
//! are `invalid_params`, as for every tool.

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::application::ingest::{BatchRequest, BeginRequest, IngestService, Mode};
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

pub fn ingest_status(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    reply(svc.status(opt_str(args, "job_id")?))
}

pub fn ingest_abort(svc: &IngestService, args: &JsonObject) -> Result<CallToolResult, McpError> {
    reply(svc.abort(require_str(args, "job_id")?))
}
