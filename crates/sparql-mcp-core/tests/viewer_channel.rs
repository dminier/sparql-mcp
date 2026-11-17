//! Integration test for the viewer side-channel (Phase 1 exit criterion,
//! `docs/designs/sparql-mcp-viewer.md`): snapshot first, events in order,
//! the connection closed on lag, and a new snapshot after `project_switch`.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rmcp::object;
use serde_json::Value;
use sparql_mcp::application::event_sink::{BroadcastEventSink, EventSink, StoreEvent};
use sparql_mcp::application::tools::{ontology, project, sparql};
use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::{serve_viewer_channel, OxigraphAdapter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::time::timeout;

type Reader = BufReader<tokio::net::tcp::OwnedReadHalf>;

async fn listen_ephemeral() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

async fn connect_and_request_snapshot(
    addr: SocketAddr,
) -> (Reader, tokio::net::tcp::OwnedWriteHalf) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (read_half, mut write_half) = stream.into_split();
    write_half
        .write_all(b"{\"cmd\":\"snapshot\"}\n")
        .await
        .unwrap();
    (BufReader::new(read_half), write_half)
}

async fn recv_json(reader: &mut Reader) -> Value {
    let mut line = String::new();
    timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("timed out waiting for a line")
        .unwrap();
    serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad JSON line {line:?}: {e}"))
}

#[tokio::test]
async fn snapshot_then_insert_and_changed_events_in_order() {
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let active_graph = Arc::new(RwLock::new("urn:project:default".to_string()));
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let sink = BroadcastEventSink(tx);

    let (listener, addr) = listen_ephemeral().await;
    tokio::spawn(serve_viewer_channel(
        listener,
        store.clone(),
        active_graph.clone(),
        rx,
    ));

    let (mut reader, _writer) = connect_and_request_snapshot(addr).await;

    // 1. Snapshot first, empty (nothing loaded yet).
    let snap = recv_json(&mut reader).await;
    assert_eq!(snap["snapshot"]["graph"], "urn:project:default");
    assert_eq!(snap["snapshot"]["triples"].as_array().unwrap().len(), 0);

    // 2. load_ontology (ground data) → a precise Insert event.
    let turtle = "@prefix ex: <http://example.org/> .\nex:Alice a ex:Person .\n";
    ontology::load_ontology(&store, &object!({ "turtle": turtle }), &active_graph, &sink)
        .expect("load_ontology");
    let insert = recv_json(&mut reader).await;
    assert_eq!(insert["event"], "insert");
    assert_eq!(insert["graph"], "urn:project:default");
    assert!(
        !insert["triples"].as_array().unwrap().is_empty(),
        "{insert:?}"
    );

    // 3. update_sparql with a ground INSERT DATA → another precise Insert.
    sparql::update_sparql(
        &store,
        &object!({ "update": "INSERT DATA { <urn:s> <urn:p> <urn:o> . }" }),
        &active_graph,
        &sink,
    )
    .expect("update_sparql ground insert");
    let insert2 = recv_json(&mut reader).await;
    assert_eq!(insert2["event"], "insert");
    assert_eq!(
        insert2["triples"],
        serde_json::json!([["<urn:s>", "<urn:p>", "<urn:o>"]])
    );

    // 4. update_sparql with a pattern-based update → the coarse Changed
    //    fallback, in order after the two inserts above.
    sparql::update_sparql(
        &store,
        &object!({ "update": "DELETE { ?s ?p ?o } WHERE { ?s ?p ?o }" }),
        &active_graph,
        &sink,
    )
    .expect("update_sparql pattern delete");
    let changed = recv_json(&mut reader).await;
    assert_eq!(changed["event"], "changed");
    assert_eq!(changed["graph"], "urn:project:default");
    assert!(
        changed["triples_after"].as_u64().unwrap() < changed["triples_before"].as_u64().unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lagged_connection_is_closed() {
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let active_graph = Arc::new(RwLock::new("urn:project:default".to_string()));
    // Tiny capacity: easy to overflow deliberately.
    let (tx, rx) = tokio::sync::broadcast::channel(2);
    let sink = BroadcastEventSink(tx);

    let (listener, addr) = listen_ephemeral().await;
    tokio::spawn(serve_viewer_channel(
        listener,
        store.clone(),
        active_graph.clone(),
        rx,
    ));

    let (mut reader, _writer) = connect_and_request_snapshot(addr).await;
    let _snap = recv_json(&mut reader).await;

    // current_thread flavor + no `.await` in this loop: the connection
    // task (parked on `events.recv()`) cannot run until this function
    // yields, so all 10 sends land before it gets a chance to drain any
    // of them — guaranteeing it falls behind the capacity-2 channel.
    for i in 0..10u64 {
        sink.emit(StoreEvent::Changed {
            graph: "urn:project:default".into(),
            triples_before: i,
            triples_after: i + 1,
        });
    }

    // The connection should now observe `RecvError::Lagged` and close —
    // the client sees EOF rather than any further event line.
    let mut buf = String::new();
    let n = timeout(Duration::from_secs(5), reader.read_line(&mut buf))
        .await
        .expect("timed out waiting for the connection to close")
        .unwrap();
    assert_eq!(n, 0, "expected EOF (closed connection), got: {buf:?}");
}

#[tokio::test]
async fn project_switch_triggers_a_new_snapshot() {
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let active_graph = Arc::new(RwLock::new("urn:project:default".to_string()));
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let sink = BroadcastEventSink(tx);

    let (listener, addr) = listen_ephemeral().await;
    tokio::spawn(serve_viewer_channel(
        listener,
        store.clone(),
        active_graph.clone(),
        rx,
    ));

    let (mut reader, _writer) = connect_and_request_snapshot(addr).await;
    let snap = recv_json(&mut reader).await;
    assert_eq!(snap["snapshot"]["graph"], "urn:project:default");

    project::project_switch(&active_graph, &object!({ "id": "other" })).expect("project_switch");

    // The viewer channel polls the active graph every RESYNC_POLL; give it
    // a few polling intervals of headroom.
    let resnap = timeout(Duration::from_secs(2), recv_json(&mut reader))
        .await
        .expect("timed out waiting for the post-switch snapshot");
    assert_eq!(resnap["snapshot"]["graph"], "urn:project:other");

    let _ = &sink; // keep the sink (and its sender) alive for the test's duration
}
