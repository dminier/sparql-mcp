//! Integration tests — query_sparql / update_sparql application tools.

use std::sync::Arc;

use serde_json::json;
use sparql_mcp::application::tools::sparql::{query_sparql, update_sparql};
use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::OxigraphAdapter;

fn store() -> Arc<dyn SparqlStore> {
    Arc::new(OxigraphAdapter::open_in_memory().unwrap())
}

fn args(val: serde_json::Value) -> rmcp::model::JsonObject {
    val.as_object().unwrap().clone()
}

// ── query_sparql ──────────────────────────────────────────────────────────────

#[test]
fn query_select_empty_store() {
    let s = store();
    let res = query_sparql(
        &s,
        &args(json!({"query": "SELECT ?s ?p ?o WHERE { ?s ?p ?o }"})),
    )
    .unwrap();
    assert!(!res.is_error.unwrap_or(true));
    let text = res.content[0].as_text().unwrap().text.as_str();
    let body: serde_json::Value = serde_json::from_str(text).unwrap();
    // SPARQL JSON results format: results.bindings is empty
    assert_eq!(body["results"]["bindings"].as_array().unwrap().len(), 0);
}

#[test]
fn query_select_after_insert() {
    let s = store();
    update_sparql(
        &s,
        &args(json!({"update": "INSERT DATA { <urn:a> <urn:b> <urn:c> . }"})),
    )
    .unwrap();

    let res = query_sparql(
        &s,
        &args(json!({"query": "SELECT ?o WHERE { <urn:a> <urn:b> ?o }"})),
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    let bindings = body["results"]["bindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["o"]["value"], "urn:c");
}

#[test]
fn query_ask_true() {
    let s = store();
    update_sparql(
        &s,
        &args(json!({"update": "INSERT DATA { <urn:x> a <urn:Thing> . }"})),
    )
    .unwrap();
    let res = query_sparql(&s, &args(json!({"query": "ASK { <urn:x> a <urn:Thing> }"}))).unwrap();
    // ASK returns the bare string "true" or "false"
    assert_eq!(res.content[0].as_text().unwrap().text.as_str(), "true");
}

#[test]
fn query_ask_false() {
    let s = store();
    let res = query_sparql(
        &s,
        &args(json!({"query": "ASK { <urn:ghost> a <urn:Thing> }"})),
    )
    .unwrap();
    assert_eq!(res.content[0].as_text().unwrap().text.as_str(), "false");
}

#[test]
fn query_missing_sparql_arg_returns_mcp_error() {
    let s = store();
    let res = query_sparql(&s, &args(json!({})));
    assert!(res.is_err());
}

#[test]
fn query_invalid_sparql_returns_error_result() {
    let s = store();
    let res = query_sparql(&s, &args(json!({"query": "NOT SPARQL @@"})));
    // Either Err or is_error=true
    match res {
        Err(_) => {}
        Ok(r) => assert!(r.is_error.unwrap_or(false)),
    }
}

// ── update_sparql ─────────────────────────────────────────────────────────────

#[test]
fn update_insert_returns_delta() {
    let s = store();
    let res = update_sparql(
        &s,
        &args(json!({"update": "INSERT DATA { <urn:s> <urn:p> <urn:o> . }"})),
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["delta"], 1);
    assert_eq!(body["triples_after"], 1);
}

#[test]
fn update_delete_returns_negative_delta() {
    let s = store();
    update_sparql(
        &s,
        &args(json!({"update": "INSERT DATA { <urn:s> <urn:p> <urn:o> . }"})),
    )
    .unwrap();
    let res = update_sparql(
        &s,
        &args(json!({"update": "DELETE DATA { <urn:s> <urn:p> <urn:o> . }"})),
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["delta"], -1);
    assert_eq!(body["triples_after"], 0);
}

#[test]
fn update_into_named_graph() {
    let s = store();
    update_sparql(
        &s,
        &args(json!({"update":
            "INSERT DATA { GRAPH <urn:g1> { <urn:s> <urn:p> <urn:o> . } }",
        })),
    )
    .unwrap();
    let res = query_sparql(
        &s,
        &args(json!({"query": "SELECT ?o WHERE { GRAPH <urn:g1> { <urn:s> <urn:p> ?o } }"})),
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["results"]["bindings"].as_array().unwrap().len(), 1);
}
