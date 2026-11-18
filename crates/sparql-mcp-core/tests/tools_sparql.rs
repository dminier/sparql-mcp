//! Integration tests — query_sparql / update_sparql application tools.

use std::sync::{Arc, RwLock};

use serde_json::json;
use sparql_mcp::application::event_sink::NullEventSink;
use sparql_mcp::application::tools::sparql::{query_sparql, update_sparql};
use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::OxigraphAdapter;

fn store() -> Arc<dyn SparqlStore> {
    Arc::new(OxigraphAdapter::open_in_memory().unwrap())
}

fn active_graph() -> Arc<RwLock<String>> {
    Arc::new(RwLock::new("urn:project:default".to_string()))
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
        &active_graph(),
        &NullEventSink,
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
        &active_graph(),
        &NullEventSink,
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
        &active_graph(),
        &NullEventSink,
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["delta"], 1);
    assert_eq!(body["inserted_new"], 1);
    assert!(
        body["triples_after"].is_null(),
        "ground updates no longer scan the store"
    );
}

#[test]
fn update_delete_returns_negative_delta() {
    let s = store();
    update_sparql(
        &s,
        &args(json!({"update": "INSERT DATA { <urn:s> <urn:p> <urn:o> . }"})),
        &active_graph(),
        &NullEventSink,
    )
    .unwrap();
    let res = update_sparql(
        &s,
        &args(json!({"update": "DELETE DATA { <urn:s> <urn:p> <urn:o> . }"})),
        &active_graph(),
        &NullEventSink,
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["delta"], -1);
    assert_eq!(body["removed_existing"], 1);
    assert!(body["triples_after"].is_null());
}

#[test]
fn update_into_named_graph() {
    let s = store();
    update_sparql(
        &s,
        &args(json!({"update":
            "INSERT DATA { GRAPH <urn:g1> { <urn:s> <urn:p> <urn:o> . } }",
        })),
        &active_graph(),
        &NullEventSink,
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

// ── Hardening 0b: events name the right graph ─────────────────────────────────

use sparql_mcp::application::event_sink::{EventSink, StoreEvent};
use sparql_mcp::application::tools::ontology::load_ontology;

#[derive(Default)]
struct Collect(std::sync::Mutex<Vec<StoreEvent>>);

impl EventSink for Collect {
    fn emit(&self, event: StoreEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn graphs_of(sink: &Collect) -> Vec<String> {
    sink.0
        .lock()
        .unwrap()
        .iter()
        .map(|e| match e {
            StoreEvent::Insert { graph, .. }
            | StoreEvent::Delete { graph, .. }
            | StoreEvent::Changed { graph, .. } => graph.clone(),
        })
        .collect()
}

#[test]
fn insert_data_into_another_graph_is_not_tagged_with_the_active_graph() {
    let s = store();
    let sink = Collect::default();
    update_sparql(
        &s,
        &args(json!({"update":
            "INSERT DATA { GRAPH <urn:project:other> { <urn:s> <urn:p> <urn:o> } }"})),
        &active_graph(), // urn:project:default
        &sink,
    )
    .unwrap();
    assert_eq!(graphs_of(&sink), vec!["urn:project:other".to_string()]);
}

#[test]
fn insert_data_in_the_default_graph_is_reported_with_an_empty_graph_name() {
    let s = store();
    let sink = Collect::default();
    update_sparql(
        &s,
        &args(json!({"update": "INSERT DATA { <urn:s> <urn:p> <urn:o> }"})),
        &active_graph(),
        &sink,
    )
    .unwrap();
    // "" is the default graph: no viewer shows it, so it never draws as if it
    // were the active project graph.
    assert_eq!(graphs_of(&sink), vec![String::new()]);
}

#[test]
fn load_ontology_without_graph_iri_goes_to_the_active_graph() {
    let s = store();
    let sink = Collect::default();
    let res = load_ontology(
        &s,
        &args(json!({"turtle": "<urn:s> <urn:p> <urn:o> .", "format": "ntriples"})),
        &active_graph(),
        &sink,
    )
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["graph_iri"], "urn:project:default");
    assert_eq!(body["inserted_new"], 1);
    assert_eq!(graphs_of(&sink), vec!["urn:project:default".to_string()]);
    // It really is in that graph, not in the default graph.
    let ask = query_sparql(
        &s,
        &args(json!({"query": "ASK { GRAPH <urn:project:default> { <urn:s> <urn:p> <urn:o> } }"})),
    )
    .unwrap();
    assert_eq!(ask.content[0].as_text().unwrap().text.as_str(), "true");
}
