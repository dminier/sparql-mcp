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

// ── ingest_batch ─────────────────────────────────────────────────────────────

use sparql_mcp::application::event_sink::{EventSink, StoreEvent};

#[derive(Default)]
struct Collect(std::sync::Mutex<Vec<StoreEvent>>);

impl EventSink for Collect {
    fn emit(&self, event: StoreEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn nt(n: u32, count: u32) -> String {
    (0..count)
        .map(|i| format!("<urn:s:{n}:{i}> <urn:p> <urn:o> .\n"))
        .collect()
}

fn graph_has(raw: &Arc<dyn SparqlStore>, graph: &str, s: &str) -> bool {
    ask(raw, &format!("ASK {{ GRAPH <{graph}> {{ {s} ?p ?o }} }}"))
}

async fn begin(server: &SparqlMcpServer, args: Value) -> (String, Value) {
    let b = ok(server, "ingest_begin", args).await;
    (b["job_id"].as_str().unwrap().to_string(), b)
}

#[tokio::test]
async fn a_staged_batch_lands_in_the_staging_graph_and_is_recorded() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();

    let r = ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 5) }),
    )
    .await;
    assert_eq!(r["parsed"], 5);
    assert_eq!(r["new_to_staging"], 5);
    assert_eq!(r["already_staged"], 0);
    assert_eq!(r["duplicate"], false);
    assert_eq!(r["job"]["batches"], 1);
    assert_eq!(r["job"]["triples"], 5);
    assert_eq!(r["job"]["remaining_triples"], 500_000 - 5);

    // In the staging graph, not in the target, and the target is untouched.
    assert!(graph_has(&raw, &staging, "<urn:s:1:0>"));
    assert!(!graph_has(&raw, ACTIVE, "<urn:s:1:0>"));

    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["batches"], 1);
    assert_eq!(s["received_seqs"], "1");
    assert_eq!(s["triples"], 5);
}

#[tokio::test]
async fn re_sending_a_batch_returns_the_original_result_and_stores_it_once() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();
    let data = nt(1, 4);

    let first = ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": data }),
    )
    .await;
    let size = raw.triple_count().unwrap();
    let again = ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": data }),
    )
    .await;

    assert_eq!(again["duplicate"], true);
    assert_eq!(again["parsed"], first["parsed"]);
    assert_eq!(
        again["new_to_staging"], first["new_to_staging"],
        "the original counts are replayed"
    );
    assert_eq!(again["job"]["triples"], 4, "a retry does not count twice");
    assert_eq!(again["job"]["batches"], 1);
    assert_eq!(raw.triple_count().unwrap(), size, "a retry wrote something");
    assert!(graph_has(&raw, &staging, "<urn:s:1:0>"));
}

#[tokio::test]
async fn the_same_seq_with_other_data_is_a_conflict_and_writes_nothing() {
    let (server, raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 3) }),
    )
    .await;
    let size = raw.triple_count().unwrap();

    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(2, 3) }),
    )
    .await;
    assert_eq!(b["code"], "seq_conflict");
    assert_eq!(b["job_id"], id);
    assert_eq!(raw.triple_count().unwrap(), size);
}

#[tokio::test]
async fn a_refused_batch_can_be_fixed_and_re_sent_under_the_same_seq() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();
    let size = raw.triple_count().unwrap();

    // Line 3 is broken: the whole batch is refused, nothing is recorded.
    let broken = "<urn:a> <urn:p> <urn:b> .\n<urn:c> <urn:p> <urn:d> .\n<urn:e> <urn:p> .\n";
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": broken }),
    )
    .await;
    assert_eq!(b["code"], "parse_error");
    assert_eq!(
        raw.triple_count().unwrap(),
        size,
        "part of a broken batch was written"
    );
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["received_seqs"], "");

    // The corrected batch is accepted under the same seq.
    let fixed = "<urn:a> <urn:p> <urn:b> .\n<urn:c> <urn:p> <urn:d> .\n<urn:e> <urn:p> <urn:f> .\n";
    let r = ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": fixed }),
    )
    .await;
    assert_eq!(r["parsed"], 3);
    assert!(graph_has(&raw, &staging, "<urn:e>"));
}

#[tokio::test]
async fn limits_are_refused_before_anything_is_written() {
    let cfg = IngestConfig {
        max_batch_bytes: 200,
        max_batch_triples: 3,
        max_iri_bytes: 40,
        ..IngestConfig::default()
    };
    let (server, raw) = server_with(cfg);
    let (id, _) = begin(&server, json!({})).await;
    let size = raw.triple_count().unwrap();

    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 20) }),
    )
    .await;
    assert_eq!(
        (b["code"].as_str(), b["limit_key"].as_str()),
        (Some("limit_exceeded"), Some("max_batch_bytes"))
    );

    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 4) }),
    )
    .await;
    assert_eq!(b["limit_key"], "max_batch_triples");
    assert_eq!(b["limit"], 3);
    assert_eq!(b["value"], 4);

    let long_iri = format!("<urn:{}> <urn:p> <urn:o> .\n", "x".repeat(60));
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": long_iri }),
    )
    .await;
    assert_eq!(b["limit_key"], "max_iri_bytes");

    assert_eq!(raw.triple_count().unwrap(), size);
}

#[tokio::test]
async fn reserved_iris_and_bad_arguments_are_refused() {
    let (server, raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    let size = raw.triple_count().unwrap();

    // An agent cannot forge a skolem IRI or name an internal resource.
    let forged = "<urn:sparql-mcp:genid:x:1> <urn:p> <urn:o> .\n";
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": forged }),
    )
    .await;
    assert_eq!(b["code"], "reserved_iri");

    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 0, "data": nt(1, 1) }),
    )
    .await;
    assert_eq!(b["code"], "invalid_argument");
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 99_999, "data": nt(1, 1) }),
    )
    .await;
    assert_eq!(b["code"], "invalid_argument");
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "seq": 1, "data": nt(1, 1) }),
    )
    .await;
    assert_eq!(b["code"], "job_not_found");
    assert!(
        call(
            &server,
            "ingest_batch",
            json!({ "job_id": id, "data": "x" })
        )
        .await
        .is_err(),
        "seq is required"
    );

    assert_eq!(raw.triple_count().unwrap(), size);
}

#[tokio::test]
async fn only_an_open_job_accepts_batches() {
    let (server, _raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    ok(&server, "ingest_abort", json!({ "job_id": id })).await;
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 1) }),
    )
    .await;
    assert_eq!(b["code"], "job_not_open");
    assert!(b["message"].as_str().unwrap().contains("aborted"));
}

#[tokio::test]
async fn a_labelled_blank_node_is_one_node_across_the_batches_of_a_job() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();

    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": "_:a <urn:name> \"first\" .\n" }),
    )
    .await;
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 2, "data": "_:a <urn:age> \"3\" .\n" }),
    )
    .await;

    // `_:a` is the same node in both: one subject with both properties.
    let skolem = format!("<urn:sparql-mcp:genid:{id}:a>");
    assert!(graph_has(&raw, &staging, &skolem));
    assert!(ask(
        &raw,
        &format!("ASK {{ GRAPH <{staging}> {{ {skolem} <urn:name> ?n ; <urn:age> ?a }} }}")
    ));

    // Another job's `_:a` is a different node.
    let (other, ob) = begin(&server, json!({})).await;
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": other, "seq": 1, "data": "_:a <urn:name> \"x\" .\n" }),
    )
    .await;
    assert!(!ask(
        &raw,
        &format!(
            "ASK {{ GRAPH <{}> {{ {skolem} <urn:name> \"x\" }} }}",
            ob["staging_graph"].as_str().unwrap()
        )
    ));
}

#[tokio::test]
async fn blank_nodes_can_be_refused_for_a_job() {
    let (server, raw) = server();
    let (id, _) = begin(&server, json!({ "blank_nodes": "reject" })).await;
    let size = raw.triple_count().unwrap();
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": "_:a <urn:p> <urn:o> .\n" }),
    )
    .await;
    assert_eq!(b["code"], "blank_node_rejected");
    assert_eq!(raw.triple_count().unwrap(), size);
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 1) }),
    )
    .await;
}

#[tokio::test]
async fn a_turtle_batch_is_deterministic_under_retry() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({ "format": "turtle" })).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();
    let ttl = "<urn:s> <urn:p> [ <urn:q> <urn:o> ] .\n";

    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": ttl }),
    )
    .await;
    let again = ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": ttl }),
    )
    .await;
    assert_eq!(again["duplicate"], true);
    // The anonymous node became a skolem IRI named after the job, batch and
    // order, not a random blank node.
    assert!(graph_has(
        &raw,
        &staging,
        &format!("<urn:sparql-mcp:genid:{id}:1:a0>")
    ));
}

#[tokio::test]
async fn the_job_ceilings_refuse_the_whole_batch() {
    let cfg = IngestConfig {
        max_job_triples_staged: 10,
        max_job_batches: 3,
        ..IngestConfig::default()
    };
    let (server, raw) = server_with(cfg);
    let (id, _) = begin(&server, json!({})).await;
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 8) }),
    )
    .await;
    let size = raw.triple_count().unwrap();

    // 8 + 4 = 12 > 10.
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 2, "data": nt(2, 4) }),
    )
    .await;
    assert_eq!(b["limit_key"], "max_job_triples_staged");
    assert_eq!(
        (b["limit"].as_u64(), b["value"].as_u64()),
        (Some(10), Some(12))
    );
    assert_eq!(
        raw.triple_count().unwrap(),
        size,
        "a refused batch moved the counters or wrote data"
    );
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(
        (s["batches"].as_u64(), s["triples"].as_u64()),
        (Some(1), Some(8))
    );

    // A smaller batch still fits, up to exactly the ceiling.
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 2, "data": nt(2, 2) }),
    )
    .await;
    // Seq 4 is above max_job_batches = 3.
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 4, "data": nt(4, 1) }),
    )
    .await;
    assert_eq!(b["code"], "invalid_argument");
}

#[tokio::test]
async fn status_reports_received_seqs_as_ranges() {
    let (server, _raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    for seq in [1, 2, 4] {
        ok(
            &server,
            "ingest_batch",
            json!({ "job_id": id, "seq": seq, "data": nt(seq, 2) }),
        )
        .await;
    }
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["received_seqs"], "1-2,4");
    assert_eq!(s["triples"], 6);
}

#[tokio::test]
async fn a_direct_batch_lands_in_the_target_and_tells_the_viewer_a_staged_one_does_not() {
    let sink = Arc::new(Collect::default());
    let (server, raw) = server();
    let server = server.with_event_sink(sink.clone());

    let (staged, _) = begin(&server, json!({})).await;
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": staged, "seq": 1, "data": nt(1, 3) }),
    )
    .await;
    assert!(
        sink.0.lock().unwrap().is_empty(),
        "a staged batch is invisible until commit"
    );

    let (direct, _) = begin(&server, json!({ "mode": "direct" })).await;
    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": direct, "seq": 1, "data": nt(2, 3) }),
    )
    .await;
    assert!(
        graph_has(&raw, ACTIVE, "<urn:s:2:0>"),
        "a direct batch goes to the target graph"
    );
    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        StoreEvent::Insert { graph, triples, .. } => {
            assert_eq!(graph, ACTIVE);
            assert_eq!(triples.len(), 3);
        }
        other => panic!("expected an insert event, got {other:?}"),
    }
}

#[tokio::test]
async fn a_direct_job_above_the_event_threshold_sends_one_changed_event_at_a_time() {
    let cfg = IngestConfig {
        event_changed_threshold: 5,
        ..IngestConfig::default()
    };
    let sink = Arc::new(Collect::default());
    let (server, _raw) = server_with(cfg);
    let server = server.with_event_sink(sink.clone());
    let (id, _) = begin(&server, json!({ "mode": "direct" })).await;

    ok(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 3) }),
    )
    .await; // total 3: inserts
    for seq in 2..=4 {
        ok(
            &server,
            "ingest_batch",
            json!({ "job_id": id, "seq": seq, "data": nt(seq, 3) }),
        )
        .await; // total 12: above
    }
    let events = sink.0.lock().unwrap();
    let inserts = events
        .iter()
        .filter(|e| matches!(e, StoreEvent::Insert { .. }))
        .count();
    let changed = events
        .iter()
        .filter(|e| matches!(e, StoreEvent::Changed { .. }))
        .count();
    assert_eq!(inserts, 1, "{events:?}");
    // Batches 2-4 arrive within the 5 s throttle window: one Changed, not three.
    assert_eq!(changed, 1, "{events:?}");
}

#[tokio::test]
async fn an_agent_cannot_write_into_a_staging_graph() {
    let (server, _raw) = server();
    let (_, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap();
    let r = refused(
        &server,
        "update_sparql",
        json!({
            "update": format!("INSERT DATA {{ GRAPH <{staging}> {{ <urn:x> <urn:p> <urn:o> }} }}")
        }),
    )
    .await;
    assert_eq!(r["code"], "graph_protected");
    let r = refused(
        &server,
        "load_ontology",
        json!({
            "turtle": "<urn:x> <urn:p> <urn:o> .", "format": "ntriples", "graph_iri": staging
        }),
    )
    .await;
    assert_eq!(r["code"], "graph_protected");
}

// ── concurrency, on a real RocksDB store ─────────────────────────────────────
//
// The in-memory store serializes writers by itself, so it cannot show a missing
// check inside the transaction. These tests use RocksDB, and several tool
// calls at once.

fn rocks_server(cfg: IngestConfig) -> (SparqlMcpServer, Arc<dyn SparqlStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let raw: Arc<dyn SparqlStore> =
        Arc::new(OxigraphAdapter::open(&dir.path().join("store")).unwrap());
    let (gated, privileged) = build_stores_with(
        raw.clone(),
        WriteGate::new(Duration::from_secs(20)),
        GuardConfig::default(),
    );
    let server = SparqlMcpServer::new(
        gated,
        privileged,
        Arc::new(FsDocStore::new(dir.path().join("docs"))),
        dir.path().join("ontology"),
        Arc::new(RwLock::new(ACTIVE.to_string())),
        None,
        dir.path().join("store"),
    )
    .with_ingest_config(cfg);
    (server, raw, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_retries_of_one_batch_store_it_once() {
    let (server, raw, _dir) = rocks_server(IngestConfig::default());
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();
    let data = nt(1, 50);

    // Eight calls for the same batch at once: the original and seven retries
    // that overlap it. All of them pass the fast paths before any has written.
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let server = server.clone();
            let (id, data) = (id.clone(), data.clone());
            tokio::spawn(async move {
                call(
                    &server,
                    "ingest_batch",
                    json!({ "job_id": id, "seq": 1, "data": data }),
                )
                .await
            })
        })
        .collect();
    let mut accepted = 0;
    let mut duplicates = 0;
    for c in calls {
        let (is_err, body) = c.await.unwrap().unwrap();
        assert!(!is_err, "a retry must not fail: {body}");
        if body["duplicate"] == true {
            duplicates += 1
        } else {
            accepted += 1
        }
        assert_eq!(
            body["new_to_staging"], 50,
            "every answer carries the original counts: {body}"
        );
        assert_eq!(body["job"]["triples"], 50, "{body}");
    }
    assert_eq!(
        (accepted, duplicates),
        (1, 7),
        "exactly one call writes the batch"
    );

    // The batch is in the store once, and the job counted it once.
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(
        (s["batches"].as_u64(), s["triples"].as_u64()),
        (Some(1), Some(50))
    );
    let count = match raw
        .query(&format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{staging}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap()
    {
        QueryResult::Solutions(s) => s.rows[0]["n"].as_value_str().parse::<u64>().unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(count, 50);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_different_batches_for_one_seq_never_both_land() {
    let (server, raw, _dir) = rocks_server(IngestConfig::default());
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();

    let calls: Vec<_> = (0..6)
        .map(|i| {
            let server = server.clone();
            let id = id.clone();
            tokio::spawn(async move {
                call(
                    &server,
                    "ingest_batch",
                    json!({ "job_id": id, "seq": 1, "data": nt(i, 10) }),
                )
                .await
            })
        })
        .collect();
    let mut landed = 0;
    let mut conflicts = 0;
    for c in calls {
        match c.await.unwrap().unwrap() {
            (false, _) => landed += 1,
            (true, body) => {
                assert_eq!(body["code"], "seq_conflict", "{body}");
                conflicts += 1;
            }
        }
    }
    assert_eq!((landed, conflicts), (1, 5));
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(
        (s["batches"].as_u64(), s["triples"].as_u64()),
        (Some(1), Some(10))
    );
    let count = match raw
        .query(&format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{staging}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap()
    {
        QueryResult::Solutions(s) => s.rows[0]["n"].as_value_str().parse::<u64>().unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(count, 10, "two different batches were both written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_batches_cannot_exceed_the_job_ceiling() {
    let cfg = IngestConfig {
        max_job_triples_staged: 40,
        ..IngestConfig::default()
    };
    let (server, _raw, _dir) = rocks_server(cfg);
    let (id, _) = begin(&server, json!({})).await;

    // Ten batches of 10 triples at once against a ceiling of 40: exactly four fit.
    let calls: Vec<_> = (1..=10)
        .map(|seq| {
            let server = server.clone();
            let id = id.clone();
            tokio::spawn(async move {
                call(
                    &server,
                    "ingest_batch",
                    json!({ "job_id": id, "seq": seq, "data": nt(seq, 10) }),
                )
                .await
            })
        })
        .collect();
    let mut landed = 0;
    for c in calls {
        match c.await.unwrap().unwrap() {
            (false, _) => landed += 1,
            (true, body) => assert_eq!(body["limit_key"], "max_job_triples_staged", "{body}"),
        }
    }
    assert_eq!(landed, 4, "the ceiling was exceeded or under-used");
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["triples"], 40);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_racing_an_abort_either_lands_before_it_or_is_refused() {
    let (server, raw, _dir) = rocks_server(IngestConfig::default());
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();

    let batches: Vec<_> = (1..=6)
        .map(|seq| {
            let server = server.clone();
            let id = id.clone();
            tokio::spawn(async move {
                call(
                    &server,
                    "ingest_batch",
                    json!({ "job_id": id, "seq": seq, "data": nt(seq, 5) }),
                )
                .await
            })
        })
        .collect();
    let abort = {
        let server = server.clone();
        let id = id.clone();
        tokio::spawn(async move { call(&server, "ingest_abort", json!({ "job_id": id })).await })
    };
    for c in batches {
        match c.await.unwrap().unwrap() {
            (false, _) => {}
            (true, body) => assert_eq!(body["code"], "job_not_open", "{body}"),
        }
    }
    let (abort_err, body) = abort.await.unwrap().unwrap();
    assert!(!abort_err, "{body}");

    // Aborted: the staging graph is gone, whatever order they ran in, and no
    // batch wrote into it afterwards.
    assert!(
        !ask(&raw, &format!("ASK {{ GRAPH <{staging}> {{ ?s ?p ?o }} }}")),
        "a batch landed in the staging graph after the abort"
    );
}
