//! query_sparql and update_sparql tools.

use std::borrow::Cow;
use std::sync::{Arc, RwLock};

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};

use crate::application::event_sink::{emit_quads, EventLimits, EventSink, QuadChange, StoreEvent};
use crate::application::guards::classify_sparql;
use crate::application::limits::{refuse, take_captured, ToolError};
use crate::domain::{QueryResult, RdfTerm, SolutionSet, SparqlStore, UpdateOutcome};

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
    active_graph: &Arc<RwLock<String>>,
    event_sink: &dyn EventSink,
) -> Result<CallToolResult, McpError> {
    let sparql = require_str(args, "update")?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if dry_run {
        // Parse and classify without running anything. A syntax error is a
        // `parse_error`; a request this handle would refuse is reported under
        // `refused` instead of being run.
        let class = classify_sparql(sparql).map_err(refuse)?;
        let refused = match store.check_update(sparql) {
            Ok(()) => None,
            Err(e) => {
                // The refusal is reported as data here, not raised.
                let _ = take_captured();
                Some(
                    e.downcast_ref::<ToolError>()
                        .map(ToolError::to_json)
                        .unwrap_or_else(|| json!({ "code": "error", "message": e.to_string() })),
                )
            }
        };
        let graphs: Vec<&str> = class.graphs.iter().map(String::as_str).collect();
        return Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&json!({
                "status": "ok",
                "dry_run": true,
                "operations": class.operations,
                // "" is the default graph.
                "graphs": graphs,
                "all_graphs": class.all_graphs,
                "destructive": class.destructive,
                "refused": refused,
            }))
            .unwrap(),
        )]));
    }

    let outcome = store
        .update_with_outcome(sparql)
        .map_err(|e| McpError::invalid_params(format!("update: {e}"), None))?;

    let graph = active_graph
        .read()
        .map_err(|_| McpError::internal_error("active_graph lock poisoned", None))?
        .clone();

    // This is an explicit, documented limitation, not a silent gap:
    // pattern-based updates do not get node-level detail in the viewer in
    // v1 — only ground INSERT DATA / DELETE DATA produce a precise event.
    // Ground events name the graph each quad actually went to, which is not
    // necessarily the active one.
    match &outcome {
        UpdateOutcome::Ground {
            inserted, deleted, ..
        } => {
            let limits = EventLimits::default();
            emit_quads(event_sink, QuadChange::Inserted, inserted, limits);
            emit_quads(event_sink, QuadChange::Deleted, deleted, limits);
        }
        UpdateOutcome::Pattern {
            triples_before,
            triples_after,
        } => {
            event_sink.emit(StoreEvent::Changed {
                graph,
                triples_before: *triples_before,
                triples_after: *triples_after,
            });
        }
    }

    // `triples_before` / `triples_after` are whole-store sizes: only a
    // pattern update has them (it already scanned), and they are `null` for a
    // ground update, which no longer scans. `delta` is exact in both cases.
    let mut payload = json!({
        "status": "ok",
        "triples_before": outcome.triples_before(),
        "triples_after": outcome.triples_after(),
        "delta": outcome.delta(),
    });
    if let UpdateOutcome::Ground {
        inserted_new,
        removed_existing,
        ..
    } = &outcome
    {
        payload["inserted_new"] = json!(inserted_new);
        payload["removed_existing"] = json!(removed_existing);
    }
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
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
