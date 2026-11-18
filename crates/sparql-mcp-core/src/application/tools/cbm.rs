//! `cbm_load_graph` tool — bridge a codebase-memory-mcp knowledge graph into
//! the SPARQL store as Turtle, in one MCP call.
//!
//! Without this tool, mirroring code into SPARQL requires dumping the cbm
//! graph through `query_graph`, reformatting JSON to Turtle in client code,
//! then calling `load_ontology` — an aller-retour that drops edge metadata
//! when the engine doesn't expose `type(r)`. Going through the existing
//! `cbm::db::load_graph` + `cbm::turtle::graph_to_turtle` pipeline keeps the
//! full edge typing and reuses the canonical vocabulary emitter.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::application::event_sink::{emit_quads, EventLimits, EventSink, QuadChange};
use crate::application::limits::{refuse, ToolError};
use crate::cbm::turtle::ExportOptions;
use crate::cbm::{db as cbm_db, turtle as cbm_turtle};
use crate::domain::{InputFormat, LoadOpts, SparqlStore};

use super::sparql::make_tool;

pub fn tool_cbm_load_graph_def() -> Tool {
    make_tool(
        "cbm_load_graph",
        "Load a codebase-memory-mcp graph into the SPARQL store as Turtle. \
         Provide either `repo_path` (absolute path of the indexed repo — the \
         cache DB is resolved under $CBM_CACHE_DIR or \
         ~/.cache/codebase-memory-mcp/<slug>.db where slug is the path with \
         '/' replaced by '-') or `cbm_db_path` (explicit SQLite file). \
         `cbm_project` disambiguates a multi-project DB. `graph_iri` targets \
         a named graph (recommended: one per ingestion, e.g. \
         urn:cbm:<repo>:<date>:<sha>). `with_source=true` slurps File contents \
         as cbm:sourceCode (capped at `max_source_bytes`, default 500_000). \
         Defaults to the active project graph. Written in chunks of 10,000 triples; \
         returns inserted_new (the triples not already present), chunks and \
         node/edge counts.",
        json!({
            "type": "object",
            "properties": {
                "repo_path":        { "type": "string", "description": "Absolute repo path; cache DB derived from it." },
                "cbm_db_path":      { "type": "string", "description": "Explicit path to the CBM SQLite file (overrides repo_path)." },
                "cbm_project":      { "type": "string", "description": "Project name inside the DB (for multi-project DBs)." },
                "graph_iri":        { "type": "string", "description": "Target named graph IRI." },
                "with_source":      { "type": "boolean", "default": false },
                "max_source_bytes": { "type": "integer", "default": 500000 }
            }
        }),
    )
}

fn cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CBM_CACHE_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".cache")
        .join("codebase-memory-mcp")
}

fn slug_from_repo_path(repo: &str) -> Result<String, McpError> {
    let canon = std::fs::canonicalize(repo)
        .map_err(|e| McpError::invalid_params(format!("repo_path '{repo}': {e}"), None))?;
    Ok(canon
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "-"))
}

fn resolve_db_path(args: &JsonObject) -> Result<PathBuf, McpError> {
    if let Some(p) = args.get("cbm_db_path").and_then(Value::as_str) {
        return Ok(PathBuf::from(p));
    }
    let repo = args
        .get("repo_path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            McpError::invalid_params("must provide either 'cbm_db_path' or 'repo_path'", None)
        })?;
    let slug = slug_from_repo_path(repo)?;
    Ok(cache_dir().join(format!("{slug}.db")))
}

/// More triples than this in one call is refused: the generated Turtle is
/// parsed in memory. Not yet configurable (`[ingest]` keys arrive in 0c).
pub const MAX_CBM_TRIPLES: usize = 1_000_000;

/// Triples written per transaction. Each chunk is its own gated write, so
/// the write gate is released between chunks and other writers (and the
/// transaction's memory) stay bounded.
pub const CBM_CHUNK_TRIPLES: usize = 10_000;

pub fn cbm_load_graph(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
    active_graph: &Arc<RwLock<String>>,
    event_sink: &dyn EventSink,
) -> Result<CallToolResult, McpError> {
    let db_path = resolve_db_path(args)?;
    if !db_path.exists() {
        return Err(McpError::invalid_params(
            format!(
                "CBM database not found at {} — has the repo been indexed via codebase-memory-mcp?",
                db_path.display()
            ),
            None,
        ));
    }

    let project_name = args.get("cbm_project").and_then(Value::as_str);
    // The target graph: the argument, or else the active project graph, so
    // the viewer and every other load agree on where unqualified data goes.
    let graph_iri = match args.get("graph_iri").and_then(Value::as_str) {
        Some(iri) => iri.to_string(),
        None => active_graph
            .read()
            .map_err(|_| McpError::internal_error("active_graph lock poisoned", None))?
            .clone(),
    };
    let with_source = args
        .get("with_source")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_source_bytes = args
        .get("max_source_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(500_000) as usize;

    let kg = cbm_db::load_graph(&db_path, project_name)
        .map_err(|e| McpError::invalid_params(format!("cbm load_graph: {e}"), None))?;
    let opts = ExportOptions {
        with_source,
        max_source_bytes,
    };
    let ttl = cbm_turtle::graph_to_turtle_with(&kg, opts);

    // Parse once, outside the write gate (pure CPU), then write in chunks.
    let load_opts = LoadOpts {
        format: InputFormat::Turtle,
        graph_iri: Some(graph_iri.clone()),
        base_iri: None,
    };
    let quads = store
        .parse_rdf(ttl.as_bytes(), &load_opts)
        .map_err(|e| McpError::invalid_params(format!("parse: {e}"), None))?;
    if quads.len() > MAX_CBM_TRIPLES {
        return Err(refuse(ToolError::LimitExceeded {
            limit_key: "max_cbm_triples".to_string(),
            limit: MAX_CBM_TRIPLES as u64,
            value: quads.len() as u64,
        }));
    }

    let mut inserted_new = 0u64;
    let mut chunks = 0u32;
    for chunk in quads.chunks(CBM_CHUNK_TRIPLES) {
        inserted_new += store
            .write_quads(chunk)
            .map_err(|e| McpError::invalid_params(format!("load_rdf: {e}"), None))?;
        chunks += 1;
    }

    // Every write to a graph reports a `StoreEvent`; a big import is split or
    // collapsed into one `Changed` by the usual event limits.
    emit_quads(
        event_sink,
        QuadChange::Inserted,
        &quads,
        EventLimits::default(),
    );

    let payload = json!({
        "status": "ok",
        // Deprecated: whole-store sizes cost a full scan. Use `inserted_new`.
        "triples_before": null,
        "triples_after": null,
        "delta": inserted_new,
        "inserted_new": inserted_new,
        "parsed": quads.len(),
        "chunks": chunks,
        "cbm_db_path": db_path.display().to_string(),
        "cbm_project": kg.project.name.clone(),
        "nodes": kg.nodes.len(),
        "edges": kg.edges.len(),
        "graph_iri": graph_iri,
    });
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
}
