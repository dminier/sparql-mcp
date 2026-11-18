//! CUJ 4 smoke test (Phase 2 exit criterion): load the ontology, insert
//! instances, delete one with a pattern-based update, and confirm the
//! client-side connection + graph projection track the server correctly
//! with no Bevy app involved.

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rmcp::object;
use sparql_mcp::application::event_sink::BroadcastEventSink;
use sparql_mcp::application::tools::{ontology, sparql};
use sparql_mcp::application::viewer_control::{ViewerControl, ViewerOp, ViewerOutcome};
use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::{serve_viewer_channel, OxigraphAdapter, ViewerHub};
use sparql_mcp_viewer::connection::{self, ConnState, ViewerMsg};
use sparql_mcp_viewer::graph::{NodeKind, ViewerGraph};
use tokio::net::TcpListener;

/// Drain `rx`, applying content messages to `graph` and recording every
/// state transition into `states`, until `pred(graph)` holds or `timeout`
/// elapses. Uses non-blocking `try_recv` plus an async sleep (never a
/// blocking `recv_timeout`): this test's tokio runtime also has to drive
/// `serve_viewer_channel`'s spawned task, and a blocking call here would
/// starve it.
async fn drain_until(
    rx: &crossbeam_channel::Receiver<ViewerMsg>,
    graph: &mut Option<ViewerGraph>,
    states: &mut Vec<ConnState>,
    timeout: Duration,
    pred: impl Fn(&ViewerGraph) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(ViewerMsg::Snapshot(snapshot)) => {
                let (rebuilt, _remap) = ViewerGraph::from_snapshot(&snapshot, graph.as_ref());
                *graph = Some(rebuilt);
            }
            Ok(ViewerMsg::Event(event)) => {
                if let Some(g) = graph.as_mut() {
                    g.apply_event(&event);
                }
            }
            Ok(ViewerMsg::State(state)) => states.push(state),
            Ok(ViewerMsg::Request(_)) => {}
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
        if graph.as_ref().is_some_and(&pred) {
            return true;
        }
    }
    false
}

fn live_node_count(graph: &ViewerGraph) -> usize {
    graph.nodes.iter().filter(|n| n.is_some()).count()
}

fn class_node_count(graph: &ViewerGraph) -> usize {
    graph
        .nodes
        .iter()
        .filter(|n| matches!(n, Some(node) if node.kind == NodeKind::Class))
        .count()
}

#[tokio::test]
async fn cuj4_smoke_test_without_a_window() {
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let active_graph = Arc::new(RwLock::new("urn:project:default".to_string()));
    let (tx, server_rx) = tokio::sync::broadcast::channel(64);
    let sink = BroadcastEventSink(tx);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_viewer_channel(
        listener,
        store.clone(),
        active_graph.clone(),
        server_rx,
        ViewerHub::new(),
    ));

    // Step 1: load the ontology (ground data — a precise Insert event).
    let ttl_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ontology/1-smc.ttl");
    let turtle = std::fs::read_to_string(&ttl_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", ttl_path.display()));
    ontology::load_ontology(
        &store,
        &object!({ "turtle": turtle, "graph_iri": "urn:project:default" }),
        &active_graph,
        &sink,
    )
    .expect("load_ontology");

    let (client_rx, _shutdown) = connection::spawn(addr, Duration::from_secs(5));
    let mut graph: Option<ViewerGraph> = None;
    let mut states = Vec::new();

    assert!(
        drain_until(
            &client_rx,
            &mut graph,
            &mut states,
            Duration::from_secs(5),
            |g| { live_node_count(g) > 0 }
        )
        .await,
        "timed out waiting for the ontology snapshot"
    );
    // smc:Project (owl:Class) plus its four properties (owl:DatatypeProperty
    // x3, owl:ObjectProperty x1) are all class-kind nodes.
    assert_eq!(
        class_node_count(graph.as_ref().unwrap()),
        5,
        "smc:Project + its 4 properties should classify as Class"
    );

    // Step 2: insert three smc:Project instances → 3 more instance nodes,
    // 3 Type edges, all pointed at the already-existing smc:Project node.
    let nodes_before_insert = live_node_count(graph.as_ref().unwrap());
    let edges_before_insert = graph.as_ref().unwrap().edges.len();
    let insert = "INSERT DATA { GRAPH <urn:project:default> {
        <urn:project:p1> a <https://sparql-mcp.dev/ns#Project> .
        <urn:project:p2> a <https://sparql-mcp.dev/ns#Project> .
        <urn:project:p3> a <https://sparql-mcp.dev/ns#Project> .
    } }";
    sparql::update_sparql(&store, &object!({ "update": insert }), &active_graph, &sink)
        .expect("update_sparql ground insert");

    assert!(
        drain_until(
            &client_rx,
            &mut graph,
            &mut states,
            Duration::from_secs(5),
            |g| { live_node_count(g) >= nodes_before_insert + 3 }
        )
        .await,
        "timed out waiting for the 3 inserted instances"
    );
    let g = graph.as_ref().unwrap();
    assert_eq!(live_node_count(g), nodes_before_insert + 3);
    assert_eq!(g.edges.len(), edges_before_insert + 3, "3 new Type edges");

    // Step 3: a pattern-based DELETE WHERE → a coarse `changed` event, which
    // reconnects without backoff: Connecting, then AwaitSnapshot, then
    // Streaming again, and the fresh snapshot already reflects the delete.
    sparql::update_sparql(
        &store,
        &object!({ "update": "DELETE WHERE { GRAPH <urn:project:default> { <urn:project:p1> ?p ?o } }" }),
        &active_graph,
        &sink,
    )
    .expect("update_sparql pattern delete");

    let nodes_before_delete = live_node_count(graph.as_ref().unwrap());
    assert!(
        drain_until(
            &client_rx,
            &mut graph,
            &mut states,
            Duration::from_secs(5),
            |g| { live_node_count(g) == nodes_before_delete - 1 }
        )
        .await,
        "timed out waiting for the rebuild after the pattern delete"
    );

    // The rebuild predicate above is satisfied by the second snapshot,
    // which arrives slightly before the Streaming transition that follows
    // it — give that transition a brief chance to land too.
    let state_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < state_deadline
        && states
            .iter()
            .filter(|s| **s == ConnState::Streaming)
            .count()
            < 2
    {
        match client_rx.try_recv() {
            Ok(ViewerMsg::State(s)) => states.push(s),
            Ok(_) => {}
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }

    let streaming_count = states
        .iter()
        .filter(|s| **s == ConnState::Streaming)
        .count();
    let connecting_count = states
        .iter()
        .filter(|s| **s == ConnState::Connecting)
        .count();
    let await_snapshot_count = states
        .iter()
        .filter(|s| **s == ConnState::AwaitSnapshot)
        .count();
    assert!(
        streaming_count >= 2 && connecting_count >= 2 && await_snapshot_count >= 2,
        "expected a second Connecting → AwaitSnapshot → Streaming cycle (the \
         reconnect-without-backoff after the `changed` event); got {states:?}"
    );

    let final_instance_count = graph
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .filter(|n| matches!(n, Some(node) if node.term.starts_with("<urn:project:p")))
        .count();
    assert_eq!(
        final_instance_count, 2,
        "2 instance nodes remain after deleting one of three"
    );
}

/// Kills the child process when the test ends, however it ends.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn the_core_reads_the_status_of_a_real_headless_viewer_and_sees_its_own_writes_at_once() {
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let active_graph = Arc::new(RwLock::new("urn:project:default".to_string()));
    let (tx, server_rx) = tokio::sync::broadcast::channel(64);
    let sink = BroadcastEventSink(tx);
    let hub = ViewerHub::new();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_viewer_channel(
        listener,
        store.clone(),
        active_graph.clone(),
        server_rx,
        hub.clone(),
    ));
    let ttl_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ontology/1-smc.ttl");
    let turtle = std::fs::read_to_string(&ttl_path).unwrap();
    ontology::load_ontology(
        &store,
        &object!({ "turtle": turtle, "graph_iri": "urn:project:default" }),
        &active_graph,
        &sink,
    )
    .unwrap();

    // The real binary, without a window.
    let _child = Child(
        std::process::Command::new(env!("CARGO_BIN_EXE_sparql-mcp-viewer"))
            .args(["--headless", "--connect", &addr.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("starting the headless viewer"),
    );

    let ask = |hub: Arc<ViewerHub>| async move {
        hub.request(ViewerOp::Status {}, Instant::now() + Duration::from_secs(5))
            .await
    };
    // The first connection takes a moment (the process starts, then connects).
    let started = Instant::now();
    let first = loop {
        match ask(hub.clone()).await {
            ViewerOutcome::Applied { result, user, .. } => break (result, user),
            ViewerOutcome::NotConnected | ViewerOutcome::Timeout => {
                assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "the viewer never connected"
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            other => panic!("{other:?}"),
        }
    };
    let (result, user) = first;
    assert_eq!(result["graph"], "urn:project:default");
    assert_eq!(result["connection"], "streaming");
    assert_eq!(hub.info().instance.as_deref(), result["instance"].as_str());
    assert_eq!(user.epoch, 0);
    let nodes_before = result["nodes"].as_u64().unwrap();
    assert!(nodes_before > 0, "the ontology is loaded: {result}");

    // Three inserts, then a request at once: the viewer has applied the events
    // that were emitted before the request when it answers.
    let insert = "INSERT DATA { GRAPH <urn:project:default> {
        <urn:project:q1> a <https://sparql-mcp.dev/ns#Project> .
        <urn:project:q2> a <https://sparql-mcp.dev/ns#Project> .
        <urn:project:q3> a <https://sparql-mcp.dev/ns#Project> .
    } }";
    sparql::update_sparql(&store, &object!({ "update": insert }), &active_graph, &sink).unwrap();
    match ask(hub.clone()).await {
        ViewerOutcome::Applied { result, .. } => {
            assert_eq!(
                result["nodes"].as_u64().unwrap(),
                nodes_before + 3,
                "{result}"
            );
        }
        other => panic!("{other:?}"),
    }

    // A headless viewer has no window to highlight in, and says so.
    let show = ViewerOp::Show {
        iris: vec!["urn:project:q1".into()],
        mode: sparql_mcp::application::viewer_control::ShowMode::Highlight,
        focus: false,
        select: None,
        if_user_epoch: None,
    };
    match hub
        .request(show, Instant::now() + Duration::from_secs(5))
        .await
    {
        ViewerOutcome::Rejected { code, .. } => assert_eq!(code, "unsupported_headless"),
        other => panic!("{other:?}"),
    }
    let camera = ViewerOp::Camera {
        action: sparql_mcp::application::viewer_control::CameraAction::FrameAll,
        iris: vec![],
        factor: None,
        duration_ms: 0,
        if_user_epoch: None,
    };
    match hub
        .request(camera, Instant::now() + Duration::from_secs(5))
        .await
    {
        ViewerOutcome::Rejected { code, .. } => assert_eq!(code, "unsupported_headless"),
        other => panic!("{other:?}"),
    }
}
