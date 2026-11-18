//! load_ontology and load_ontology_file tools.

use std::path::Path;
use std::sync::{Arc, RwLock};

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::application::event_sink::{emit_quads, EventLimits, EventSink, QuadChange};
use crate::domain::{InputFormat, LoadOpts, SparqlStore};

use super::sparql::{make_tool, require_str};

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_load_ontology_def() -> Tool {
    make_tool(
        "load_ontology",
        "Parse an inline RDF payload (Turtle by default; also N-Triples / RDF-XML) \
         and insert it into the store. Optional graph_iri targets a named graph; \
         it defaults to the active project graph. Returns inserted_new (the quads \
         that were not already present) and delta.",
        json!({
            "type": "object",
            "properties": {
                "turtle": { "type": "string", "description": "RDF payload." },
                "format": {
                    "type": "string",
                    "enum": ["turtle", "ntriples", "rdfxml"],
                    "default": "turtle"
                },
                "base_iri": { "type": "string" },
                "graph_iri": {
                    "type": "string",
                    "description": "Target named graph IRI. Omit to load into the active project graph."
                }
            },
            "required": ["turtle"]
        }),
    )
}

pub fn tool_load_ontology_file_def() -> Tool {
    make_tool(
        "load_ontology_file",
        "Load an RDF file from disk. SHA-256 idempotence: re-loading the same file \
         is a no-op unless force=true. Optional graph_iri targets a named graph.",
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Filesystem path to the RDF file." },
                "format": { "type": "string", "enum": ["turtle", "ntriples", "rdfxml"] },
                "base_iri": { "type": "string" },
                "graph_iri": { "type": "string" },
                "force": {
                    "type": "boolean",
                    "default": false,
                    "description": "Reload even if the file was already loaded (same SHA-256)."
                }
            },
            "required": ["path"]
        }),
    )
}

// ── Implementations ───────────────────────────────────────────────────────────

pub fn load_ontology(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
    active_graph: &Arc<RwLock<String>>,
    event_sink: &dyn EventSink,
) -> Result<CallToolResult, McpError> {
    let turtle = require_str(args, "turtle")?;
    let format_label = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("turtle");
    let format = parse_format(format_label)?;
    let graph_iri = target_graph(args, active_graph)?;

    let opts = LoadOpts {
        format,
        graph_iri: Some(graph_iri.clone()),
        base_iri: args
            .get("base_iri")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    // The load is atomic and costs no whole-store scan: it reports how many
    // of the parsed quads were new.
    let (inserted_new, triples) = store
        .load_rdf_with_triples(turtle.as_bytes(), opts)
        .map_err(|e| McpError::invalid_params(format!("parse/load error: {e}"), None))?;

    emit_quads(
        event_sink,
        QuadChange::Inserted,
        &triples,
        EventLimits::default(),
    );

    let payload = json!({
        "status": "ok",
        // Deprecated: whole-store sizes cost a full scan, so they are no
        // longer computed. Use `inserted_new` / `delta`.
        "triples_before": null,
        "triples_after": null,
        "delta": inserted_new,
        "inserted_new": inserted_new,
        "parsed": triples.len(),
        "format": format_label,
        "graph_iri": graph_iri,
    });
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
}

pub fn load_ontology_file(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
    active_graph: &Arc<RwLock<String>>,
    event_sink: &dyn EventSink,
) -> Result<CallToolResult, McpError> {
    let path_str = require_str(args, "path")?;
    let format = args
        .get("format")
        .and_then(Value::as_str)
        .and_then(InputFormat::from_label)
        .unwrap_or_else(|| InputFormat::guess_from_path(path_str));
    let graph_iri = target_graph(args, active_graph)?;
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);

    let opts = LoadOpts {
        format,
        graph_iri: Some(graph_iri.clone()),
        base_iri: args
            .get("base_iri")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    let result = store
        .load_rdf_file(Path::new(path_str), opts, force)
        .map_err(|e| McpError::invalid_params(format!("load file: {e}"), None))?;

    if !result.skipped {
        emit_quads(
            event_sink,
            QuadChange::Inserted,
            &result.triples,
            EventLimits::default(),
        );
    }

    let mut payload = json!({
        "status": "ok",
        "path": result.path,
        "format": result.format.as_label(),
        "bytes_read": result.bytes_read,
        "sha256": result.sha256,
        // Deprecated, see `load_ontology`.
        "triples_before": null,
        "triples_after": null,
        "delta": result.delta(),
        "inserted_new": result.inserted_new,
        "graph_iri": graph_iri,
    });
    if result.skipped {
        payload["skipped"] = json!(true);
        payload["reason"] = json!("file already loaded (same SHA-256)");
    }

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
}

/// The graph a load goes into: the `graph_iri` argument, or else the active
/// project graph (as the CLI `load-file` does). Loading into the default
/// graph, which no viewer shows, is no longer the silent fallback.
fn target_graph(args: &JsonObject, active_graph: &Arc<RwLock<String>>) -> Result<String, McpError> {
    if let Some(iri) = args.get("graph_iri").and_then(Value::as_str) {
        return Ok(iri.to_string());
    }
    Ok(active_graph
        .read()
        .map_err(|_| McpError::internal_error("active_graph lock poisoned", None))?
        .clone())
}

fn parse_format(label: &str) -> Result<InputFormat, McpError> {
    InputFormat::from_label(label).ok_or_else(|| {
        McpError::invalid_params(
            format!("unsupported format '{label}' (expected turtle | ntriples | rdfxml)"),
            None,
        )
    })
}
