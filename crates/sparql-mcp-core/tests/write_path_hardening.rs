//! Write-path hardening — Phase 0a exit criteria
//! (`docs/designs/sparql-mcp-write-path-hardening.md`).
//!
//! A `SlowStore` wraps a real RocksDB-backed adapter (the in-memory store
//! serializes writers on its own, so it could hide a missing gate) and makes
//! `update_with_outcome` slow, so these tests can tell whether the gate and
//! `spawn_blocking` really keep writes serialized and reads responsive.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use rmcp::model::{CallToolResult, JsonObject};
use serde_json::{json, Value};
use sparql_mcp::application::{build_stores, WriteGate};
use sparql_mcp::domain::{FileLoadResult, LoadOpts, QueryResult, SparqlStore, UpdateOutcome};
use sparql_mcp::infrastructure::{FsDocStore, OxigraphAdapter};
use sparql_mcp::mcp::server::SparqlMcpServer;

/// Delegates to a real adapter; `update_with_outcome` sleeps first and
/// records how many calls are inside it at once.
struct SlowStore {
    inner: OxigraphAdapter,
    delay: Duration,
    /// Sleep inside `write_quads`, to model a slow chunk.
    write_delay: Duration,
    inside: AtomicUsize,
    peak: AtomicUsize,
}

impl SlowStore {
    fn new(dir: &Path, delay: Duration) -> Self {
        Self {
            inner: OxigraphAdapter::open(dir).unwrap(),
            delay,
            write_delay: Duration::ZERO,
            inside: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }
}

impl SparqlStore for SlowStore {
    fn query(&self, sparql: &str) -> Result<QueryResult> {
        self.inner.query(sparql)
    }
    fn update(&self, sparql: &str) -> Result<()> {
        self.inner.update(sparql)
    }
    fn update_with_outcome(&self, sparql: &str) -> Result<UpdateOutcome> {
        let now = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let out = self.inner.update_with_outcome(sparql);
        self.inside.fetch_sub(1, Ordering::SeqCst);
        out
    }
    fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64> {
        self.inner.load_rdf(content, opts)
    }
    fn load_rdf_with_triples(
        &self,
        content: &[u8],
        opts: LoadOpts,
    ) -> Result<(u64, Vec<sparql_mcp::domain::StoredQuad>)> {
        self.inner.load_rdf_with_triples(content, opts)
    }
    fn parse_rdf(
        &self,
        content: &[u8],
        opts: &LoadOpts,
    ) -> Result<Vec<sparql_mcp::domain::StoredQuad>> {
        self.inner.parse_rdf(content, opts)
    }
    fn write_quads(&self, quads: &[sparql_mcp::domain::StoredQuad]) -> Result<u64> {
        std::thread::sleep(self.write_delay);
        self.inner.write_quads(quads)
    }
    fn load_rdf_file(&self, path: &Path, opts: LoadOpts, force: bool) -> Result<FileLoadResult> {
        self.inner.load_rdf_file(path, opts, force)
    }
    fn export_graph(&self, graph_iri: Option<&str>, dest: &Path) -> Result<usize> {
        self.inner.export_graph(graph_iri, dest)
    }
    fn list_graphs(&self) -> Result<Vec<String>> {
        self.inner.list_graphs()
    }
    fn triple_count(&self) -> Result<u64> {
        self.inner.triple_count()
    }
    fn load_ontology_dir(&self, dir: &Path) -> Result<usize> {
        self.inner.load_ontology_dir(dir)
    }
}

struct Fixture {
    server: SparqlMcpServer,
    slow: Arc<SlowStore>,
    _dir: tempfile::TempDir,
}

fn fixture(delay: Duration, wait: Duration) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let slow = Arc::new(SlowStore::new(&dir.path().join("store"), delay));
    let raw: Arc<dyn SparqlStore> = slow.clone();
    let (gated, privileged) = build_stores(raw, WriteGate::new(wait));
    let server = SparqlMcpServer::new(
        gated,
        privileged,
        Arc::new(FsDocStore::new(dir.path().join("docs"))),
        dir.path().join("ontology"),
        Arc::new(RwLock::new("urn:project:default".to_string())),
        None,
        dir.path().join("store"),
    );
    Fixture {
        server,
        slow,
        _dir: dir,
    }
}

fn obj(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

fn insert(n: u32) -> JsonObject {
    obj(json!({ "update": format!("INSERT DATA {{ <urn:s:{n}> <urn:p> <urn:o> }}") }))
}

fn body(res: &CallToolResult) -> Value {
    let text = res.content[0].as_text().unwrap().text.as_str();
    serde_json::from_str(text).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_read_is_not_blocked_by_a_running_write() {
    let f = fixture(Duration::from_millis(800), Duration::from_secs(5));
    let server = f.server.clone();
    let write =
        tokio::spawn(async move { server.dispatch_core("update_sparql", &insert(1)).await });
    tokio::time::sleep(Duration::from_millis(100)).await; // the write is now sleeping in the store

    let started = Instant::now();
    let read = f
        .server
        .dispatch_core("query_sparql", &obj(json!({ "query": "ASK { ?s ?p ?o }" })))
        .await
        .expect("known tool")
        .expect("query succeeds");
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "the read waited {:?} for the write",
        started.elapsed()
    );
    assert!(!read.is_error.unwrap_or(false));

    write.await.unwrap().unwrap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_writes_never_overlap() {
    let f = fixture(Duration::from_millis(150), Duration::from_secs(10));
    let writes: Vec<_> = (0..4)
        .map(|n| {
            let server = f.server.clone();
            tokio::spawn(async move { server.dispatch_core("update_sparql", &insert(n)).await })
        })
        .collect();
    for w in writes {
        let res = w.await.unwrap().unwrap().unwrap();
        assert!(!res.is_error.unwrap_or(false), "{:?}", body(&res));
    }
    assert_eq!(
        f.slow.peak.load(Ordering::SeqCst),
        1,
        "two writes ran inside the store at once"
    );
    assert_eq!(f.slow.triple_count().unwrap(), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn a_dropped_request_keeps_the_gate_until_its_write_ends() {
    // First write sleeps 1.5 s inside the store; a second write waits at most 1 s.
    let f = fixture(Duration::from_millis(1500), Duration::from_secs(1));

    // The request future is dropped after 50 ms, while the write is running.
    let first_args = insert(1);
    let first = f.server.dispatch_core("update_sparql", &first_args);
    let _ = tokio::time::timeout(Duration::from_millis(50), first).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The gate is still held by the abandoned write: the second one is refused
    // as data (isError), naming the holder, not as a JSON-RPC error.
    let second = f
        .server
        .dispatch_core("update_sparql", &insert(2))
        .await
        .expect("known tool")
        .expect("a refusal is a tool result, not a JSON-RPC error");
    assert_eq!(second.is_error, Some(true));
    let payload = body(&second);
    assert_eq!(payload["code"], "write_busy");
    assert_eq!(payload["holder"], "update_with_outcome");

    // Once the first write ends, writes succeed again.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let third = f
        .server
        .dispatch_core("update_sparql", &insert(3))
        .await
        .unwrap()
        .unwrap();
    assert!(!third.is_error.unwrap_or(false), "{:?}", body(&third));
    // Both the abandoned write and the third one landed; the refused one did not.
    assert_eq!(f.slow.triple_count().unwrap(), 2);
}

#[test]
fn a_chunked_load_lets_other_writers_in_between_chunks() {
    use sparql_mcp::domain::StoredQuad;

    // Four chunks of 300 ms each: 1.2 s in total. A second writer arriving
    // after 100 ms must be served after the current chunk, not after the whole
    // load (the gate is released between chunks, and handed out in order).
    let dir = tempfile::tempdir().unwrap();
    let mut slow = SlowStore::new(&dir.path().join("store"), Duration::ZERO);
    slow.write_delay = Duration::from_millis(300);
    let raw: Arc<dyn SparqlStore> = Arc::new(slow);
    let (gated, _privileged) = build_stores(raw, WriteGate::new(Duration::from_secs(10)));

    let quads: Vec<StoredQuad> = (0..4)
        .map(|n| StoredQuad {
            s: format!("<urn:s:{n}>"),
            p: "<urn:p>".into(),
            o: "<urn:o>".into(),
            g: Some("urn:g".into()),
        })
        .collect();
    let loader_store = gated.clone();
    let loader = std::thread::spawn(move || {
        let mut inserted = 0;
        for chunk in quads.chunks(1) {
            inserted += loader_store.write_quads(chunk).unwrap();
        }
        inserted
    });

    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    gated
        .update("INSERT DATA { <urn:other> <urn:p> <urn:o> }")
        .unwrap();
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(700),
        "the second writer waited {waited:?}: the gate was held for the whole load"
    );
    assert!(
        !loader.is_finished(),
        "the second writer only ran once the whole load was over"
    );
    assert_eq!(loader.join().unwrap(), 4);
    assert_eq!(gated.triple_count().unwrap(), 5);
}

// ── Phase 0c: guards, limits and dry_run, end to end through dispatch ─────────

use sparql_mcp::application::{build_stores_with, GuardConfig};

fn guarded_server(guards: GuardConfig) -> (SparqlMcpServer, Arc<dyn SparqlStore>) {
    let raw: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let (gated, privileged) =
        build_stores_with(raw.clone(), WriteGate::new(Duration::from_secs(5)), guards);
    let dir = std::env::temp_dir().join("smcp-guards");
    let server = SparqlMcpServer::new(
        gated,
        privileged,
        Arc::new(FsDocStore::new(dir.join("docs"))),
        dir.join("ontology"),
        Arc::new(RwLock::new("urn:project:default".to_string())),
        None,
        dir.join("store"),
    );
    (server, raw)
}

/// Run a tool and return its result as `(is_error, json body)`.
async fn run(server: &SparqlMcpServer, tool: &str, args: Value) -> (bool, Value) {
    let res = server
        .dispatch_core(tool, args.as_object().unwrap())
        .await
        .expect("known tool")
        .expect("a refusal is a tool result, not a JSON-RPC error");
    (res.is_error.unwrap_or(false), body(&res))
}

fn update(sparql: &str) -> Value {
    json!({ "update": sparql })
}

fn ask(raw: &Arc<dyn SparqlStore>, query: &str) -> bool {
    matches!(raw.query(query).unwrap(), QueryResult::Boolean(true))
}

const PROJECT_P: &str = "INSERT DATA { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }";

#[tokio::test]
async fn destructive_and_protected_updates_are_refused_as_data() {
    let (server, raw) = guarded_server(GuardConfig::default());
    raw.update(PROJECT_P).unwrap();

    // DROP ALL would reach every graph, protected ones included.
    let (is_err, b) = run(&server, "update_sparql", update("DROP ALL")).await;
    assert!(is_err);
    assert_eq!(b["code"], "graph_protected");

    // DROP of one ordinary graph needs the setting.
    let (is_err, b) = run(
        &server,
        "update_sparql",
        update("DROP GRAPH <urn:project:p>"),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "destructive_update_forbidden");
    assert!(b["message"]
        .as_str()
        .unwrap()
        .contains("allow_destructive_updates"));

    // The registry cannot be written through update_sparql.
    let (is_err, b) = run(
        &server,
        "update_sparql",
        update("INSERT DATA { GRAPH <urn:meta> { <urn:x> <urn:p> <urn:o> } }"),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "graph_protected");
    assert_eq!(b["message"], "graph <urn:meta> is protected");

    // Nothing was destroyed or written.
    assert!(ask(
        &raw,
        "ASK { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }"
    ));
    assert!(!ask(&raw, "ASK { GRAPH <urn:meta> { ?s ?p ?o } }"));
}

#[tokio::test]
async fn the_registry_is_still_writable_by_the_core_itself() {
    let (server, _raw) = guarded_server(GuardConfig::default());
    // project_create writes <urn:meta> through the privileged handle.
    let (is_err, b) = run(
        &server,
        "project_create",
        json!({ "id": "demo", "label": "Demo" }),
    )
    .await;
    assert!(!is_err, "{b}");
}

#[tokio::test]
async fn allow_destructive_updates_lets_drop_graph_run_but_not_protected_graphs() {
    let guards = GuardConfig {
        allow_destructive_updates: true,
        ..GuardConfig::default()
    };
    let (server, raw) = guarded_server(guards);
    raw.update(PROJECT_P).unwrap();

    let (is_err, b) = run(
        &server,
        "update_sparql",
        update("DROP GRAPH <urn:project:p>"),
    )
    .await;
    assert!(!is_err, "{b}");
    assert!(!ask(&raw, "ASK { GRAPH <urn:project:p> { ?s ?p ?o } }"));

    for sparql in ["DROP ALL", "CLEAR GRAPH <urn:meta>"] {
        let (is_err, b) = run(&server, "update_sparql", update(sparql)).await;
        assert!(is_err, "{sparql}");
        assert_eq!(b["code"], "graph_protected", "{sparql}");
    }
}

#[tokio::test]
async fn oversized_input_is_refused_before_anything_runs() {
    let guards = GuardConfig {
        max_update_bytes: 100,
        max_batch_bytes: 100,
        ..GuardConfig::default()
    };
    let (server, raw) = guarded_server(guards);

    let big = format!("INSERT DATA {{ <urn:s> <urn:p> \"{}\" }}", "x".repeat(200));
    let (is_err, b) = run(&server, "update_sparql", update(&big)).await;
    assert!(is_err);
    assert_eq!(b["code"], "limit_exceeded");
    assert_eq!(b["limit_key"], "max_update_bytes");
    assert_eq!(b["limit"], 100);
    assert!(b["value"].as_u64().unwrap() > 100);

    let turtle = format!("<urn:s> <urn:p> \"{}\" .", "x".repeat(200));
    let (is_err, b) = run(
        &server,
        "load_ontology",
        json!({ "turtle": turtle, "format": "ntriples" }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["limit_key"], "max_batch_bytes");

    assert_eq!(raw.triple_count().unwrap(), 0);
}

#[tokio::test]
async fn load_ontology_into_a_protected_graph_is_refused() {
    let (server, raw) = guarded_server(GuardConfig::default());
    let (is_err, b) = run(
        &server,
        "load_ontology",
        json!({ "turtle": "<urn:s> <urn:p> <urn:o> .", "format": "ntriples",
                "graph_iri": "urn:hkb:meta:loaded-files" }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "graph_protected");
    assert_eq!(raw.triple_count().unwrap(), 0);
}

#[tokio::test]
async fn an_oversized_file_is_refused_with_its_limit() {
    let guards = GuardConfig {
        max_load_file_bytes: 10,
        ..GuardConfig::default()
    };
    let (server, raw) = guarded_server(guards);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.nt");
    std::fs::write(&path, "<urn:s> <urn:p> <urn:o> .\n").unwrap();
    let (is_err, b) = run(
        &server,
        "load_ontology_file",
        json!({ "path": path.display().to_string() }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "limit_exceeded");
    assert_eq!(b["limit_key"], "max_load_file_bytes");
    assert_eq!(raw.triple_count().unwrap(), 0);
}

#[tokio::test]
async fn dry_run_classifies_and_reports_a_refusal_without_running() {
    let (server, raw) = guarded_server(GuardConfig::default());
    raw.update(PROJECT_P).unwrap();

    // An ordinary write: classified, not refused, and not executed.
    let (is_err, b) = run(
        &server,
        "update_sparql",
        json!({ "update": "INSERT DATA { GRAPH <urn:project:q> { <urn:a> <urn:p> <urn:b> } }",
                "dry_run": true }),
    )
    .await;
    assert!(!is_err);
    assert_eq!(b["dry_run"], true);
    assert_eq!(b["operations"], json!(["insert_data"]));
    assert_eq!(b["graphs"], json!(["urn:project:q"]));
    assert_eq!(b["destructive"], false);
    assert!(b["refused"].is_null());
    assert_eq!(raw.triple_count().unwrap(), 1, "a dry run wrote something");

    // A request that would be refused says so, and still does nothing.
    let (is_err, b) = run(
        &server,
        "update_sparql",
        json!({ "update": "DROP GRAPH <urn:project:p>", "dry_run": true }),
    )
    .await;
    assert!(!is_err);
    assert_eq!(b["destructive"], true);
    assert_eq!(b["refused"]["code"], "destructive_update_forbidden");
    assert_eq!(raw.triple_count().unwrap(), 1);

    // Invalid SPARQL is a parse error, not a successful dry run.
    let (is_err, b) = run(
        &server,
        "update_sparql",
        json!({ "update": "INSERT DATA {", "dry_run": true }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "parse_error");
}

// ── Phase 0c: file_roots ───────────────────────────────────────────────────────

use sparql_mcp::config::IngestConfig;

/// A server confined to `<tmp>/root`, plus the directory outside it.
fn confined_server() -> (
    SparqlMcpServer,
    Arc<dyn SparqlStore>,
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let cfg = IngestConfig {
        file_roots: Some(vec![root.clone()]),
        ..IngestConfig::default()
    };
    let (server, raw) = guarded_server(GuardConfig::from_config(&cfg).unwrap());
    (server, raw, dir, root, outside)
}

const ONE_TRIPLE: &str = "<urn:s> <urn:p> <urn:o> .\n";

#[tokio::test]
async fn a_file_inside_file_roots_loads_and_one_outside_is_refused() {
    let (server, raw, _dir, root, outside) = confined_server();
    let inside = root.join("in.nt");
    let elsewhere = outside.join("out.nt");
    std::fs::write(&inside, ONE_TRIPLE).unwrap();
    std::fs::write(&elsewhere, ONE_TRIPLE).unwrap();

    let (is_err, b) = run(
        &server,
        "load_ontology_file",
        json!({ "path": elsewhere.display().to_string() }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "path_outside_roots");
    assert_eq!(raw.triple_count().unwrap(), 0, "a refused file was loaded");

    let (is_err, b) = run(
        &server,
        "load_ontology_file",
        json!({ "path": inside.display().to_string() }),
    )
    .await;
    assert!(!is_err, "{b}");
    assert_eq!(b["inserted_new"], 1);

    // `..` out of the root is refused too.
    let sneaky = root.join("..").join("outside").join("out.nt");
    let (is_err, b) = run(
        &server,
        "load_ontology_file",
        json!({ "path": sneaky.display().to_string() }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "path_outside_roots");
}

#[tokio::test]
async fn export_graph_cannot_write_outside_file_roots() {
    let (server, raw, _dir, root, outside) = confined_server();
    raw.update(PROJECT_P).unwrap();

    let target = outside.join("dump.ttl");
    let (is_err, b) = run(
        &server,
        "export_graph",
        json!({ "graph_iri": "urn:project:p", "path": target.display().to_string() }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "path_outside_roots");
    assert!(!target.exists(), "the export wrote outside the root");

    let ok_target = root.join("exports").join("dump.ttl");
    let (is_err, b) = run(
        &server,
        "export_graph",
        json!({ "graph_iri": "urn:project:p", "path": ok_target.display().to_string() }),
    )
    .await;
    assert!(!is_err, "{b}");
    assert!(ok_target.exists());
}

#[tokio::test]
async fn without_file_roots_any_path_still_loads() {
    // Compatibility: file_roots unset keeps today's behaviour (with a warning).
    let (server, raw) = guarded_server(GuardConfig::default());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("any.nt");
    std::fs::write(&file, ONE_TRIPLE).unwrap();
    let (is_err, b) = run(
        &server,
        "load_ontology_file",
        json!({ "path": file.display().to_string() }),
    )
    .await;
    assert!(!is_err, "{b}");
    // The data landed (the ledger entry adds more quads, so do not count them).
    assert_eq!(b["inserted_new"], 1);
    assert!(ask(
        &raw,
        "ASK { GRAPH <urn:project:default> { <urn:s> <urn:p> <urn:o> } }"
    ));
}

#[cfg(windows)]
#[tokio::test]
async fn windows_unc_and_stream_paths_are_refused_without_touching_the_network() {
    let (server, raw, _dir, root, _outside) = confined_server();
    let started = Instant::now();
    for path in [
        r"\\attacker.invalid\share\data.nt".to_string(),
        r"\\.\pipe\x".to_string(),
        format!(r"{}:hidden", root.join("in.nt").display()),
    ] {
        let (is_err, b) = run(&server, "load_ontology_file", json!({ "path": path })).await;
        assert!(is_err, "{path}");
        assert_eq!(b["code"], "path_rejected", "{path}");
    }
    // A UNC path that is resolved would block on a DNS/SMB timeout: refusing
    // it lexically is immediate.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "refusing UNC paths took {:?}",
        started.elapsed()
    );
    assert_eq!(raw.triple_count().unwrap(), 0);
}

#[cfg(windows)]
#[tokio::test]
async fn a_junction_inside_the_root_cannot_reach_outside_through_the_tool() {
    let (server, raw, _dir, root, outside) = confined_server();
    std::fs::write(outside.join("out.nt"), ONE_TRIPLE).unwrap();
    let link = root.join("link");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");

    let through = link.join("out.nt");
    let (is_err, b) = run(
        &server,
        "load_ontology_file",
        json!({ "path": through.display().to_string() }),
    )
    .await;
    assert!(is_err);
    assert_eq!(b["code"], "path_outside_roots");
    assert_eq!(raw.triple_count().unwrap(), 0);
}
