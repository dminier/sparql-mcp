//! Integration tests — project_create / project_list / project_switch tools.

use std::sync::Arc;

use serde_json::json;
use sparql_mcp::application::tools::project::{project_create, project_list, project_switch};
use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::OxigraphAdapter;

fn store() -> Arc<dyn SparqlStore> {
    Arc::new(OxigraphAdapter::open_in_memory().unwrap())
}

fn args(val: serde_json::Value) -> rmcp::model::JsonObject {
    val.as_object().unwrap().clone()
}

// ── project_create ────────────────────────────────────────────────────────────

#[test]
fn create_project_roundtrip() {
    let s = store();
    let res = project_create(&s, &args(json!({"id": "my-proj", "label": "My Project"}))).unwrap();
    assert!(!res.is_error.unwrap_or(true));

    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["id"], "my-proj");
    assert_eq!(body["named_graph"], "urn:project:my-proj");
}

#[test]
fn create_project_invalid_id_rejected() {
    let s = store();
    let res = project_create(&s, &args(json!({"id": "bad id!", "label": "x"})));
    assert!(res.is_err() || res.unwrap().is_error.unwrap_or(false));
}

#[test]
fn create_project_empty_id_rejected() {
    let s = store();
    let res = project_create(&s, &args(json!({"id": "", "label": "x"})));
    assert!(res.is_err() || res.unwrap().is_error.unwrap_or(false));
}

#[test]
fn create_project_missing_label_rejected() {
    let s = store();
    let res = project_create(&s, &args(json!({"id": "ok-id"})));
    assert!(res.is_err());
}

// ── project_list ──────────────────────────────────────────────────────────────

#[test]
fn list_empty_store_returns_empty_array() {
    let s = store();
    let res = project_list(&s).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["projects"].as_array().unwrap().len(), 0);
}

#[test]
fn list_shows_created_projects() {
    let s = store();
    project_create(&s, &args(json!({"id": "alpha", "label": "Alpha"}))).unwrap();
    project_create(&s, &args(json!({"id": "beta",  "label": "Beta"}))).unwrap();

    let res = project_list(&s).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    let projects = body["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2);

    let ids: Vec<&str> = projects.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"alpha"));
    assert!(ids.contains(&"beta"));
}

#[test]
fn create_same_id_twice_project_appears_in_list() {
    // load_rdf is additive (no upsert) — creating twice leaves both sets of
    // triples in <urn:meta>.  The project still appears in the list.
    let s = store();
    project_create(&s, &args(json!({"id": "dup", "label": "First"}))).unwrap();
    project_create(&s, &args(json!({"id": "dup", "label": "Second"}))).unwrap();

    let res = project_list(&s).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    let ids: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["id"].as_str() == Some("dup"))
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(!ids.is_empty(), "project should still be listed");
}

// ── project_switch ────────────────────────────────────────────────────────────

#[test]
fn switch_returns_correct_graph_iri() {
    let s = store();
    let res = project_switch(&s, &args(json!({"id": "my-proj"}))).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(res.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(body["active_graph"], "urn:project:my-proj");
    assert_eq!(body["status"], "ok");
}

#[test]
fn switch_invalid_id_rejected() {
    let s = store();
    let res = project_switch(&s, &args(json!({"id": "bad id!"})));
    assert!(res.is_err() || res.unwrap().is_error.unwrap_or(false));
}
