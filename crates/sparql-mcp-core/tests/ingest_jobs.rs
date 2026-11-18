//! Ingestion jobs, lifecycle slice: `ingest_begin`, `ingest_status`,
//! `ingest_abort` (`docs/designs/sparql-mcp-batch-ingestion.md`), driven through
//! the real tool dispatch.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use rmcp::model::CallToolResult;
use serde_json::{json, Value};
use sparql_mcp::application::{build_stores_with, GuardConfig, WriteGate};
use sparql_mcp::config::IngestConfig;
use sparql_mcp::domain::{QueryResult, SparqlStore};
use sparql_mcp::infrastructure::{FsDocStore, OxigraphAdapter};
use sparql_mcp::mcp::server::SparqlMcpServer;

const ACTIVE: &str = "urn:project:default";

fn server_with(cfg: IngestConfig) -> (SparqlMcpServer, Arc<dyn SparqlStore>) {
    let raw: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let (gated, privileged) = build_stores_with(
        raw.clone(),
        WriteGate::new(Duration::from_secs(5)),
        GuardConfig::default(),
    );
    let dir = std::env::temp_dir().join("smcp-ingest");
    let server = SparqlMcpServer::new(
        gated,
        privileged,
        Arc::new(FsDocStore::new(dir.join("docs"))),
        dir.join("ontology"),
        Arc::new(RwLock::new(ACTIVE.to_string())),
        None,
        dir.join("store"),
    )
    .with_ingest_config(cfg);
    (server, raw)
}

fn server() -> (SparqlMcpServer, Arc<dyn SparqlStore>) {
    server_with(IngestConfig::default())
}

fn body(res: &CallToolResult) -> Value {
    let text = res.content[0].as_text().unwrap().text.as_str();
    serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

/// Run a tool: `Ok((is_error, body))`, or `Err(message)` for a JSON-RPC error.
async fn call(server: &SparqlMcpServer, tool: &str, args: Value) -> Result<(bool, Value), String> {
    let result = server
        .dispatch_core(tool, args.as_object().unwrap())
        .await
        .expect("a known tool");
    match result {
        Ok(res) => Ok((res.is_error.unwrap_or(false), body(&res))),
        Err(e) => Err(e.message.to_string()),
    }
}

async fn ok(server: &SparqlMcpServer, tool: &str, args: Value) -> Value {
    let (is_err, b) = call(server, tool, args).await.expect("no JSON-RPC error");
    assert!(!is_err, "{tool} was refused: {b}");
    b
}

async fn refused(server: &SparqlMcpServer, tool: &str, args: Value) -> Value {
    let (is_err, b) = call(server, tool, args).await.expect("no JSON-RPC error");
    assert!(is_err, "{tool} should be refused, got {b}");
    b
}

fn ask(raw: &Arc<dyn SparqlStore>, q: &str) -> bool {
    matches!(raw.query(q).unwrap(), QueryResult::Boolean(true))
}

#[tokio::test]
async fn a_staged_job_is_created_in_the_protected_ledger() {
    let (server, raw) = server();
    let b = ok(&server, "ingest_begin", json!({ "label": "code analysis" })).await;

    let id = b["job_id"].as_str().unwrap();
    assert_eq!(id.len(), 26, "a ULID: {id}");
    assert_eq!(b["state"], "open");
    assert_eq!(b["mode"], "staged");
    assert_eq!(b["target_graph"], ACTIVE);
    assert_eq!(b["staging_graph"], format!("urn:sparql-mcp:staging:{id}"));
    assert_eq!(b["format"], "ntriples");
    // The agent is told the limits to size its batches by.
    assert_eq!(b["limits"]["max_batch_bytes"], 1_048_576);
    assert_eq!(b["limits"]["max_batch_triples"], 10_000);
    assert_eq!(b["limits"]["max_open_jobs"], 4);

    assert!(ask(
        &raw,
        &format!("ASK {{ GRAPH <urn:sparql-mcp:jobs> {{ <urn:sparql-mcp:job:{id}> a <urn:sparql-mcp:jobs#Job> }} }}")
    ));
}

#[tokio::test]
async fn a_direct_job_has_no_staging_graph() {
    let (server, _raw) = server();
    let b = ok(&server, "ingest_begin", json!({ "mode": "direct" })).await;
    assert_eq!(b["mode"], "direct");
    assert!(b.get("staging_graph").is_none());
}

#[tokio::test]
async fn status_reports_one_job_and_lists_the_unfinished_ones() {
    let (server, _raw) = server();
    let a = ok(
        &server,
        "ingest_begin",
        json!({ "label": "first", "expected_batches": 3 }),
    )
    .await;
    let b = ok(&server, "ingest_begin", json!({ "mode": "direct" })).await;
    let (a_id, b_id) = (a["job_id"].as_str().unwrap(), b["job_id"].as_str().unwrap());

    let s = ok(&server, "ingest_status", json!({ "job_id": a_id })).await;
    assert_eq!(s["job_id"], a_id);
    assert_eq!(s["state"], "open");
    assert_eq!(s["mode"], "staged");
    assert_eq!(s["target_graph"], ACTIVE);
    assert_eq!(s["label"], "first");
    assert_eq!(s["expected_batches"], 3);
    assert_eq!(s["batches"], 0);
    assert_eq!(s["received_seqs"], "");
    assert_eq!(s["triples"], 0);
    assert!(s["created"].as_str().unwrap().ends_with('Z'));

    let list = ok(&server, "ingest_status", json!({})).await;
    let ids: Vec<&str> = list["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["job_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&a_id) && ids.contains(&b_id), "{list}");
    assert_eq!(list["max_open_jobs"], 4);
}

#[tokio::test]
async fn abort_ends_the_job_and_cannot_be_repeated() {
    let (server, raw) = server();
    let b = ok(&server, "ingest_begin", json!({})).await;
    let id = b["job_id"].as_str().unwrap();
    let staging = b["staging_graph"].as_str().unwrap();
    // Something already in the staging graph goes with it.
    raw.update(&format!(
        "INSERT DATA {{ GRAPH <{staging}> {{ <urn:s> <urn:p> <urn:o> }} }}"
    ))
    .unwrap();

    let a = ok(&server, "ingest_abort", json!({ "job_id": id })).await;
    assert_eq!(a["state"], "aborted");
    assert_eq!(a["direct_data_kept"], false);
    assert!(
        !ask(&raw, &format!("ASK {{ GRAPH <{staging}> {{ ?s ?p ?o }} }}")),
        "staging graph survived"
    );

    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["state"], "aborted");
    assert!(s["ended"].as_str().is_some());
    // A finished job is not in the unfinished list.
    let list = ok(&server, "ingest_status", json!({})).await;
    assert!(list["jobs"].as_array().unwrap().is_empty(), "{list}");

    let again = refused(&server, "ingest_abort", json!({ "job_id": id })).await;
    assert_eq!(again["code"], "job_not_open");
    assert_eq!(again["job_id"], id);
}

#[tokio::test]
async fn unknown_and_malformed_job_ids_are_not_found_and_never_reach_a_query() {
    let (server, raw) = server();
    let missing = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let b = refused(&server, "ingest_status", json!({ "job_id": missing })).await;
    assert_eq!(b["code"], "job_not_found");

    let b = refused(&server, "ingest_abort", json!({ "job_id": missing })).await;
    assert_eq!(b["code"], "job_not_found");

    // Anything that is not a ULID is refused as unknown, without being put in
    // SPARQL: an injection attempt does nothing.
    raw.update("INSERT DATA { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }")
        .unwrap();
    for evil in [
        "x> . } ; DROP GRAPH <urn:project:p> ; { <y",
        "../../etc/passwd",
        "",
    ] {
        let b = refused(&server, "ingest_abort", json!({ "job_id": evil })).await;
        assert_eq!(b["code"], "job_not_found", "{evil:?}");
    }
    assert!(ask(
        &raw,
        "ASK { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }"
    ));
}

#[tokio::test]
async fn the_number_of_open_jobs_is_limited() {
    let cfg = IngestConfig {
        max_open_jobs: 2,
        ..IngestConfig::default()
    };
    let (server, _raw) = server_with(cfg);
    let first = ok(&server, "ingest_begin", json!({})).await;
    ok(&server, "ingest_begin", json!({})).await;

    let b = refused(&server, "ingest_begin", json!({})).await;
    assert_eq!(b["code"], "too_many_open_jobs");
    assert!(b["message"].as_str().unwrap().contains("max_open_jobs = 2"));

    // Aborting one frees a slot.
    ok(
        &server,
        "ingest_abort",
        json!({ "job_id": first["job_id"] }),
    )
    .await;
    ok(&server, "ingest_begin", json!({})).await;
}

#[tokio::test]
async fn the_target_must_be_the_active_graph_or_a_created_project() {
    let (server, _raw) = server();

    for target in [
        "urn:meta",
        "urn:sparql-mcp:jobs",
        "urn:hkb:meta:loaded-files",
        "urn:project:unknown",
        "not an iri",
    ] {
        let b = refused(&server, "ingest_begin", json!({ "target_graph": target })).await;
        assert_eq!(b["code"], "invalid_target", "{target}");
    }

    // A project created through project_create is a valid target.
    ok(
        &server,
        "project_create",
        json!({ "id": "other", "label": "Other" }),
    )
    .await;
    let b = ok(
        &server,
        "ingest_begin",
        json!({ "target_graph": "urn:project:other" }),
    )
    .await;
    assert_eq!(b["target_graph"], "urn:project:other");
}

#[tokio::test]
async fn bad_arguments_are_told_apart() {
    let (server, _raw) = server();

    // Well-typed but not allowed: a refusal with a code.
    let b = refused(
        &server,
        "ingest_begin",
        json!({ "mode": "direct", "replace": true }),
    )
    .await;
    assert_eq!(b["code"], "invalid_argument");
    let b = refused(&server, "ingest_begin", json!({ "format": "rdfxml" })).await;
    assert_eq!(b["code"], "invalid_argument");
    let b = refused(
        &server,
        "ingest_begin",
        json!({ "expected_batches": 999_999 }),
    )
    .await;
    assert_eq!(b["code"], "invalid_argument");
    let b = refused(&server, "ingest_begin", json!({ "label": "x".repeat(201) })).await;
    assert_eq!(b["code"], "invalid_argument");

    // Wrong type or unknown enum value: a JSON-RPC error, as for any tool.
    assert!(call(&server, "ingest_begin", json!({ "mode": "bulk" }))
        .await
        .is_err());
    assert!(call(&server, "ingest_begin", json!({ "replace": "yes" }))
        .await
        .is_err());
    assert!(call(&server, "ingest_begin", json!({ "label": 5 }))
        .await
        .is_err());
    assert!(call(&server, "ingest_abort", json!({})).await.is_err());
}

#[tokio::test]
async fn a_label_cannot_break_out_of_the_ledger() {
    let (server, raw) = server();
    raw.update("INSERT DATA { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }")
        .unwrap();
    let evil = "x\" . } } ; DROP ALL ; INSERT DATA { GRAPH <urn:project:p> { <urn:e> <urn:p> \"";
    let b = ok(&server, "ingest_begin", json!({ "label": evil })).await;
    let id = b["job_id"].as_str().unwrap();

    // Stored and returned exactly as given, and nothing else happened.
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["label"], evil);
    assert!(ask(
        &raw,
        "ASK { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }"
    ));
    assert!(!ask(
        &raw,
        "ASK { GRAPH <urn:project:p> { <urn:e> ?p ?o } }"
    ));
}

#[tokio::test]
async fn an_agent_cannot_forge_or_change_a_job_through_update_sparql() {
    let (server, _raw) = server();
    let b = ok(&server, "ingest_begin", json!({})).await;
    let id = b["job_id"].as_str().unwrap();

    for update in [
        format!(
            "DELETE WHERE {{ GRAPH <urn:sparql-mcp:jobs> {{ <urn:sparql-mcp:job:{id}> ?p ?o }} }}"
        ),
        format!(
            "INSERT DATA {{ GRAPH <urn:sparql-mcp:jobs> {{ <urn:sparql-mcp:job:{id}> <urn:sparql-mcp:jobs#state> <urn:sparql-mcp:jobs#committed> }} }}"
        ),
        "CLEAR GRAPH <urn:sparql-mcp:jobs>".to_string(),
    ] {
        let r = refused(&server, "update_sparql", json!({ "update": update })).await;
        assert_eq!(r["code"], "graph_protected", "{update}");
    }
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["state"], "open");
}

#[tokio::test]
async fn the_ingest_tools_are_listed_with_the_core_tools() {
    // The tool-parity test already requires a dispatch arm for each listed
    // tool; this pins the names an agent will see.
    let (server, _raw) = server();
    for tool in ["ingest_begin", "ingest_status", "ingest_abort"] {
        assert!(server
            .dispatch_core(tool, &serde_json::Map::new())
            .await
            .is_some());
    }
}
