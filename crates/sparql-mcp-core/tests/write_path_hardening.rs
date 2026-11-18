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
    inside: AtomicUsize,
    peak: AtomicUsize,
}

impl SlowStore {
    fn new(dir: &Path, delay: Duration) -> Self {
        Self {
            inner: OxigraphAdapter::open(dir).unwrap(),
            delay,
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
