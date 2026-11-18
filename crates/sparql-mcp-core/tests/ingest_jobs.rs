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
    assert_eq!(r["job"]["remaining_triples"], 100_000 - 5);

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

// ── ingest_commit ────────────────────────────────────────────────────────────

fn count_in(raw: &Arc<dyn SparqlStore>, graph: &str) -> u64 {
    match raw
        .query(&format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap()
    {
        QueryResult::Solutions(s) => s.rows[0]["n"].as_value_str().parse().unwrap(),
        other => panic!("{other:?}"),
    }
}

async fn send(server: &SparqlMcpServer, id: &str, seq: u32, data: String) -> Value {
    ok(
        server,
        "ingest_batch",
        json!({ "job_id": id, "seq": seq, "data": data }),
    )
    .await
}

#[tokio::test]
async fn a_staged_commit_makes_the_data_visible_in_one_step() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();
    send(&server, &id, 1, nt(1, 5)).await;
    send(&server, &id, 2, nt(2, 5)).await;

    // Before the commit, nothing of the job is in the target.
    assert_eq!(count_in(&raw, ACTIVE), 0);

    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(c["state"], "committed");
    assert_eq!(c["mode"], "staged");
    assert_eq!(c["target_graph"], ACTIVE);
    assert_eq!(c["batches"], 2);
    assert_eq!(c["triples"], 10);
    assert_eq!(c["target_triples"], 10);
    assert_eq!(c["new_to_target"], 10);
    assert_eq!(c["new_subject_count"], 10);
    // The agent gets the IRIs to pass to viewer_show: sorted, bare.
    let subjects: Vec<&str> = c["new_subjects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert_eq!(subjects.len(), 10);
    assert_eq!(subjects[0], "urn:s:1:0");
    assert!(
        subjects.windows(2).all(|w| w[0] <= w[1]),
        "not sorted: {subjects:?}"
    );

    // All of it is in the target, none of it is left in staging.
    assert_eq!(count_in(&raw, ACTIVE), 10);
    assert_eq!(count_in(&raw, &staging), 0);
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["state"], "committed");
    assert!(s["ended"].as_str().is_some());
}

#[tokio::test]
async fn a_committed_job_is_closed() {
    let (server, _raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    send(&server, &id, 1, nt(1, 2)).await;
    ok(&server, "ingest_commit", json!({ "job_id": id })).await;

    for (tool, args) in [
        ("ingest_commit", json!({ "job_id": id })),
        ("ingest_abort", json!({ "job_id": id })),
        (
            "ingest_batch",
            json!({ "job_id": id, "seq": 2, "data": nt(2, 1) }),
        ),
    ] {
        let b = refused(&server, tool, args).await;
        assert_eq!(b["code"], "job_not_open", "{tool}");
        assert!(
            b["message"].as_str().unwrap().contains("committed"),
            "{tool}: {b}"
        );
    }
    // Retrying a batch the commit already took is refused too, not replayed.
    let b = refused(
        &server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 1, "data": nt(1, 2) }),
    )
    .await;
    assert_eq!(b["code"], "job_not_open");
}

#[tokio::test]
async fn the_commit_lists_only_subjects_that_were_not_in_the_target() {
    let (server, raw) = server();
    raw.update(&format!(
        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:s:1:0> <urn:old> <urn:x> }} }}"
    ))
    .unwrap();
    let (id, _) = begin(&server, json!({})).await;
    send(&server, &id, 1, nt(1, 3)).await; // urn:s:1:0 .. urn:s:1:2

    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    // `urn:s:1:0` already had a triple in the target: not new. Its staged
    // triple is, so three triples are new but only two subjects.
    assert_eq!(c["new_subject_count"], 2);
    assert_eq!(c["new_subjects"], json!(["urn:s:1:1", "urn:s:1:2"]));
    assert_eq!(c["new_to_target"], 3);
}

#[tokio::test]
async fn an_incomplete_job_cannot_be_committed() {
    let (server, raw) = server();
    let (id, b) = begin(&server, json!({ "expected_batches": 4 })).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();
    send(&server, &id, 1, nt(1, 2)).await;
    send(&server, &id, 4, nt(4, 2)).await;

    let e = refused(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(e["code"], "missing_batches");
    assert!(e["message"].as_str().unwrap().contains("2-3"), "{e}");
    assert_eq!(count_in(&raw, ACTIVE), 0, "a refused commit moved data");
    assert_eq!(count_in(&raw, &staging), 4);

    send(&server, &id, 2, nt(2, 2)).await;
    send(&server, &id, 3, nt(3, 2)).await;
    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(c["triples"], 8);

    // Without expected_batches, a gap below the highest seq is still a gap.
    let (id, _) = begin(&server, json!({})).await;
    send(&server, &id, 1, nt(10, 1)).await;
    send(&server, &id, 3, nt(11, 1)).await;
    let e = refused(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(e["code"], "missing_batches");
    assert!(
        e["message"].as_str().unwrap().contains("batches 2 of 1-3"),
        "{e}"
    );
}

#[tokio::test]
async fn an_empty_job_can_be_committed_and_validation_cannot_be_required_yet() {
    let (server, _raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    // ingest_validate does not exist yet, so a validation can never be on record.
    let e = refused(
        &server,
        "ingest_commit",
        json!({ "job_id": id, "require_validation": true }),
    )
    .await;
    assert_eq!(e["code"], "validation_required");
    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(
        (c["triples"].as_u64(), c["new_to_target"].as_u64()),
        (Some(0), Some(0))
    );
}

#[tokio::test]
async fn a_replace_commit_swaps_the_targets_contents() {
    let (server, raw) = server();
    raw.update(&format!(
        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:old:1> <urn:p> <urn:o> . <urn:old:2> <urn:p> <urn:o> }} }}"
    ))
    .unwrap();
    let (id, b) = begin(&server, json!({ "replace": true })).await;
    assert_eq!(b["replace"], true);
    send(&server, &id, 1, nt(1, 3)).await;

    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(c["replace"], true);
    assert_eq!(c["target_triples"], 3);
    assert_eq!(c["target_writes_discarded"], false);
    assert!(
        !graph_has(&raw, ACTIVE, "<urn:old:1>"),
        "the old contents survived a replace"
    );
    assert!(graph_has(&raw, ACTIVE, "<urn:s:1:0>"));
}

#[tokio::test]
async fn a_replace_reports_writes_that_it_discards() {
    let (server, raw) = server();
    let (id, _) = begin(&server, json!({ "replace": true })).await;
    send(&server, &id, 1, nt(1, 2)).await;
    // Somebody writes to the target while the job is open.
    raw.update(&format!(
        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:late> <urn:p> <urn:o> }} }}"
    ))
    .unwrap();
    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(c["target_writes_discarded"], true);
    assert!(!graph_has(&raw, ACTIVE, "<urn:late>"));
}

#[tokio::test]
async fn an_empty_replace_cannot_wipe_the_target_by_accident() {
    let (server, raw) = server();
    raw.update(&format!(
        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:keep> <urn:p> <urn:o> }} }}"
    ))
    .unwrap();
    let (id, _) = begin(&server, json!({ "replace": true })).await;

    // No batch was sent: the staging graph is empty.
    let e = refused(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(e["code"], "empty_replace");
    assert!(
        graph_has(&raw, ACTIVE, "<urn:keep>"),
        "a refused replace wiped the target"
    );
    // The job is still open and can take a batch.
    send(&server, &id, 1, nt(1, 1)).await;
    ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert!(!graph_has(&raw, ACTIVE, "<urn:keep>"));

    // Wiping on purpose is possible, and explicit.
    let (id, _) = begin(&server, json!({ "replace": true })).await;
    let c = ok(
        &server,
        "ingest_commit",
        json!({ "job_id": id, "allow_empty_replace": true }),
    )
    .await;
    assert_eq!(c["target_triples"], 0);
}

#[tokio::test]
async fn a_replace_that_would_not_fit_is_refused_when_the_job_begins() {
    let cfg = IngestConfig {
        max_replace_triples: 100,
        max_job_triples_staged: 60,
        ..IngestConfig::default()
    };
    let (server, raw) = server_with(cfg);
    // 50 triples in the target + up to 60 staged > 100.
    let data: String = (0..50)
        .map(|i| format!("<urn:t:{i}> <urn:p> <urn:o> . "))
        .collect();
    raw.update(&format!("INSERT DATA {{ GRAPH <{ACTIVE}> {{ {data} }} }}"))
        .unwrap();

    let e = refused(&server, "ingest_begin", json!({ "replace": true })).await;
    assert_eq!(e["code"], "replace_too_large");
    // An append of the same size is fine.
    ok(&server, "ingest_begin", json!({})).await;
}

#[tokio::test]
async fn a_direct_commit_only_closes_the_job() {
    let (server, raw) = server();
    let (id, _) = begin(&server, json!({ "mode": "direct" })).await;
    send(&server, &id, 1, nt(1, 4)).await;
    assert_eq!(
        count_in(&raw, ACTIVE),
        4,
        "a direct batch is already in the target"
    );

    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    assert_eq!(c["mode"], "direct");
    assert_eq!(c["triples"], 4);
    assert_eq!(c["target_triples"], 4);
    // There is no staging graph to diff against: no new_subjects for a direct job.
    assert!(c.get("new_subjects").is_none());
    let s = ok(&server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["state"], "committed");
}

#[tokio::test]
async fn a_small_staged_commit_reports_its_triples_to_the_viewer() {
    let sink = Arc::new(Collect::default());
    let (server, _raw) = server();
    let server = server.with_event_sink(sink.clone());
    let (id, _) = begin(&server, json!({})).await;
    send(&server, &id, 1, nt(1, 6)).await;
    assert!(
        sink.0.lock().unwrap().is_empty(),
        "nothing before the commit"
    );

    ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    let events = sink.0.lock().unwrap();
    let triples: usize = events
        .iter()
        .map(|e| match e {
            StoreEvent::Insert { graph, triples, .. } => {
                assert_eq!(
                    graph, ACTIVE,
                    "events must name the target graph, not the staging graph"
                );
                triples.len()
            }
            other => panic!("expected insert events, got {other:?}"),
        })
        .sum();
    assert_eq!(triples, 6);
}

#[tokio::test]
async fn a_large_or_replace_commit_sends_one_changed_event_with_real_sizes() {
    let cfg = IngestConfig {
        event_changed_threshold: 5,
        ..IngestConfig::default()
    };
    let sink = Arc::new(Collect::default());
    let (server, raw) = server_with(cfg);
    let server = server.with_event_sink(sink.clone());
    raw.update(&format!(
        "INSERT DATA {{ GRAPH <{ACTIVE}> {{ <urn:old> <urn:p> <urn:o> }} }}"
    ))
    .unwrap();

    // 8 triples above a threshold of 5: one Changed.
    let (id, _) = begin(&server, json!({})).await;
    send(&server, &id, 1, nt(1, 8)).await;
    ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    {
        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(
            matches!(
                &events[0],
                StoreEvent::Changed { graph, triples_before: 1, triples_after: 9 } if graph == ACTIVE
            ),
            "{events:?}"
        );
    }

    // A replace is always one Changed, even when small.
    sink.0.lock().unwrap().clear();
    let (id, _) = begin(&server, json!({ "replace": true })).await;
    send(&server, &id, 1, nt(2, 2)).await;
    ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    let events = sink.0.lock().unwrap();
    assert!(
        matches!(
            &events[..],
            [StoreEvent::Changed {
                triples_before: 9,
                triples_after: 2,
                ..
            }]
        ),
        "{events:?}"
    );
}

#[tokio::test]
async fn the_commit_reply_can_feed_viewer_show() {
    // The names an agent passes on: bare IRIs, not <...> and not blank nodes.
    let (server, _raw) = server();
    let (id, _) = begin(&server, json!({})).await;
    send(
        &server,
        &id,
        1,
        "_:a <urn:name> \"x\" .\n<urn:b> <urn:p> _:a .\n".to_string(),
    )
    .await;
    let c = ok(&server, "ingest_commit", json!({ "job_id": id })).await;
    let subjects = c["new_subjects"].as_array().unwrap();
    assert_eq!(subjects.len(), 2);
    for s in subjects {
        let s = s.as_str().unwrap();
        assert!(!s.starts_with('<') && !s.starts_with("_:"), "{s}");
    }
    // The skolemized node is an IRI under the job's namespace.
    assert!(subjects
        .iter()
        .any(|s| s.as_str().unwrap() == format!("urn:sparql-mcp:genid:{id}:a")));
}

// ── commit on a real RocksDB store, with concurrent callers ──────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_commits_close_the_job_once() {
    let (server, raw, _dir) = rocks_server(IngestConfig::default());
    let (id, staging) = {
        let (id, b) = begin(&server, json!({})).await;
        (id, b["staging_graph"].as_str().unwrap().to_string())
    };
    send(&server, &id, 1, nt(1, 20)).await;

    let calls: Vec<_> = (0..5)
        .map(|_| {
            let (server, id) = (server.clone(), id.clone());
            tokio::spawn(
                async move { call(&server, "ingest_commit", json!({ "job_id": id })).await },
            )
        })
        .collect();
    let mut committed = 0;
    for c in calls {
        match c.await.unwrap().unwrap() {
            (false, _) => committed += 1,
            (true, body) => assert_eq!(body["code"], "job_not_open", "{body}"),
        }
    }
    assert_eq!(committed, 1);
    assert_eq!(count_in(&raw, ACTIVE), 20);
    assert_eq!(count_in(&raw, &staging), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_racing_a_commit_is_in_the_commit_or_refused_never_lost() {
    let (server, raw, _dir) = rocks_server(IngestConfig::default());
    let (id, b) = begin(&server, json!({})).await;
    let staging = b["staging_graph"].as_str().unwrap().to_string();

    let batches: Vec<_> = (1..=8)
        .map(|seq| {
            let (server, id) = (server.clone(), id.clone());
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
    // The commit comes while the batches are in flight. Without expected_batches
    // it takes whatever has landed; it needs the batches it saw to be gapless.
    let commit = {
        let (server, id) = (server.clone(), id.clone());
        tokio::spawn(async move { call(&server, "ingest_commit", json!({ "job_id": id })).await })
    };
    let mut landed = 0u64;
    for c in batches {
        match c.await.unwrap().unwrap() {
            (false, _) => landed += 1,
            (true, body) => assert_eq!(body["code"], "job_not_open", "{body}"),
        }
    }
    let (commit_err, body) = commit.await.unwrap().unwrap();

    if commit_err {
        // The commit may be refused for a gap (batches landed out of order);
        // then the job is still open and nothing moved.
        assert_eq!(body["code"], "missing_batches", "{body}");
        assert_eq!(count_in(&raw, ACTIVE), 0);
    } else {
        // Every batch that was accepted is in the target, and nothing is left behind.
        assert_eq!(
            count_in(&raw, ACTIVE),
            landed * 5,
            "an accepted batch was lost"
        );
        assert_eq!(
            count_in(&raw, &staging),
            0,
            "a batch landed in staging after the commit"
        );
        assert_eq!(body["triples"].as_u64().unwrap(), landed * 5);
    }
}
