//! Network client — no Bevy dependency. Runs the connection state machine
//! on its own thread with a `tokio` current-thread runtime, and pushes
//! decoded messages to the Bevy side over an `std`-friendly channel.
//!
//! A viewer started with [`spawn_controllable`] also tells the core that it can
//! be controlled (`"control":1` and a random `"instance"` id in its first line).
//! The core then sends `{"request": …}` lines, which arrive as
//! [`ViewerMsg::Request`], and the Bevy side answers through
//! [`Connection::responses`]. A core that understands this says so in its
//! snapshot line (`"control":1`); only then does the viewer resynchronise on
//! the same connection after a `changed` event instead of reconnecting, so a
//! request in flight is not lost. Against an older core it reconnects as before.
//!
//! Reading and writing never wait for each other: responses go to a writer task
//! of their own, so a large response (a screenshot) written while the core is
//! writing a large snapshot cannot deadlock the pair.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sparql_mcp::application::event_sink::{Snapshot, StoreEvent};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

/// A decoded line from the viewer side-channel, or a connection-state
/// transition — `plugin::drain_inbox` moves these into `FrameMessages`
/// each frame.
#[derive(Debug, Clone)]
pub enum ViewerMsg {
    Snapshot(Snapshot),
    Event(StoreEvent),
    State(ConnState),
    /// A command from the core, to be answered through [`Connection::responses`].
    Request(AgentRequest),
}

/// One command of the core: the `id` to echo, the `op` and its `args`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRequest {
    pub id: u64,
    pub op: String,
    pub args: Value,
}

/// The viewer's answer to one [`AgentRequest`], as the object that goes under
/// `"response"` on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentResponse(pub Value);

impl AgentResponse {
    /// A command that was applied. `user` is the `user` block every successful
    /// response carries.
    pub fn ok(id: u64, result: Value, user: Value) -> Self {
        Self(json!({ "id": id, "ok": true, "result": result, "user": user }))
    }

    /// Like [`Self::ok`], with a `reason` for a command applied only in part.
    pub fn ok_with_reason(id: u64, result: Value, user: Value, reason: &str) -> Self {
        Self(json!({ "id": id, "ok": true, "result": result, "user": user, "reason": reason }))
    }

    pub fn err(id: u64, code: &str, message: &str) -> Self {
        Self(json!({ "id": id, "ok": false, "error": { "code": code, "message": message } }))
    }

    fn line(&self) -> String {
        format!("{}\n", json!({ "response": self.0 }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnState {
    Connecting,
    AwaitSnapshot,
    Streaming,
    Backoff { retry_at: Instant },
}

/// Wraps a `tokio::sync::oneshot::Sender`. Dropping it (or calling
/// `shutdown`) tells the connection thread's runtime to stop.
pub struct ShutdownHandle(oneshot::Sender<()>);

impl ShutdownHandle {
    pub fn shutdown(self) {
        let _ = self.0.send(());
    }
}

/// A started connection that can be controlled.
pub struct Connection {
    pub rx: crossbeam_channel::Receiver<ViewerMsg>,
    pub shutdown: ShutdownHandle,
    /// Answers to [`ViewerMsg::Request`]. One not yet written when the
    /// connection closes is dropped: the core retries or gives up itself.
    pub responses: mpsc::UnboundedSender<AgentResponse>,
    /// This process's id, the same across reconnects.
    pub instance: String,
}

const AWAIT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// What a controllable connection needs besides the stream.
struct Control {
    instance: String,
    responses: mpsc::UnboundedReceiver<AgentResponse>,
}

/// Starts the connection on its own thread and returns a channel of
/// [`ViewerMsg`] plus a handle to stop it. `connect_timeout` bounds the
/// initial TCP `connect()`; the snapshot wait once connected has its own
/// fixed 30 s timeout (see the Technical Plan).
///
/// The viewer does not offer to be controlled; use [`spawn_controllable`].
pub fn spawn(
    addr: SocketAddr,
    connect_timeout: Duration,
) -> (crossbeam_channel::Receiver<ViewerMsg>, ShutdownHandle) {
    spawn_inner(addr, connect_timeout, None)
}

/// Like [`spawn`], and offers the core to control this viewer.
pub fn spawn_controllable(addr: SocketAddr, connect_timeout: Duration) -> Connection {
    let instance = new_instance_id();
    let (responses_tx, responses_rx) = mpsc::unbounded_channel();
    let (rx, shutdown) = spawn_inner(
        addr,
        connect_timeout,
        Some(Control {
            instance: instance.clone(),
            responses: responses_rx,
        }),
    );
    Connection {
        rx,
        shutdown,
        responses: responses_tx,
        instance,
    }
}

fn new_instance_id() -> String {
    let n: u128 = rand::rng().random();
    format!("{n:032x}")
}

fn spawn_inner(
    addr: SocketAddr,
    connect_timeout: Duration,
    control: Option<Control>,
) -> (crossbeam_channel::Receiver<ViewerMsg>, ShutdownHandle) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("sparql-mcp-viewer-connection".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("building the connection thread's tokio runtime");
            rt.block_on(run(addr, connect_timeout, tx, shutdown_rx, control));
        })
        .expect("spawning the connection thread");
    (rx, ShutdownHandle(shutdown_tx))
}

fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

fn set_state(tx: &crossbeam_channel::Sender<ViewerMsg>, state: ConnState) {
    let _ = tx.send(ViewerMsg::State(state));
}

type Frames = FramedRead<tokio::net::tcp::OwnedReadHalf, LinesCodec>;

/// The next line, or `None` when the connection ended or failed.
async fn next_line(frames: &mut Frames) -> Option<String> {
    match frames.next().await {
        Some(Ok(line)) => Some(line),
        Some(Err(e)) => {
            tracing::warn!(error = %e, "viewer channel read error");
            None
        }
        None => None,
    }
}

/// The next response to write, or never when this viewer cannot be controlled.
async fn next_response(control: &mut Option<Control>) -> Option<AgentResponse> {
    match control {
        Some(c) => c.responses.recv().await,
        None => std::future::pending().await,
    }
}

/// Writes lines until the channel closes or a write fails. A task of its own,
/// so that a slow write never stops the reading.
async fn write_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut lines: mpsc::UnboundedReceiver<String>,
) {
    while let Some(line) = lines.recv().await {
        if writer.write_all(line.as_bytes()).await.is_err() {
            return;
        }
    }
}

async fn run(
    addr: SocketAddr,
    connect_timeout: Duration,
    tx: crossbeam_channel::Sender<ViewerMsg>,
    mut shutdown_rx: oneshot::Receiver<()>,
    mut control: Option<Control>,
) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        if shutdown_rx.try_recv().is_ok() {
            return;
        }

        set_state(&tx, ConnState::Connecting);
        let stream = match tokio::time::timeout(connect_timeout, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => stream,
            _ => {
                backoff = enter_backoff(&tx, backoff, &mut shutdown_rx).await;
                if backoff_was_shutdown(&mut shutdown_rx) {
                    return;
                }
                continue;
            }
        };

        let (read_half, mut write_half) = stream.into_split();
        let hello = match &control {
            Some(c) => format!(
                "{{\"cmd\":\"snapshot\",\"control\":1,\"instance\":\"{}\"}}\n",
                c.instance
            ),
            None => "{\"cmd\":\"snapshot\"}\n".to_string(),
        };
        if write_half.write_all(hello.as_bytes()).await.is_err() {
            backoff = enter_backoff(&tx, backoff, &mut shutdown_rx).await;
            continue;
        }
        // Answers prepared for an earlier connection are of no use to the core now.
        if let Some(c) = control.as_mut() {
            while c.responses.try_recv().is_ok() {}
        }

        set_state(&tx, ConnState::AwaitSnapshot);
        let mut frames: Frames = FramedRead::new(read_half, LinesCodec::new());

        let first = tokio::time::timeout(AWAIT_SNAPSHOT_TIMEOUT, next_line(&mut frames)).await;
        // Whether the core said it understands control: only then is a resync
        // asked for on this connection rather than by reconnecting.
        let mut in_band_resync = false;
        let got_snapshot = match first {
            Ok(Some(line)) => match decode_line(&line) {
                Decoded::Snapshot(snap, control_ack) => {
                    in_band_resync = control.is_some() && control_ack;
                    let _ = tx.send(ViewerMsg::Snapshot(snap));
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !got_snapshot {
            backoff = enter_backoff(&tx, backoff, &mut shutdown_rx).await;
            continue;
        }

        set_state(&tx, ConnState::Streaming);
        backoff = INITIAL_BACKOFF; // reset on reaching Streaming

        let (lines_tx, lines_rx) = mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(write_loop(write_half, lines_rx));
        let mut awaiting_resync = false;
        let mut reconnect_without_backoff = false;
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    writer.abort();
                    return;
                }
                response = next_response(&mut control) => {
                    match response {
                        Some(r) => { let _ = lines_tx.send(r.line()); }
                        // The Bevy side is gone: nothing more to answer with.
                        None => control = None,
                    }
                }
                line = next_line(&mut frames) => {
                    let Some(line) = line else { break }; // EOF or error: reconnect with backoff
                    match decode_line(&line) {
                        Decoded::Snapshot(snap, _) => {
                            let _ = tx.send(ViewerMsg::Snapshot(snap));
                            if awaiting_resync {
                                awaiting_resync = false;
                                set_state(&tx, ConnState::Streaming);
                            }
                        }
                        Decoded::Event(event) => {
                            let is_changed = matches!(event, StoreEvent::Changed { .. });
                            let _ = tx.send(ViewerMsg::Event(event));
                            if is_changed {
                                if in_band_resync {
                                    // Ask again on this connection; the answer
                                    // comes as a new snapshot.
                                    awaiting_resync = true;
                                    set_state(&tx, ConnState::AwaitSnapshot);
                                    let _ = lines_tx.send("{\"cmd\":\"snapshot\"}\n".to_string());
                                } else {
                                    // Forwarded, then the connection is closed and
                                    // reopened at once, without backoff.
                                    reconnect_without_backoff = true;
                                    break;
                                }
                            }
                        }
                        Decoded::Request(request) => {
                            let _ = tx.send(ViewerMsg::Request(request));
                        }
                        Decoded::Malformed => {
                            tracing::warn!(line = %line.trim(), "malformed viewer channel line; skipped");
                        }
                    }
                }
            }
        }
        // Whatever is still queued for writing is dropped with the connection.
        writer.abort();

        if reconnect_without_backoff {
            continue;
        }
        backoff = enter_backoff(&tx, backoff, &mut shutdown_rx).await;
    }
}

async fn enter_backoff(
    tx: &crossbeam_channel::Sender<ViewerMsg>,
    current: Duration,
    shutdown_rx: &mut oneshot::Receiver<()>,
) -> Duration {
    let retry_at = Instant::now() + current;
    set_state(tx, ConnState::Backoff { retry_at });
    tokio::select! {
        _ = tokio::time::sleep(current) => {}
        _ = &mut *shutdown_rx => {}
    }
    next_backoff(current)
}

fn backoff_was_shutdown(shutdown_rx: &mut oneshot::Receiver<()>) -> bool {
    shutdown_rx.try_recv().is_ok()
}

#[derive(Deserialize)]
struct SnapshotWrapper {
    snapshot: Snapshot,
}

enum Decoded {
    /// The snapshot, and whether the core said it understands control.
    Snapshot(Snapshot, bool),
    Event(StoreEvent),
    Request(AgentRequest),
    Malformed,
}

/// Decode one NDJSON line from the viewer side-channel: `{"snapshot": …}`,
/// `{"event": …, …}` (the `StoreEvent` wire tag) or `{"request": …}`. Each kind
/// is told apart by its top-level key, checked in this order. A line that
/// parses as none — including one with an unknown `event` value, which fails
/// `StoreEvent`'s internally-tagged deserialization — is [`Decoded::Malformed`].
fn decode_line(line: &str) -> Decoded {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Decoded::Malformed;
    }
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Decoded::Malformed,
    };
    if value.get("snapshot").is_some() {
        let control = value.get("control").and_then(Value::as_u64).unwrap_or(0) >= 1;
        match serde_json::from_value::<SnapshotWrapper>(value) {
            Ok(w) => Decoded::Snapshot(w.snapshot, control),
            Err(_) => Decoded::Malformed,
        }
    } else if value.get("event").is_some() {
        match serde_json::from_value::<StoreEvent>(value) {
            Ok(event) => Decoded::Event(event),
            Err(_) => Decoded::Malformed,
        }
    } else if let Some(r) = value.get("request") {
        match (
            r.get("id").and_then(Value::as_u64),
            r.get("op").and_then(Value::as_str),
        ) {
            (Some(id), Some(op)) => Decoded::Request(AgentRequest {
                id,
                op: op.to_string(),
                args: r.get("args").cloned().unwrap_or(Value::Null),
            }),
            _ => Decoded::Malformed,
        }
    } else {
        Decoded::Malformed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener;

    #[test]
    fn decodes_a_snapshot_line() {
        let line = r#"{"snapshot":{"graph":"urn:project:default","triples":[],"kinds":{}}}"#;
        match decode_line(line) {
            Decoded::Snapshot(s, control) => {
                assert_eq!(s.graph, "urn:project:default");
                assert!(!control, "no control echo means an older core");
            }
            _ => panic!("expected Decoded::Snapshot"),
        }
        let echoed = r#"{"snapshot":{"graph":"g","triples":[],"kinds":{}},"control":1}"#;
        assert!(matches!(decode_line(echoed), Decoded::Snapshot(_, true)));
    }

    #[test]
    fn decodes_each_event_kind() {
        let insert = r#"{"event":"insert","graph":"g","triples":[],"kinds":{}}"#;
        assert!(matches!(
            decode_line(insert),
            Decoded::Event(StoreEvent::Insert { .. })
        ));

        let delete = r#"{"event":"delete","graph":"g","triples":[],"kinds":{}}"#;
        assert!(matches!(
            decode_line(delete),
            Decoded::Event(StoreEvent::Delete { .. })
        ));

        let changed = r#"{"event":"changed","graph":"g","triples_before":1,"triples_after":2}"#;
        assert!(matches!(
            decode_line(changed),
            Decoded::Event(StoreEvent::Changed { .. })
        ));
    }

    #[test]
    fn decodes_a_request_line() {
        let line = r#"{"request":{"id":42,"op":"show","args":{"iris":["a"],"focus":true}}}"#;
        match decode_line(line) {
            Decoded::Request(r) => {
                assert_eq!((r.id, r.op.as_str()), (42, "show"));
                assert_eq!(r.args["focus"], true);
            }
            _ => panic!("expected Decoded::Request"),
        }
        // No args is allowed; no id or no op is not.
        assert!(matches!(
            decode_line(r#"{"request":{"id":1,"op":"status"}}"#),
            Decoded::Request(AgentRequest {
                args: Value::Null,
                ..
            })
        ));
        assert!(matches!(
            decode_line(r#"{"request":{"op":"status"}}"#),
            Decoded::Malformed
        ));
        assert!(matches!(
            decode_line(r#"{"request":{"id":1}}"#),
            Decoded::Malformed
        ));
    }

    #[test]
    fn malformed_line_is_skipped() {
        assert!(matches!(decode_line("not json at all"), Decoded::Malformed));
        assert!(matches!(decode_line(""), Decoded::Malformed));
        assert!(matches!(
            decode_line(r#"{"event":"not-a-real-kind","graph":"g"}"#),
            Decoded::Malformed
        ));
        assert!(matches!(
            decode_line(r#"{"nothing":"useful"}"#),
            Decoded::Malformed
        ));
    }

    #[test]
    fn a_response_is_one_line_under_the_response_key() {
        let r = AgentResponse::ok(7, json!({ "n": 1 }), json!({ "epoch": 2 }));
        let v: Value = serde_json::from_str(r.line().trim()).unwrap();
        assert_eq!(v["response"]["id"], 7);
        assert_eq!(v["response"]["ok"], true);
        assert_eq!(v["response"]["result"]["n"], 1);
        assert!(r.line().ends_with('\n') && r.line().matches('\n').count() == 1);
        let e: Value = serde_json::from_str(
            AgentResponse::err(8, "unknown_op", "no such op")
                .line()
                .trim(),
        )
        .unwrap();
        assert_eq!(
            (
                e["response"]["ok"].clone(),
                e["response"]["error"]["code"].clone()
            ),
            (json!(false), json!("unknown_op"))
        );
    }

    #[test]
    fn backoff_sequence_is_1_2_4_8_16_30_30() {
        let mut d = INITIAL_BACKOFF;
        for expected in [1u64, 2, 4, 8, 16, 30, 30] {
            assert_eq!(d.as_secs(), expected);
            d = next_backoff(d);
        }
    }

    #[test]
    fn an_instance_id_is_32_hex_digits_and_differs_each_time() {
        let a = new_instance_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, new_instance_id());
    }

    /// Minimal fake server: accepts one connection, expects the
    /// `{"cmd":"snapshot"}` request, replies with an empty snapshot, then
    /// writes `extra_lines` and closes.
    async fn fake_server(extra_lines: Vec<String>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = socket.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let snapshot = serde_json::json!({
                "snapshot": {"graph": "urn:project:default", "triples": [], "kinds": {}}
            });
            write_half
                .write_all(format!("{snapshot}\n").as_bytes())
                .await
                .unwrap();
            for extra in extra_lines {
                write_half.write_all(extra.as_bytes()).await.unwrap();
                write_half.write_all(b"\n").await.unwrap();
            }
        });
        addr
    }

    #[tokio::test]
    async fn changed_event_reconnects_without_backoff() {
        let changed = serde_json::json!({"event": "changed", "graph": "g", "triples_before": 1, "triples_after": 0}).to_string();
        let addr = fake_server(vec![changed]).await;

        let (rx, _handle) = spawn(addr, Duration::from_secs(2));

        let mut states = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_second_connecting = false;
        // `try_recv` + an async sleep (not a blocking `recv_timeout`) so this
        // single-threaded test runtime still gets to poll `fake_server`'s
        // spawned task between polls — a blocking call here would starve it.
        while Instant::now() < deadline && !saw_second_connecting {
            match rx.try_recv() {
                Ok(ViewerMsg::State(s)) => {
                    if states.contains(&ConnState::Streaming) && s == ConnState::Connecting {
                        saw_second_connecting = true;
                    }
                    states.push(s);
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }

        assert!(
            saw_second_connecting,
            "expected Connecting again after Streaming, without going through Backoff; got {states:?}"
        );
        assert!(
            !states
                .iter()
                .any(|s| matches!(s, ConnState::Backoff { .. })),
            "a changed event must reconnect without backoff; got {states:?}"
        );
    }

    /// A fake core that understands control (`echo`) or ignores it like an
    /// older core. It records every line the viewer writes, sends `script`
    /// after the snapshot, and keeps the connection open.
    async fn fake_core(
        echo: bool,
        script: Vec<String>,
    ) -> (SocketAddr, mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen_tx, seen_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            // Every connection is served the same way, so a reconnect is visible.
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let (read_half, mut write_half) = socket.into_split();
                let seen_tx = seen_tx.clone();
                let script = script.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(read_half).lines();
                    let hello = lines.next_line().await.unwrap().unwrap();
                    let _ = seen_tx.send(hello);
                    let mut snapshot = serde_json::json!({
                        "snapshot": {"graph": "urn:project:default", "triples": [], "kinds": {}}
                    });
                    if echo {
                        snapshot["control"] = json!(1);
                    }
                    write_half
                        .write_all(format!("{snapshot}\n").as_bytes())
                        .await
                        .unwrap();
                    for line in script {
                        write_half
                            .write_all(format!("{line}\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    while let Ok(Some(line)) = lines.next_line().await {
                        let _ = seen_tx.send(line);
                    }
                });
            }
        });
        (addr, seen_rx)
    }

    async fn wait_for<T>(mut poll: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = poll() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_controllable_viewer_says_so_in_its_first_line() {
        let (addr, mut seen) = fake_core(true, vec![]).await;
        let conn = spawn_controllable(addr, Duration::from_secs(2));
        let hello = wait_for(|| seen.try_recv().ok()).await;
        let v: Value = serde_json::from_str(&hello).unwrap();
        assert_eq!(
            (v["cmd"].as_str(), v["control"].as_u64()),
            (Some("snapshot"), Some(1))
        );
        assert_eq!(v["instance"], conn.instance.as_str());

        // A viewer that is not controllable sends the plain line, as before.
        let (addr, mut seen) = fake_core(true, vec![]).await;
        let (_rx, _shutdown) = spawn(addr, Duration::from_secs(2));
        let hello = wait_for(|| seen.try_recv().ok()).await;
        assert_eq!(hello, r#"{"cmd":"snapshot"}"#);
    }

    #[tokio::test]
    async fn a_request_is_delivered_and_its_response_written_back() {
        let request = r#"{"request":{"id":5,"op":"status","args":{}}}"#.to_string();
        let (addr, mut seen) = fake_core(true, vec![request]).await;
        let conn = spawn_controllable(addr, Duration::from_secs(2));
        let _hello = wait_for(|| seen.try_recv().ok()).await;

        let req = wait_for(|| match conn.rx.try_recv() {
            Ok(ViewerMsg::Request(r)) => Some(r),
            _ => None,
        })
        .await;
        assert_eq!((req.id, req.op.as_str()), (5, "status"));

        conn.responses
            .send(AgentResponse::ok(
                5,
                json!({ "nodes": 3 }),
                json!({ "epoch": 0 }),
            ))
            .unwrap();
        let line = wait_for(|| seen.try_recv().ok()).await;
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["response"]["id"], 5);
        assert_eq!(v["response"]["result"]["nodes"], 3);
    }

    #[tokio::test]
    async fn against_a_core_that_understands_control_changed_is_answered_on_the_same_connection() {
        let changed =
            r#"{"event":"changed","graph":"g","triples_before":1,"triples_after":2}"#.to_string();
        let (addr, mut seen) = fake_core(true, vec![changed]).await;
        let conn = spawn_controllable(addr, Duration::from_secs(2));
        let _hello = wait_for(|| seen.try_recv().ok()).await;
        // The viewer asks for a new snapshot on the open connection.
        let asked = wait_for(|| seen.try_recv().ok()).await;
        assert_eq!(asked, r#"{"cmd":"snapshot"}"#);
        // And it did not reconnect: no second hello, no Connecting after Streaming.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut states = Vec::new();
        while let Ok(m) = conn.rx.try_recv() {
            if let ViewerMsg::State(s) = m {
                states.push(s);
            }
        }
        let first_streaming = states
            .iter()
            .position(|s| *s == ConnState::Streaming)
            .unwrap();
        assert!(
            !states[first_streaming..].contains(&ConnState::Connecting),
            "reconnected: {states:?}"
        );
        assert!(
            seen.try_recv().is_err(),
            "a second line, so a second connection?"
        );
    }

    #[tokio::test]
    async fn against_an_older_core_a_controllable_viewer_still_reconnects_on_changed() {
        // The older core ignores `control`: no echo in its snapshot.
        let changed =
            r#"{"event":"changed","graph":"g","triples_before":1,"triples_after":2}"#.to_string();
        let (addr, mut seen) = fake_core(false, vec![changed]).await;
        let _conn = spawn_controllable(addr, Duration::from_secs(2));
        let first = wait_for(|| seen.try_recv().ok()).await;
        assert!(first.contains("\"cmd\":\"snapshot\""));
        // A second connection (a second hello), not an in-band request.
        let second = wait_for(|| seen.try_recv().ok()).await;
        assert!(
            second.contains("\"control\":1"),
            "expected a reconnect, got {second}"
        );
    }
}
