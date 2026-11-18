//! Viewer control, phase 1 exit criterion
//! (`docs/designs/sparql-mcp-viewer-agent-control.md`, *Delivery phases*): the
//! [`ViewerHub`] and the side-channel, driven by a fake viewer over real TCP.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Value};
use sparql_mcp::application::event_sink::{BroadcastEventSink, EventSink, StoreEvent};
use sparql_mcp::application::tools::viewer;
use sparql_mcp::application::viewer_control::{
    ListenerState, UserActivity, ViewerControl, ViewerOp, ViewerOutcome,
};
use sparql_mcp::domain::{InputFormat, LoadOpts, SparqlStore};
use sparql_mcp::infrastructure::{serve_viewer_channel, OxigraphAdapter, ViewerHub};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, LinesCodec};

const GRAPH: &str = "urn:project:default";

struct Core {
    addr: SocketAddr,
    hub: Arc<ViewerHub>,
    sink: BroadcastEventSink,
    store: Arc<dyn SparqlStore>,
}

async fn core_with(store: Arc<dyn SparqlStore>, capacity: usize) -> Core {
    let hub = ViewerHub::new();
    // `spawn_viewer_channel` records this once it has bound; here the test binds.
    hub.set_listener(ListenerState::Listening);
    let (tx, rx) = tokio::sync::broadcast::channel(capacity);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_viewer_channel(
        listener,
        store.clone(),
        Arc::new(RwLock::new(GRAPH.to_string())),
        rx,
        hub.clone(),
    ));
    Core {
        addr,
        hub,
        sink: BroadcastEventSink(tx),
        store,
    }
}

async fn core() -> Core {
    core_with(Arc::new(OxigraphAdapter::open_in_memory().unwrap()), 64).await
}

/// A store whose snapshot is several MiB: more than the sockets buffer.
fn big_store(triples: usize) -> Arc<dyn SparqlStore> {
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let data: String = (0..triples)
        .map(|i| format!("<urn:node:{i}> <urn:p> \"a fairly long literal value for node number {i}, padding\" .\n"))
        .collect();
    store
        .load_rdf(
            data.as_bytes(),
            LoadOpts {
                format: InputFormat::NTriples,
                graph_iri: Some(GRAPH.to_string()),
                base_iri: None,
            },
        )
        .unwrap();
    store
}

/// The viewer end of a connection.
struct FakeViewer {
    frames: FramedRead<tokio::net::tcp::OwnedReadHalf, LinesCodec>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl FakeViewer {
    /// Connect with the given handshake and read the first snapshot.
    async fn connect(addr: SocketAddr, handshake: Value) -> Self {
        let mut v = Self::connect_without_reading(addr, handshake).await;
        let snap = v.next().await;
        assert!(snap.get("snapshot").is_some(), "first line: {snap}");
        v
    }

    async fn connect_without_reading(addr: SocketAddr, handshake: Value) -> Self {
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut writer) = stream.into_split();
        writer
            .write_all(format!("{handshake}\n").as_bytes())
            .await
            .unwrap();
        Self {
            // The viewer reads without a cap: its snapshots are large.
            frames: FramedRead::new(read_half, LinesCodec::new()),
            writer,
        }
    }

    async fn controllable(addr: SocketAddr, instance: &str) -> Self {
        Self::connect(
            addr,
            json!({ "cmd": "snapshot", "control": 1, "instance": instance }),
        )
        .await
    }

    async fn next(&mut self) -> Value {
        let line = timeout(Duration::from_secs(10), self.frames.next())
            .await
            .expect("timed out waiting for a line")
            .expect("connection closed")
            .expect("read error");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad line {line:.80}: {e}"))
    }

    /// The next line is a request; return its id and op.
    async fn next_request(&mut self) -> (u64, String) {
        let v = self.next().await;
        let r = v
            .get("request")
            .unwrap_or_else(|| panic!("expected a request, got {v}"));
        (
            r["id"].as_u64().unwrap(),
            r["op"].as_str().unwrap().to_string(),
        )
    }

    async fn send(&mut self, value: Value) {
        self.writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn respond(&mut self, id: u64, result: Value) {
        self.send(json!({ "response": {
            "id": id, "ok": true, "result": result,
            "user": { "active": false, "epoch": 3, "overrode_last_agent_command": false }
        }}))
        .await;
    }

    async fn is_closed(&mut self) -> bool {
        matches!(
            timeout(Duration::from_millis(300), self.frames.next()).await,
            Ok(None) | Ok(Some(Err(_)))
        )
    }
}

fn soon(secs: u64) -> Instant {
    Instant::now() + Duration::from_secs(secs)
}

fn status() -> ViewerOp {
    ViewerOp::Status {}
}

#[tokio::test]
async fn a_status_request_round_trips_with_the_user_block() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });

    let (id, op) = v.next_request().await;
    assert_eq!(op, "status");
    v.respond(id, json!({ "nodes": 12, "edges": 30 })).await;

    match call.await.unwrap() {
        ViewerOutcome::Applied {
            result,
            user,
            reason,
        } => {
            assert_eq!(result["nodes"], 12);
            assert_eq!(
                user,
                UserActivity {
                    active: false,
                    epoch: 3,
                    overrode_last_agent_command: false
                }
            );
            assert_eq!(reason, None);
        }
        other => panic!("{other:?}"),
    }
    // A second request gets a new id.
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let (id2, _) = v.next_request().await;
    assert_ne!(id, id2);
    v.respond(id2, json!({})).await;
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));
}

#[tokio::test]
async fn the_viewer_can_refuse_a_command_and_the_refusal_is_reported() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let (id, _) = v.next_request().await;
    v.send(json!({ "response": { "id": id, "ok": false,
        "error": { "code": "unsupported_headless", "message": "no window" } } }))
        .await;
    assert_eq!(
        call.await.unwrap(),
        ViewerOutcome::Rejected {
            code: "unsupported_headless".into(),
            message: "no window".into()
        }
    );
}

#[tokio::test]
async fn with_no_viewer_the_answer_is_not_connected_at_once() {
    let c = core().await;
    let started = Instant::now();
    assert_eq!(
        c.hub.request(status(), soon(5)).await,
        ViewerOutcome::NotConnected
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    let info = c.hub.info();
    assert_eq!(
        (info.listener, info.instance, info.controllable_connections),
        (ListenerState::Listening, None, 0)
    );
}

#[tokio::test]
async fn a_viewer_that_never_answers_gives_timeout_and_a_late_answer_is_ignored() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    let started = Instant::now();
    let out = c
        .hub
        .request(status(), Instant::now() + Duration::from_millis(300))
        .await;
    assert_eq!(out, ViewerOutcome::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
    // The viewer answers after all: nothing breaks, and the next request works.
    let (late, _) = v.next_request().await;
    v.respond(late, json!({})).await;
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let (id, _) = v.next_request().await;
    assert_ne!(id, late);
    v.respond(id, json!({ "ok": true })).await;
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));

    // The tool reports it as unknown, not as failed.
    let envelope = viewer::envelope(ViewerOutcome::Timeout);
    assert!(envelope["applied"].is_null());
}

#[tokio::test]
async fn a_dropped_connection_then_a_reconnect_of_the_same_instance_retries_the_request() {
    let c = core().await;
    let v = FakeViewer::controllable(c.addr, "viewer-a").await;
    let mut v = v;
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(8)).await });

    // The request reaches the viewer, which dies before answering.
    let (first, _) = v.next_request().await;
    drop(v);

    // It comes back (the same instance) and receives the request again.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut v2 = FakeViewer::controllable(c.addr, "viewer-a").await;
    let (second, op) = v2.next_request().await;
    assert_eq!(op, "status");
    assert_ne!(first, second, "a retry gets a new id");
    v2.respond(second, json!({ "back": true })).await;
    match call.await.unwrap() {
        ViewerOutcome::Applied { result, .. } => assert_eq!(result["back"], true),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_dropped_connection_that_does_not_come_back_ends_not_connected_within_the_deadline() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    let hub = c.hub.clone();
    let started = Instant::now();
    let call = tokio::spawn(async move {
        hub.request(status(), Instant::now() + Duration::from_millis(800))
            .await
    });
    v.next_request().await;
    drop(v);
    assert_eq!(call.await.unwrap(), ViewerOutcome::NotConnected);
    assert!(started.elapsed() < Duration::from_secs(3));
    // Once the connection is gone, the hub no longer lists it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(c.hub.info().controllable_connections, 0);
}

#[tokio::test]
async fn an_event_emitted_before_a_request_reaches_the_viewer_first() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    for n in 0..20 {
        c.sink.emit(StoreEvent::Changed {
            graph: GRAPH.into(),
            triples_before: n,
            triples_after: n + 1,
        });
    }
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let mut events = 0;
    loop {
        let line = v.next().await;
        if line.get("request").is_some() {
            v.respond(line["request"]["id"].as_u64().unwrap(), json!({}))
                .await;
            break;
        }
        assert_eq!(line["event"], "changed", "{line}");
        events += 1;
    }
    assert_eq!(
        events, 20,
        "every event emitted before the request must precede it"
    );
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));
}

#[tokio::test]
async fn a_mid_stream_snapshot_command_is_answered_on_the_same_connection() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    c.store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{GRAPH}> {{ <urn:a> <urn:p> <urn:b> }} }}"
        ))
        .unwrap();
    // What a viewer does on a `changed` event: ask again, without reconnecting.
    v.send(json!({ "cmd": "snapshot" })).await;
    let snap = v.next().await;
    assert_eq!(
        snap["snapshot"]["triples"].as_array().unwrap().len(),
        1,
        "{snap}"
    );
    // And control still works on that connection.
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let (id, _) = v.next_request().await;
    v.respond(id, json!({})).await;
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));
}

#[tokio::test]
async fn a_controllable_viewer_that_falls_behind_gets_a_new_snapshot_and_keeps_its_connection() {
    // A snapshot bigger than the socket buffers, a tiny event channel, and a
    // viewer that does not read: the writer blocks, the events overflow.
    let c = core_with(big_store(150_000), 4).await;
    let mut v = FakeViewer::connect_without_reading(
        c.addr,
        json!({ "cmd": "snapshot", "control": 1, "instance": "viewer-a" }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    for n in 0..50 {
        c.sink.emit(StoreEvent::Changed {
            graph: GRAPH.into(),
            triples_before: n,
            triples_after: n + 1,
        });
    }
    // Now it reads: the first snapshot, then (after the lag) a second one.
    let first = v.next().await;
    assert!(first.get("snapshot").is_some());
    let second = v.next().await;
    assert!(
        second.get("snapshot").is_some(),
        "after a lag a controllable viewer is re-snapshotted, not closed: {:.100}",
        second.to_string()
    );
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(8)).await });
    let (id, _) = loop {
        let line = v.next().await;
        if line.get("request").is_some() {
            break (line["request"]["id"].as_u64().unwrap(), ());
        }
    };
    v.respond(id, json!({})).await;
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));
}

#[tokio::test]
async fn a_viewer_without_control_is_never_sent_a_request_and_still_closes_on_a_lag() {
    let c = core_with(big_store(150_000), 4).await;
    let mut old = FakeViewer::connect_without_reading(c.addr, json!({ "cmd": "snapshot" })).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        c.hub.request(status(), soon(2)).await,
        ViewerOutcome::NotConnected
    );
    for n in 0..50 {
        c.sink.emit(StoreEvent::Changed {
            graph: GRAPH.into(),
            triples_before: n,
            triples_after: n + 1,
        });
    }
    let first = old.next().await;
    assert!(first.get("snapshot").is_some());
    assert!(
        old.is_closed().await,
        "an old viewer keeps the base behaviour: closed on a lag"
    );
}

#[tokio::test]
async fn a_large_answer_is_read_while_the_core_writes_a_large_snapshot() {
    let c = core_with(big_store(150_000), 64).await;
    let mut v = FakeViewer::connect_without_reading(
        c.addr,
        json!({ "cmd": "snapshot", "control": 1, "instance": "viewer-a" }),
    )
    .await;
    // The viewer first writes 6 MiB, without reading: the core is writing its
    // snapshot at the same time and nobody reads that yet. With one task doing
    // both, each side would wait for the other.
    let big = format!(
        "{}\n",
        json!({ "response": { "id": 999_999, "ok": true,
        "result": { "pad": "x".repeat(6 * 1024 * 1024) } } })
    );
    timeout(Duration::from_secs(20), v.writer.write_all(big.as_bytes()))
        .await
        .expect("the core stopped reading while it was writing")
        .unwrap();
    // A line over the 8 MiB cap is dropped; the connection survives it.
    let huge = format!("{}\n", "y".repeat(9 * 1024 * 1024));
    timeout(Duration::from_secs(20), v.writer.write_all(huge.as_bytes()))
        .await
        .expect("the core stopped reading an oversized line")
        .unwrap();
    // Now it reads the snapshot, and control works.
    let snap = v.next().await;
    assert!(snap.get("snapshot").is_some());
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(8)).await });
    let (id, _) = v.next_request().await;
    v.respond(id, json!({})).await;
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));
}

#[tokio::test]
async fn control_belongs_to_the_newest_instance_and_a_reconnect_does_not_move_it() {
    let c = core().await;
    let mut a = FakeViewer::controllable(c.addr, "viewer-a").await;
    let mut b = FakeViewer::controllable(c.addr, "viewer-b").await;
    assert_eq!(c.hub.info().instance.as_deref(), Some("viewer-b"));

    // Requests go to b.
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let (id, _) = b.next_request().await;
    b.respond(id, json!({ "who": "b" })).await;
    assert!(matches!(call.await.unwrap(), ViewerOutcome::Applied { .. }));

    // a reconnects: it does not take control from b.
    drop(a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    a = FakeViewer::controllable(c.addr, "viewer-a").await;
    assert_eq!(c.hub.info().instance.as_deref(), Some("viewer-b"));
    assert_eq!(c.hub.info().controllable_connections, 2);

    // If b goes away, a (still connected) takes over at the next request.
    drop(b);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let hub = c.hub.clone();
    let call = tokio::spawn(async move { hub.request(status(), soon(5)).await });
    let (id, _) = a.next_request().await;
    a.respond(id, json!({ "who": "a" })).await;
    match call.await.unwrap() {
        ViewerOutcome::Applied { result, .. } => assert_eq!(result["who"], "a"),
        other => panic!("{other:?}"),
    }
    assert_eq!(c.hub.info().instance.as_deref(), Some("viewer-a"));
}

#[tokio::test]
async fn a_bind_failure_is_recorded_for_viewer_status() {
    // Take a port, then ask the core to bind the same one.
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = taken.local_addr().unwrap();
    let hub = ViewerHub::new();
    let (_tx, rx) = tokio::sync::broadcast::channel(4);
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let result = sparql_mcp::infrastructure::spawn_viewer_channel(
        addr,
        store,
        Arc::new(RwLock::new(GRAPH.to_string())),
        rx,
        hub.clone(),
    )
    .await;
    assert!(result.is_err());
    assert!(matches!(hub.info().listener, ListenerState::BindFailed(_)));

    let out = viewer::dispatch("viewer_status", &Default::default(), hub.as_ref())
        .await
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_str(&out.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(v["viewer"], "not_connected");
    assert!(v["core"]["listener"]["bind_failed"].is_string(), "{v}");
}

#[tokio::test]
async fn the_status_tool_returns_the_envelope_with_what_the_viewer_said() {
    let c = core().await;
    let mut v = FakeViewer::controllable(c.addr, "viewer-a").await;
    let hub = c.hub.clone();
    let call = tokio::spawn(async move {
        viewer::dispatch("viewer_status", &Default::default(), hub.as_ref()).await
    });
    let (id, _) = v.next_request().await;
    v.respond(id, json!({ "graph": GRAPH, "nodes": 5 })).await;
    let out = call.await.unwrap().unwrap().unwrap();
    let env: Value = serde_json::from_str(&out.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(env["viewer"], "connected");
    assert_eq!(env["applied"], true);
    assert_eq!(env["result"]["nodes"], 5);
    assert_eq!(env["user"]["epoch"], 3);
    assert_eq!(env["core"]["instance"], "viewer-a");
    assert_eq!(env["core"]["listener"], "listening");
}

#[tokio::test]
async fn only_a_controllable_viewers_snapshot_carries_the_control_echo() {
    let c = core().await;
    let mut controllable = FakeViewer::connect_without_reading(
        c.addr,
        json!({ "cmd": "snapshot", "control": 1, "instance": "viewer-a" }),
    )
    .await;
    assert_eq!(controllable.next().await["control"], 1);
    // No instance id, or `control` 0: not controllable, so no echo.
    for hello in [
        json!({ "cmd": "snapshot", "control": 1 }),
        json!({ "cmd": "snapshot", "control": 0, "instance": "x" }),
        json!({ "cmd": "snapshot" }),
    ] {
        let mut v = FakeViewer::connect_without_reading(c.addr, hello.clone()).await;
        assert!(v.next().await.get("control").is_none(), "{hello}");
    }
    assert_eq!(c.hub.info().controllable_connections, 1);
}
