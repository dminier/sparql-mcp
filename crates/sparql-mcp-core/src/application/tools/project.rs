//! project_create, project_list, project_switch tools.
//!
//! Projects are `smc:Project` instances in the `<urn:meta>` named graph.
//! Each project owns a named graph `<urn:project:ID>`.

use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::json;

use crate::domain::{LoadOpts, QueryResult, RdfTerm, SparqlStore};

use super::sparql::{make_tool, require_str};

const META_GRAPH: &str = "urn:meta";
const SMC_NS: &str = "https://sparql-mcp.dev/ns#";

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_project_create_def() -> Tool {
    make_tool(
        "project_create",
        "Create a new smc:Project. Allocates <urn:project:ID> named graph and records \
         metadata (id, label, created) in <urn:meta>.",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Short slug (alphanumerics, hyphens, underscores)."
                },
                "label": { "type": "string", "description": "Human-readable project name." }
            },
            "required": ["id", "label"]
        }),
    )
}

pub fn tool_project_list_def() -> Tool {
    make_tool(
        "project_list",
        "List all smc:Project instances recorded in <urn:meta>.",
        json!({ "type": "object", "properties": {} }),
    )
}

pub fn tool_project_switch_def() -> Tool {
    make_tool(
        "project_switch",
        "Set the active project by ID. Returns the active named graph IRI to use \
         in subsequent SPARQL queries.",
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" }
            },
            "required": ["id"]
        }),
    )
}

// ── Implementations ───────────────────────────────────────────────────────────

pub fn project_create(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let id = require_str(args, "id")?;
    let label = require_str(args, "label")?;

    validate_id(id)?;

    let project_iri = format!("urn:project:{id}");
    let now = iso_now();
    let turtle = format!(
        "@prefix smc: <{SMC_NS}> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         <{project_iri}> a smc:Project ;\n\
             smc:projectId \"{id}\" ;\n\
             smc:label \"{label}\" ;\n\
             smc:namedGraph <{project_iri}> ;\n\
             smc:created \"{now}\"^^xsd:dateTime .\n"
    );

    let opts = LoadOpts {
        graph_iri: Some(META_GRAPH.to_string()),
        ..Default::default()
    };
    store
        .load_rdf(turtle.as_bytes(), opts)
        .map_err(|e| McpError::internal_error(format!("create project: {e}"), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({
            "status": "ok",
            "id": id,
            "label": label,
            "named_graph": project_iri,
            "created": now,
        }))
        .unwrap(),
    )]))
}

pub fn project_list(store: &Arc<dyn SparqlStore>) -> Result<CallToolResult, McpError> {
    let sparql = format!(
        "PREFIX smc: <{SMC_NS}>\n\
         SELECT ?id ?label ?graph ?created WHERE {{\n\
             GRAPH <{META_GRAPH}> {{\n\
                 ?p a smc:Project ;\n\
                    smc:projectId ?id ;\n\
                    smc:label ?label .\n\
                 OPTIONAL {{ ?p smc:namedGraph ?graph }}\n\
                 OPTIONAL {{ ?p smc:created ?created }}\n\
             }}\n\
         }} ORDER BY ?id"
    );

    let result = store
        .query(&sparql)
        .map_err(|e| McpError::internal_error(format!("list projects: {e}"), None))?;

    let QueryResult::Solutions(sol) = result else {
        return Err(McpError::internal_error("expected SELECT", None));
    };

    let mut projects = Vec::new();
    for row in sol.rows {
        let id = str_cell(&row, "id");
        let graph_iri = iri_cell(&row, "graph").unwrap_or_else(|| format!("urn:project:{id}"));
        projects.push(json!({
            "id": id,
            "label": str_cell(&row, "label"),
            "named_graph": graph_iri,
            "created": str_cell(&row, "created"),
        }));
    }

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({ "projects": projects })).unwrap(),
    )]))
}

pub fn project_switch(
    _store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let id = require_str(args, "id")?;
    validate_id(id)?;
    let graph_iri = format!("urn:project:{id}");
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({
            "status": "ok",
            "active_project": id,
            "active_graph": graph_iri,
        }))
        .unwrap(),
    )]))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn validate_id(id: &str) -> Result<(), McpError> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(McpError::invalid_params(
            "project id must contain only alphanumerics, hyphens, or underscores",
            None,
        ));
    }
    Ok(())
}

fn str_cell(row: &std::collections::HashMap<String, RdfTerm>, var: &str) -> String {
    row.get(var)
        .map(|t| t.as_value_str().to_string())
        .unwrap_or_default()
}

fn iri_cell(row: &std::collections::HashMap<String, RdfTerm>, var: &str) -> Option<String> {
    row.get(var).and_then(|t| match t {
        RdfTerm::Iri(s) => Some(s.clone()),
        _ => None,
    })
}

fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let (y, m, d) = epoch_ymd(secs / 86400);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn epoch_ymd(days: u64) -> (u64, u64, u64) {
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
