//! load_ontology and load_ontology_file tools.

use std::path::Path;
use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::domain::{InputFormat, LoadOpts, SparqlStore};

use super::sparql::{make_tool, require_str, with_delta};

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_load_ontology_def() -> Tool {
    make_tool(
        "load_ontology",
        "Parse an inline RDF payload (Turtle by default; also N-Triples / RDF-XML) \
         and insert it into the store. Optional graph_iri targets a named graph. \
         Returns triple counts + delta.",
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
                    "description": "Target named graph IRI. Omit to load into the default graph."
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
) -> Result<CallToolResult, McpError> {
    let turtle = require_str(args, "turtle")?;
    let format_label = args
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("turtle");
    let format = parse_format(format_label)?;
    let graph_iri = args.get("graph_iri").and_then(Value::as_str);

    let mut extra = json!({ "format": format_label });
    if let Some(iri) = graph_iri {
        extra["graph_iri"] = json!(iri);
    }

    let opts = LoadOpts {
        format,
        graph_iri: graph_iri.map(str::to_string),
        base_iri: args
            .get("base_iri")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let content = turtle.as_bytes().to_vec();

    with_delta(store, extra, |_| {
        store
            .load_rdf(&content, opts)
            .map(|_| ())
            .map_err(|e| McpError::invalid_params(format!("parse/load error: {e}"), None))
    })
}

pub fn load_ontology_file(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let path_str = require_str(args, "path")?;
    let format = args
        .get("format")
        .and_then(Value::as_str)
        .and_then(InputFormat::from_label)
        .unwrap_or_else(|| InputFormat::guess_from_path(path_str));
    let graph_iri = args.get("graph_iri").and_then(Value::as_str);
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);

    let opts = LoadOpts {
        format,
        graph_iri: graph_iri.map(str::to_string),
        base_iri: args
            .get("base_iri")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    let result = store
        .load_rdf_file(Path::new(path_str), opts, force)
        .map_err(|e| McpError::invalid_params(format!("load file: {e}"), None))?;

    let mut payload = json!({
        "status": "ok",
        "path": result.path,
        "format": result.format.as_label(),
        "bytes_read": result.bytes_read,
        "sha256": result.sha256,
        "triples_before": result.triples_before,
        "triples_after": result.triples_after,
        "delta": result.delta(),
    });
    if result.skipped {
        payload["skipped"] = json!(true);
        payload["reason"] = json!("file already loaded (same SHA-256)");
    }
    if let Some(iri) = graph_iri {
        payload["graph_iri"] = json!(iri);
    }

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
}

fn parse_format(label: &str) -> Result<InputFormat, McpError> {
    InputFormat::from_label(label).ok_or_else(|| {
        McpError::invalid_params(
            format!("unsupported format '{label}' (expected turtle | ntriples | rdfxml)"),
            None,
        )
    })
}
