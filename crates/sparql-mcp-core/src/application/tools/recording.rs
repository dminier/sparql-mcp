//! Recording import tools — bridge between Playwright sessions and the store.
//!
//! The heavy format parsing lives in `crate::mcp::recording`.
//! Here we only orchestrate: build Turtle from the recording, push via
//! SparqlStore, return MCP result.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::json;

use crate::domain::{LoadOpts, QueryResult, RdfTerm, SparqlStore};
use crate::mcp::recording;

use super::sparql::{make_tool, require_str, with_delta};

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_import_navigations_def() -> Tool {
    make_tool(
        "import_recording_navigations",
        "Import the navigations stream of a Playwright recording session into the store.",
        json!({
            "type": "object",
            "properties": {
                "session_dir": { "type": "string" }
            },
            "required": ["session_dir"]
        }),
    )
}

pub fn tool_import_network_def() -> Tool {
    make_tool(
        "import_recording_network",
        "Import the network stream of a Playwright recording session into the store.",
        json!({
            "type": "object",
            "properties": {
                "session_dir": { "type": "string" }
            },
            "required": ["session_dir"]
        }),
    )
}

pub fn tool_materialize_recording_def() -> Tool {
    make_tool(
        "materialize_recording",
        "Materialize response bodies of a recording session into a filesystem mirror.",
        json!({
            "type": "object",
            "properties": {
                "session_iri": { "type": "string" },
                "out_dir": { "type": "string" }
            },
            "required": ["session_iri"]
        }),
    )
}

// ── Implementations ───────────────────────────────────────────────────────────

pub fn import_recording_navigations(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let sp = resolve_session(args)?;
    let session_ttl = recording::build_session_turtle(&sp)
        .map_err(|e| McpError::invalid_params(format!("session.json: {e}"), None))?;
    let kg = recording::build_navigations(&sp)
        .map_err(|e| McpError::invalid_params(format!("navigations.jsonl: {e}"), None))?;
    import_kind(store, &sp, &session_ttl, kg, "navigations")
}

pub fn import_recording_network(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let sp = resolve_session(args)?;
    let session_ttl = recording::build_session_turtle(&sp)
        .map_err(|e| McpError::invalid_params(format!("session.json: {e}"), None))?;
    let kg = recording::build_network(&sp)
        .map_err(|e| McpError::invalid_params(format!("network.jsonl: {e}"), None))?;
    import_kind(store, &sp, &session_ttl, kg, "network")
}

pub fn materialize_recording(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let session_iri = require_str(args, "session_iri")?;
    let out_dir_arg = args.get("out_dir").and_then(|v| v.as_str());

    // Query body targets from the store.
    let graph_iri = format!("{session_iri}/g/network");
    let query = format!(
        "PREFIX r: <https://sparql-mcp.dev/ns/rec#>\n\
         SELECT ?url ?path ?ct WHERE {{\n\
             GRAPH <{graph_iri}> {{\n\
                 ?e r:url ?url ; r:responseBodyPath ?path .\n\
                 OPTIONAL {{ ?e r:contentType ?ct }}\n\
             }}\n\
         }}"
    );
    let result = store
        .query(&query)
        .map_err(|e| McpError::internal_error(format!("query: {e}"), None))?;
    let QueryResult::Solutions(sol) = result else {
        return Err(McpError::internal_error("expected SELECT", None));
    };

    let targets: Vec<recording::BodyTarget> = sol
        .rows
        .into_iter()
        .map(|row| {
            let url = row
                .get("url")
                .map(|t| t.as_value_str().to_string())
                .unwrap_or_default();
            let path = row
                .get("path")
                .map(|t| t.as_value_str().to_string())
                .unwrap_or_default();
            let ct = row.get("ct").and_then(|t| match t {
                RdfTerm::Literal { value, .. } => Some(value.clone()),
                _ => None,
            });
            recording::BodyTarget {
                url,
                body_path: PathBuf::from(path),
                content_type: ct,
            }
        })
        .collect();

    let out_dir = match out_dir_arg {
        Some(s) => PathBuf::from(s),
        None => {
            let q = format!(
                "PREFIX r: <https://sparql-mcp.dev/ns/rec#>\n\
                 SELECT ?d WHERE {{ <{session_iri}> r:sessionDir ?d }} LIMIT 1"
            );
            let res = store
                .query(&q)
                .map_err(|e| McpError::internal_error(format!("query: {e}"), None))?;
            let QueryResult::Solutions(mut s) = res else {
                return Err(McpError::internal_error("expected SELECT", None));
            };
            let dir = s
                .rows
                .pop()
                .and_then(|r| r.get("d").map(|t| t.as_value_str().to_string()))
                .ok_or_else(|| {
                    McpError::invalid_params(
                        "session has no r:sessionDir — pass 'out_dir' explicitly",
                        None,
                    )
                })?;
            PathBuf::from(dir).join("fs")
        }
    };

    let stats = recording::materialize(&out_dir, &targets)
        .map_err(|e| McpError::internal_error(format!("materialize: {e}"), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({
            "status": "ok",
            "out_dir": stats.out_dir.to_string_lossy(),
            "candidates": targets.len(),
            "files_written": stats.files_written,
            "overwrites": stats.overwrites,
            "skipped_no_host": stats.skipped_no_host,
            "missing_body": stats.missing_body,
        }))
        .unwrap(),
    )]))
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn resolve_session(args: &JsonObject) -> Result<recording::SessionPaths, McpError> {
    let dir = require_str(args, "session_dir")?;
    recording::resolve_session(dir)
        .map_err(|e| McpError::invalid_params(format!("session_dir: {e}"), None))
}

fn import_kind(
    store: &Arc<dyn SparqlStore>,
    sp: &recording::SessionPaths,
    session_ttl: &str,
    kg: recording::KindGraph,
    kind: &'static str,
) -> Result<CallToolResult, McpError> {
    // Clear previous data for this kind.
    let clear = format!("CLEAR SILENT GRAPH <{}>", kg.graph_iri);
    store
        .update(&clear)
        .map_err(|e| McpError::internal_error(format!("clear_graph: {e}"), None))?;

    let extra = json!({
        "session_iri": sp.session_iri,
        "graph_iri": kg.graph_iri,
        "kind": kind,
        "items": kg.count,
    });

    // Load session metadata (default graph).
    let session_bytes = session_ttl.as_bytes().to_vec();
    with_delta(store, extra, |_| {
        store
            .load_rdf(&session_bytes, LoadOpts::default())
            .map(|_| ())
            .map_err(|e| McpError::invalid_params(format!("session load: {e}"), None))?;
        store
            .load_rdf(
                kg.turtle.as_bytes(),
                LoadOpts {
                    graph_iri: Some(kg.graph_iri.clone()),
                    ..Default::default()
                },
            )
            .map(|_| ())
            .map_err(|e| McpError::invalid_params(format!("{kind} load: {e}"), None))
    })
}
