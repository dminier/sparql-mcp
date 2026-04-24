//! export_graph, list_graphs, and stats tools.

use std::path::Path;
use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::json;

use crate::domain::SparqlStore;

use super::sparql::make_tool;

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_export_graph_def() -> Tool {
    make_tool(
        "export_graph",
        "Dump a named graph (or the default graph) to a Turtle file on disk. \
         Defaults to output/snapshots/<UTC-timestamp>.ttl.",
        json!({
            "type": "object",
            "properties": {
                "graph_iri": {
                    "type": "string",
                    "description": "IRI of the named graph to export. Omit for the default graph."
                },
                "path": {
                    "type": "string",
                    "description": "Destination file path. Defaults to output/snapshots/<ts>.ttl."
                }
            }
        }),
    )
}

pub fn tool_list_graphs_def() -> Tool {
    make_tool(
        "list_graphs",
        "List all named graphs present in the store, sorted by IRI. Returns {graphs:[…], count:N}.",
        json!({ "type": "object", "properties": {} }),
    )
}

pub fn tool_stats_def() -> Tool {
    make_tool(
        "stats",
        "Return summary statistics about the store.",
        json!({ "type": "object", "properties": {} }),
    )
}

// ── Implementations ───────────────────────────────────────────────────────────

pub fn export_graph(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let graph_iri = args.get("graph_iri").and_then(|v| v.as_str());
    let path_str = args.get("path").and_then(|v| v.as_str());

    let dest = match path_str {
        Some(p) => std::path::PathBuf::from(p),
        None => std::path::PathBuf::from(format!("output/snapshots/{}.ttl", chrono_ts())),
    };

    let bytes = store
        .export_graph(graph_iri, &dest)
        .map_err(|e| McpError::internal_error(format!("export: {e}"), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({
            "status": "ok",
            "path": dest.to_string_lossy(),
            "bytes": bytes,
            "graph_iri": graph_iri,
        }))
        .unwrap(),
    )]))
}

pub fn list_graphs(store: &Arc<dyn SparqlStore>) -> Result<CallToolResult, McpError> {
    let graphs = store
        .list_graphs()
        .map_err(|e| McpError::internal_error(format!("list_graphs: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({ "graphs": graphs, "count": graphs.len() })).unwrap(),
    )]))
}

pub fn stats(
    store: &Arc<dyn SparqlStore>,
    ontology_dir: &Path,
) -> Result<CallToolResult, McpError> {
    let triples = store
        .triple_count()
        .map_err(|e| McpError::internal_error(format!("triple_count: {e}"), None))?;
    let files = count_ttl_files(ontology_dir).unwrap_or(0);
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({
            "triples": triples,
            "ontology_dir": ontology_dir.to_string_lossy(),
            "ontology_files": files,
        }))
        .unwrap(),
    )]))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn count_ttl_files(dir: &Path) -> anyhow::Result<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    Ok(std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("ttl"))
        .count())
}

fn chrono_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let (y, m, d) = epoch_days_to_ymd(secs / 86400);
    format!("{y:04}{m:02}{d:02}T{hour:02}{min:02}{sec:02}Z")
}

fn epoch_days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
