//! `ingest_file`: streaming a server-side N-Triples file into a job
//! (`docs/designs/sparql-mcp-batch-ingestion.md`, *File jobs*), driven through
//! the real tool dispatch. The runner is a background thread, so tests poll the
//! job's status.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rmcp::model::CallToolResult;
use serde_json::{json, Value};
use sparql_mcp::application::{build_stores_with, GuardConfig, WriteGate};
use sparql_mcp::config::IngestConfig;
use sparql_mcp::domain::{QueryResult, SparqlStore};
use sparql_mcp::infrastructure::{FsDocStore, OxigraphAdapter};
use sparql_mcp::mcp::server::SparqlMcpServer;

const ACTIVE: &str = "urn:project:default";

struct Env {
    server: SparqlMcpServer,
    raw: Arc<dyn SparqlStore>,
    dir: tempfile::TempDir,
}

fn env_with(adjust: impl FnOnce(&mut IngestConfig)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let raw: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let (gated, privileged) = build_stores_with(
        raw.clone(),
        WriteGate::new(Duration::from_secs(5)),
        GuardConfig::default(),
    );
    let mut cfg = IngestConfig {
        file_roots: Some(vec![dir.path().to_path_buf()]),
        file_batch_triples: 1000,
        ..IngestConfig::default()
    };
    adjust(&mut cfg);
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
    Env { server, raw, dir }
}

fn env() -> Env {
    env_with(|_| {})
}

async fn call(server: &SparqlMcpServer, tool: &str, args: Value) -> (bool, Value) {
    let res: CallToolResult = server
        .dispatch_core(tool, args.as_object().unwrap())
        .await
        .expect("a known tool")
        .expect("no JSON-RPC error");
    let text = res.content[0].as_text().unwrap().text.as_str();
    (
        res.is_error.unwrap_or(false),
        serde_json::from_str(text).unwrap_or(Value::String(text.to_string())),
    )
}

async fn ok(server: &SparqlMcpServer, tool: &str, args: Value) -> Value {
    let (is_err, b) = call(server, tool, args).await;
    assert!(!is_err, "{tool} was refused: {b}");
    b
}

async fn refused(server: &SparqlMcpServer, tool: &str, args: Value) -> Value {
    let (is_err, b) = call(server, tool, args).await;
    assert!(is_err, "{tool} should be refused, got {b}");
    b
}

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

fn write_nt(dir: &Path, name: &str, triples: u32) -> PathBuf {
    let path = dir.join(name);
    let text: String = (0..triples)
        .map(|i| format!("<urn:s:{i}> <urn:p> <urn:o:{}> .\n", i % 7))
        .collect();
    std::fs::write(&path, text).unwrap();
    path
}

fn arg(path: &Path) -> String {
    path.display().to_string()
}

/// Poll `ingest_status` until the job is in one of `states`.
async fn wait_for(server: &SparqlMcpServer, id: &str, states: &[&str]) -> Value {
    let started = Instant::now();
    loop {
        let s = ok(server, "ingest_status", json!({ "job_id": id })).await;
        if states.contains(&s["state"].as_str().unwrap()) {
            return s;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "job {id} still {} after 60 s: {s}",
            s["state"]
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_file_is_streamed_in_batches_into_the_target_and_recorded_as_loaded() {
    let e = env();
    // An exact multiple of the batch size: the last batch must still be
    // recognised as the last one.
    let file = write_nt(e.dir.path(), "dump.nt", 12_000);
    let size = std::fs::metadata(&file).unwrap().len();

    let b = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await;
    assert_eq!(b["state"], "running");
    let id = b["job_id"].as_str().unwrap().to_string();

    let s = wait_for(&e.server, &id, &["committed", "failed"]).await;
    assert_eq!(s["state"], "committed", "{s}");
    assert_eq!(s["triples"], 12_000);
    assert_eq!(s["batches"], 12);
    assert_eq!(s["file"]["bytes"], size);
    assert_eq!(s["file"]["bytes_read"], size);
    assert_eq!(s["file"]["percent"], 100.0);
    assert!(s["ended"].as_str().is_some());
    assert_eq!(count_in(&e.raw, ACTIVE), 12_000);

    // The file is on record as loaded: the same file is refused, unless forced.
    let again = refused(&e.server, "ingest_file", json!({ "path": arg(&file) })).await;
    assert_eq!(again["code"], "already_loaded");
    let forced = ok(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "force": true }),
    )
    .await;
    let s = wait_for(
        &e.server,
        forced["job_id"].as_str().unwrap(),
        &["committed", "failed"],
    )
    .await;
    assert_eq!(s["state"], "committed");
    assert_eq!(
        count_in(&e.raw, ACTIVE),
        12_000,
        "the same triples are not added twice"
    );
}

#[tokio::test]
async fn the_same_file_into_another_graph_is_a_different_load() {
    let e = env();
    let file = write_nt(e.dir.path(), "a.nt", 50);
    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for(&e.server, &id, &["committed"]).await;
    ok(
        &e.server,
        "project_create",
        json!({ "id": "other", "label": "other" }),
    )
    .await;
    let other = "urn:project:other";
    let b = ok(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "target_graph": other }),
    )
    .await;
    wait_for(&e.server, b["job_id"].as_str().unwrap(), &["committed"]).await;
    assert_eq!(count_in(&e.raw, other), 50);
}

#[tokio::test]
async fn comments_blank_lines_crlf_and_a_missing_final_newline_are_handled() {
    let e = env_with(|c| c.file_batch_triples = 2);
    let file = e.dir.path().join("odd.nt");
    std::fs::write(
        &file,
        "# a comment\r\n\r\n<urn:a> <urn:p> <urn:b> .\r\n   \r\n# another\r\n<urn:c> <urn:p> <urn:d> .\r\n<urn:e> <urn:p> <urn:f> .\r\n<urn:g> <urn:p> <urn:h> .",
    )
    .unwrap();
    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    let s = wait_for(&e.server, &id, &["committed", "failed"]).await;
    assert_eq!(s["state"], "committed", "{s}");
    assert_eq!(count_in(&e.raw, ACTIVE), 4);
}

#[tokio::test]
async fn a_blank_node_is_the_same_node_in_every_batch_of_a_file() {
    let e = env_with(|c| c.file_batch_triples = 10);
    let file = e.dir.path().join("blank.nt");
    let mut text = String::from("_:x <urn:p> <urn:first> .\n");
    for i in 0..30 {
        text.push_str(&format!("<urn:s:{i}> <urn:p> <urn:o> .\n"));
    }
    text.push_str("_:x <urn:p> <urn:last> .\n");
    std::fs::write(&file, text).unwrap();
    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for(&e.server, &id, &["committed"]).await;
    let r = e
        .raw
        .query(&format!(
            "SELECT ?s WHERE {{ GRAPH <{ACTIVE}> {{ ?s <urn:p> <urn:first> . ?s <urn:p> <urn:last> }} }}"
        ))
        .unwrap();
    match r {
        QueryResult::Solutions(s) => assert_eq!(s.rows.len(), 1, "one node, in two batches"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn files_are_refused_unless_file_roots_allows_them() {
    // Unset: refused, whatever the path.
    let e = env_with(|c| c.file_roots = None);
    let file = write_nt(e.dir.path(), "x.nt", 3);
    let r = refused(&e.server, "ingest_file", json!({ "path": arg(&file) })).await;
    assert_eq!(r["code"], "file_roots_unset");

    // Set: a file outside the root, and one reached through `..`, are refused.
    let e = env();
    let outside = tempfile::tempdir().unwrap();
    let elsewhere = write_nt(outside.path(), "out.nt", 3);
    let r = refused(&e.server, "ingest_file", json!({ "path": arg(&elsewhere) })).await;
    assert_eq!(r["code"], "path_outside_roots");
    std::fs::create_dir(e.dir.path().join("sub")).unwrap();
    let sneaky = e.dir.path().join("sub").join("..").join("..").join(
        elsewhere
            .strip_prefix(elsewhere.ancestors().nth(2).unwrap())
            .unwrap(),
    );
    let r = refused(&e.server, "ingest_file", json!({ "path": arg(&sneaky) })).await;
    assert!(
        ["path_outside_roots", "path_rejected"].contains(&r["code"].as_str().unwrap())
            || r["message"].as_str().unwrap().contains("resolve"),
        "{r}"
    );
    // Nothing was started.
    let list = ok(&e.server, "ingest_status", json!({})).await;
    assert!(list["jobs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn only_ntriples_and_files_within_max_file_bytes_are_accepted() {
    let e = env_with(|c| c.max_file_bytes = 1_000);
    let ttl = e.dir.path().join("data.ttl");
    std::fs::write(&ttl, "<urn:a> <urn:p> <urn:b> .\n").unwrap();
    let r = refused(&e.server, "ingest_file", json!({ "path": arg(&ttl) })).await;
    assert_eq!(r["code"], "invalid_argument");
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&ttl), "format": "turtle" }),
    )
    .await;
    assert_eq!(r["code"], "invalid_argument");

    let big = write_nt(e.dir.path(), "big.nt", 100);
    let r = refused(&e.server, "ingest_file", json!({ "path": arg(&big) })).await;
    assert_eq!(r["code"], "limit_exceeded");
    assert_eq!(r["limit_key"], "max_file_bytes");

    let missing = e.dir.path().join("nope.nt");
    let r = refused(&e.server, "ingest_file", json!({ "path": arg(&missing) })).await;
    assert_eq!(r["code"], "file_not_found");
    let dir_arg = arg(e.dir.path());
    let (is_err, _) = call(&e.server, "ingest_file", json!({ "path": dir_arg })).await;
    assert!(is_err);
}

#[tokio::test]
async fn a_staged_file_job_ends_open_and_is_committed_like_any_staged_job() {
    let e = env();
    let file = write_nt(e.dir.path(), "small.nt", 2_500);
    let b = ok(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "mode": "staged" }),
    )
    .await;
    let id = b["job_id"].as_str().unwrap().to_string();
    let s = wait_for(&e.server, &id, &["open", "failed"]).await;
    assert_eq!(s["state"], "open", "{s}");
    assert_eq!(s["triples"], 2_500);
    assert!(s.get("ended").is_none(), "an open job has not ended: {s}");
    assert_eq!(
        count_in(&e.raw, ACTIVE),
        0,
        "nothing is visible before the commit"
    );

    let v = ok(
        &e.server,
        "ingest_validate",
        json!({ "job_id": id, "checks": [
            { "name": "has-data", "query": "SELECT ?s WHERE { GRAPH {{staging}} { ?s ?p ?o } }", "expect": "non_empty" }
        ]}),
    )
    .await;
    assert_eq!(v["passed"], true);
    let c = ok(
        &e.server,
        "ingest_commit",
        json!({ "job_id": id, "require_validation": true }),
    )
    .await;
    assert_eq!(c["triples"], 2_500);
    assert_eq!(count_in(&e.raw, ACTIVE), 2_500);
}

#[tokio::test]
async fn a_staged_job_whose_file_cannot_fit_is_refused_before_it_starts() {
    let e = env_with(|c| c.max_job_triples_staged = 50);
    let file = write_nt(e.dir.path(), "toobig.nt", 1_000);
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "mode": "staged" }),
    )
    .await;
    assert_eq!(r["code"], "staged_too_large");
    let list = ok(&e.server, "ingest_status", json!({})).await;
    assert!(list["jobs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_bad_line_fails_the_job_and_keeps_the_batches_before_it() {
    let e = env_with(|c| c.file_batch_triples = 100);
    let file = e.dir.path().join("bad.nt");
    let mut text: String = (0..250)
        .map(|i| format!("<urn:s:{i}> <urn:p> <urn:o> .\n"))
        .collect();
    text.push_str("this is not a triple\n");
    text.push_str("<urn:z> <urn:p> <urn:o> .\n");
    std::fs::write(&file, &text).unwrap();

    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    let s = wait_for(&e.server, &id, &["failed", "committed"]).await;
    assert_eq!(s["state"], "failed", "{s}");
    assert!(s["error"].as_str().is_some_and(|m| !m.is_empty()), "{s}");
    // Batches 1 and 2 (200 triples) landed; the third, with the bad line, did not.
    assert_eq!(s["batches"], 2);
    assert_eq!(count_in(&e.raw, ACTIVE), 200);
    // The checkpoint is the end of the last good batch.
    let two_batches: usize = text.lines().take(200).map(|l| l.len() + 1).sum();
    assert_eq!(s["file"]["bytes_read"], two_batches as u64);
    // A failed job is not resumable, and is not recorded as a completed load.
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": id }),
    )
    .await;
    assert_eq!(r["code"], "job_not_interrupted");
    ok(&e.server, "ingest_abort", json!({ "job_id": id })).await;
}

#[tokio::test]
async fn a_line_longer_than_a_batch_fails_the_job_with_its_position() {
    let e = env_with(|c| c.max_batch_bytes = 200);
    let file = e.dir.path().join("long.nt");
    let long = format!("<urn:a> <urn:p> \"{}\" .\n", "x".repeat(400));
    std::fs::write(&file, format!("<urn:s> <urn:p> <urn:o> .\n{long}")).unwrap();
    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    let s = wait_for(&e.server, &id, &["failed", "committed"]).await;
    assert_eq!(s["state"], "failed", "{s}");
    assert!(
        s["error"].as_str().unwrap().contains("max_batch_bytes"),
        "{s}"
    );
}

#[tokio::test]
async fn a_running_job_can_be_stopped_then_resumed_without_loss_or_duplication() {
    // One triple per batch makes the run slow enough to interrupt.
    let e = env_with(|c| c.file_batch_triples = 1);
    let file = write_nt(e.dir.path(), "slow.nt", 6_000);
    let original = std::fs::read(&file).unwrap();

    let b = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await;
    let id = b["job_id"].as_str().unwrap().to_string();

    // Status of a running job shows progress.
    let s = ok(&e.server, "ingest_status", json!({ "job_id": id })).await;
    assert_eq!(s["state"], "running");
    assert!(s["file"]["bytes"].as_u64().unwrap() > 0);

    // Stop it. It finishes its current batch, then becomes interrupted.
    let stop = ok(&e.server, "ingest_abort", json!({ "job_id": id })).await;
    assert_eq!(stop["stopping"], true);
    let s = wait_for(&e.server, &id, &["interrupted", "committed"]).await;
    assert_eq!(
        s["state"], "interrupted",
        "the job finished before it could be stopped: {s}"
    );
    let done = s["triples"].as_u64().unwrap();
    assert!(done < 6_000, "{done}");
    assert_eq!(
        s["file"]["bytes_read"].as_u64().unwrap(),
        original
            .split_inclusive(|b| *b == b'\n')
            .take(done as usize)
            .map(|l| l.len() as u64)
            .sum::<u64>(),
        "the checkpoint is the end of the last batch written"
    );

    // A file that changed meanwhile is not the one the job was reading.
    let mut changed = original.clone();
    changed.extend_from_slice(b"<urn:extra> <urn:p> <urn:o> .\n");
    std::fs::write(&file, &changed).unwrap();
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": id }),
    )
    .await;
    assert_eq!(r["code"], "source_changed");
    assert_eq!(
        ok(&e.server, "ingest_status", json!({ "job_id": id })).await["state"],
        "interrupted",
        "a refused resume leaves the job as it was"
    );

    // The original file resumes from the checkpoint.
    std::fs::write(&file, &original).unwrap();
    let r = ok(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": id }),
    )
    .await;
    assert_eq!(r["state"], "running");
    assert!(r["resumed_from_byte"].as_u64().unwrap() > 0);
    let s = wait_for(&e.server, &id, &["committed", "failed"]).await;
    assert_eq!(s["state"], "committed", "{s}");
    assert_eq!(s["triples"], 6_000);
    assert_eq!(s["batches"], 6_000);
    assert_eq!(count_in(&e.raw, ACTIVE), 6_000);
    assert_eq!(s["file"]["percent"], 100.0);
}

#[tokio::test]
async fn a_core_restart_interrupts_a_running_file_job_and_it_can_resume() {
    let e = env_with(|c| c.file_batch_triples = 1);
    let file = write_nt(e.dir.path(), "restart.nt", 4_000);
    let b = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await;
    let id = b["job_id"].as_str().unwrap().to_string();
    // Let it write something, then stop it the way a crash would leave it:
    // the ledger says `running` but no runner is alive for it.
    let s = wait_for(&e.server, &id, &["running", "committed"]).await;
    assert_eq!(s["state"], "running");
    ok(&e.server, "ingest_abort", json!({ "job_id": id })).await;
    wait_for(&e.server, &id, &["interrupted", "committed"]).await;
    e.raw
        .update(&format!(
            "DELETE {{ GRAPH <urn:sparql-mcp:jobs> {{ <urn:sparql-mcp:job:{id}> <urn:sparql-mcp:jobs#state> ?s }} }} \
             INSERT {{ GRAPH <urn:sparql-mcp:jobs> {{ <urn:sparql-mcp:job:{id}> <urn:sparql-mcp:jobs#state> <urn:sparql-mcp:jobs#running> }} }} \
             WHERE {{ GRAPH <urn:sparql-mcp:jobs> {{ <urn:sparql-mcp:job:{id}> <urn:sparql-mcp:jobs#state> ?s }} }}"
        ))
        .unwrap();
    // `running` with no runner cannot be stopped by an abort: it is a leftover.
    let r = e
        .server
        .ingest_service()
        .recover(sparql_mcp::application::clock::now_secs())
        .unwrap();
    assert_eq!(r.interrupted, vec![id.clone()]);
    let r = ok(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": id }),
    )
    .await;
    assert_eq!(r["state"], "running");
    let s = wait_for(&e.server, &id, &["committed", "failed"]).await;
    assert_eq!(s["state"], "committed", "{s}");
    assert_eq!(count_in(&e.raw, ACTIVE), 4_000);
}

#[tokio::test]
async fn only_a_file_job_can_be_resumed_and_only_when_interrupted() {
    let e = env();
    let file = write_nt(e.dir.path(), "r.nt", 10);
    let plain = ok(&e.server, "ingest_begin", json!({})).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": plain }),
    )
    .await;
    assert_eq!(r["code"], "invalid_argument");
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": "01ARZ3NDEKTSV4RRFFQ69G5FAV" }),
    )
    .await;
    assert_eq!(r["code"], "job_not_found");

    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for(&e.server, &id, &["committed"]).await;
    let r = refused(
        &e.server,
        "ingest_file",
        json!({ "path": arg(&file), "resume_job": id }),
    )
    .await;
    assert_eq!(r["code"], "job_not_interrupted");
}

#[tokio::test]
async fn file_jobs_count_toward_max_open_jobs_and_batches_cannot_be_sent_to_them() {
    let e = env_with(|c| {
        c.file_batch_triples = 1;
        c.max_open_jobs = 1;
    });
    let file = write_nt(e.dir.path(), "busy.nt", 6_000);
    let id = ok(&e.server, "ingest_file", json!({ "path": arg(&file) })).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = refused(&e.server, "ingest_begin", json!({})).await;
    assert_eq!(r["code"], "too_many_open_jobs");
    // A running job takes no batches from the agent: its runner owns the sequence.
    let r = refused(
        &e.server,
        "ingest_batch",
        json!({ "job_id": id, "seq": 9_999, "data": "<urn:a> <urn:p> <urn:b> .\n" }),
    )
    .await;
    assert_eq!(r["code"], "job_not_open");
    ok(&e.server, "ingest_abort", json!({ "job_id": id })).await;
    wait_for(&e.server, &id, &["interrupted", "committed"]).await;
}
