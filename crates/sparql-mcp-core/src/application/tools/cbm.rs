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
use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::cbm::turtle::ExportOptions;
use crate::cbm::{db as cbm_db, turtle as cbm_turtle};
use crate::domain::{InputFormat, LoadOpts, SparqlStore};

use super::sparql::{make_tool, with_delta};

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
         Returns triple counts + delta + node/edge counts.",
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
            McpError::invalid_params(
                "must provide either 'cbm_db_path' or 'repo_path'",
                None,
            )
        })?;
    let slug = slug_from_repo_path(repo)?;
    Ok(cache_dir().join(format!("{slug}.db")))
}

pub fn cbm_load_graph(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
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
    let graph_iri = args
        .get("graph_iri")
        .and_then(Value::as_str)
        .map(str::to_string);
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

    let mut extra = json!({
        "cbm_db_path": db_path.display().to_string(),
        "cbm_project": kg.project.name.clone(),
        "nodes": kg.nodes.len(),
        "edges": kg.edges.len(),
    });
    if let Some(ref iri) = graph_iri {
        extra["graph_iri"] = json!(iri);
    }

    let load_opts = LoadOpts {
        format: InputFormat::Turtle,
        graph_iri,
        base_iri: None,
    };

    with_delta(store, extra, move |_| {
        store
            .load_rdf(ttl.as_bytes(), load_opts)
            .map(|_| ())
            .map_err(|e| McpError::invalid_params(format!("load_rdf: {e}"), None))
    })
}
