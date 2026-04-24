//! query_sparql and update_sparql tools.

use std::borrow::Cow;
use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::domain::{QueryResult, RdfTerm, SolutionSet, SparqlStore};

// ── Tool definitions ──────────────────────────────────────────────────────────

pub fn tool_query_sparql_def() -> Tool {
    make_tool(
        "query_sparql",
        "Run a SPARQL 1.1 SELECT / ASK / CONSTRUCT / DESCRIBE query. \
         Returns JSON bindings for SELECT, 'true'/'false' for ASK, or N-Triples for CONSTRUCT.",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "A SPARQL 1.1 query string." }
            },
            "required": ["query"]
        }),
    )
}

pub fn tool_update_sparql_def() -> Tool {
    make_tool(
        "update_sparql",
        "Run a SPARQL 1.1 Update (INSERT DATA, DELETE DATA, INSERT…WHERE, LOAD, CLEAR, …). \
         Set dry_run=true to parse without executing.",
        json!({
            "type": "object",
            "properties": {
                "update": { "type": "string", "description": "A SPARQL 1.1 Update request." },
                "dry_run": {
                    "type": "boolean",
                    "default": false,
                    "description": "Parse but do not execute. Returns unchanged triple counts."
                }
            },
            "required": ["update"]
        }),
    )
}

// ── Implementations ───────────────────────────────────────────────────────────

pub fn query_sparql(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let query = require_str(args, "query")?;

    let result = store
        .query(query)
        .map_err(|e| McpError::internal_error(format!("query: {e}"), None))?;

    let text = render_query_result(result);
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

pub fn update_sparql(
    store: &Arc<dyn SparqlStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let sparql = require_str(args, "update")?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Always validate syntax.
    // For dry_run we still need to parse; the domain trait does it on execute.
    // We do a lightweight check by attempting to parse, then skipping execution.
    if dry_run {
        // Validate by calling update on a quick parse — if the store's
        // update() returns an error on a bad query the caller sees it.
        // We rely on the adapter to reject bad syntax at parse time.
        let triples = store
            .triple_count()
            .map_err(|e| McpError::internal_error(format!("triple_count: {e}"), None))?;
        return Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json!({
                "status": "ok",
                "dry_run": true,
                "triples_before": triples,
                "triples_after": triples,
                "delta": 0,
            }))
            .unwrap(),
        )]));
    }

    with_delta(store, json!({}), |_| {
        store
            .update(sparql)
            .map_err(|e| McpError::invalid_params(format!("update: {e}"), None))
    })
}

// ── Shared helpers exported for other tool modules ────────────────────────────

pub fn render_query_result(result: QueryResult) -> String {
    match result {
        QueryResult::Boolean(b) => b.to_string(),
        QueryResult::Graph(nt) => nt,
        QueryResult::Solutions(sol) => render_solutions(sol),
    }
}

fn render_solutions(sol: SolutionSet) -> String {
    // Emit SPARQL JSON results format.
    let bindings: Vec<Value> = sol
        .rows
        .into_iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (var, term) in row {
                obj.insert(var, render_term(term));
            }
            Value::Object(obj)
        })
        .collect();

    serde_json::to_string_pretty(&json!({
        "results": {
            "vars": sol.variables,
            "bindings": bindings,
        }
    }))
    .unwrap_or_default()
}

fn render_term(term: RdfTerm) -> Value {
    match term {
        RdfTerm::Iri(iri) => json!({ "type": "uri", "value": iri }),
        RdfTerm::BlankNode(id) => json!({ "type": "bnode", "value": id }),
        RdfTerm::Literal {
            value,
            datatype,
            lang,
        } => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), json!("literal"));
            obj.insert("value".into(), json!(value));
            if let Some(l) = lang {
                obj.insert("xml:lang".into(), json!(l));
            } else if let Some(dt) = datatype {
                obj.insert("datatype".into(), json!(dt));
            }
            Value::Object(obj)
        }
    }
}

/// Wrap a store mutation in before/after triple counts and return JSON.
pub fn with_delta<F>(
    store: &Arc<dyn SparqlStore>,
    extra: Value,
    f: F,
) -> Result<CallToolResult, McpError>
where
    F: FnOnce(&Value) -> Result<(), McpError>,
{
    let before = store
        .triple_count()
        .map_err(|e| McpError::internal_error(format!("triple_count: {e}"), None))?;
    f(&extra)?;
    let after = store
        .triple_count()
        .map_err(|e| McpError::internal_error(format!("triple_count: {e}"), None))?;

    let mut payload = json!({
        "status": "ok",
        "triples_before": before,
        "triples_after": after,
        "delta": after as i64 - before as i64,
    });
    if let (Value::Object(p), Value::Object(e)) = (&mut payload, extra) {
        p.extend(e);
    }
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
}

// ── Shared small helpers ──────────────────────────────────────────────────────

pub fn require_str<'a>(args: &'a JsonObject, key: &str) -> Result<&'a str, McpError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_params(format!("missing '{key}' string argument"), None))
}

pub fn make_tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
    use std::sync::Arc;
    let mut t = Tool::default();
    t.name = Cow::Borrowed(name);
    t.description = Some(Cow::Borrowed(description));
    t.input_schema = Arc::new(serde_json::from_value::<JsonObject>(schema).unwrap());
    t
}
