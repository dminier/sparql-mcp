//! Viewer side-channel — loopback TCP listener for `crates/sparql-mcp-viewer`.
//!
//! Wire protocol: newline-delimited JSON. On connect the client sends
//! `{"cmd":"snapshot"}` (a viewer that can be controlled also sends
//! `"control":1` and an `"instance"` id); the server replies with one
//! `{"snapshot": …}` line, then forwards every subsequent [`StoreEvent`] as one
//! `{"event": …}` line until the client disconnects.
//!
//! A controllable viewer also receives `{"request": …}` lines from the
//! [`ViewerHub`] and answers each with a `{"response": …}` line
//! (`docs/designs/sparql-mcp-viewer-agent-control.md`). Its connection is
//! split into a reader task and a writer task, so a large line in one
//! direction never blocks the other, and it **resynchronises on the same
//! connection** (a mid-stream `{"cmd":"snapshot"}`, a re-snapshot after a lag)
//! instead of being closed, so a request in flight survives.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot, Notify};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};
use tracing::{debug, info, warn};

use crate::application::event_sink::{classify_subjects, Snapshot, StoreEvent};
use crate::application::viewer_control::{
    HubInfo, ListenerState, OutcomeFuture, UserActivity, ViewerControl, ViewerOp, ViewerOutcome,
};
use crate::domain::{QueryResult, SparqlStore};

/// How often an open connection re-checks the active graph for a change
/// (e.g. from `project_switch`) and, if it changed, pushes a fresh
/// snapshot. There is no change-notification on `Arc<RwLock<String>>`, so
/// this bounds the latency instead of requiring one.
const RESYNC_POLL: Duration = Duration::from_millis(200);

/// Longest line the core reads from a viewer (a screenshot answer is the
/// largest). A longer line is discarded and reading goes on.
const MAX_INBOUND_LINE: usize = 8 * 1024 * 1024;

/// Longest request line the core sends.
const MAX_REQUEST_LINE: usize = 1024 * 1024;

// ── The hub ───────────────────────────────────────────────────────────────────

/// One request waiting for its response.
struct Pending {
    conn: u64,
    reply: oneshot::Sender<Reply>,
}

/// What a waiting request is told.
enum Reply {
    Response(Response),
    /// The connection closed before the viewer answered.
    Dropped,
}

/// A decoded `{"response": …}` line.
#[derive(Debug)]
struct Response {
    ok: bool,
    result: Value,
    user: UserActivity,
    reason: Option<String>,
    error: Option<(String, String)>,
}

struct Outbound {
    line: String,
}

#[derive(Clone)]
struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<Outbound>,
}

struct InstanceSlot {
    instance: String,
    conn: Option<Conn>,
}

struct HubState {
    listener: ListenerState,
    /// Instances in the order they first appeared.
    instances: Vec<InstanceSlot>,
    /// The instance requests go to.
    controlled: Option<String>,
    pending: HashMap<u64, Pending>,
}

/// The registry of controllable viewers, shared by the side-channel (which
/// registers connections) and the `viewer_*` tools (which send requests).
///
/// *   The **controlled** instance is the one that appeared most recently. A
///     viewer that reconnects keeps its slot, so control never moves to another
///     window because of a reconnect.
/// *   If the controlled instance is not connected when a request arrives, the
///     most recently seen connected one takes over.
/// *   When a connection closes while a request is waiting, the request waits
///     (within its deadline) for the same instance to come back and, if the
///     command can be repeated safely, is sent once more.
pub struct ViewerHub {
    state: Mutex<HubState>,
    /// Woken when a connection registers.
    registered: Notify,
    next_request: AtomicU64,
    next_conn: AtomicU64,
}

impl Default for ViewerHub {
    fn default() -> Self {
        Self {
            state: Mutex::new(HubState {
                listener: ListenerState::Absent,
                instances: Vec::new(),
                controlled: None,
                pending: HashMap::new(),
            }),
            registered: Notify::new(),
            next_request: AtomicU64::new(1),
            next_conn: AtomicU64::new(1),
        }
    }
}

/// Removes a pending request if the future waiting for it is dropped.
struct PendingGuard<'a> {
    hub: &'a ViewerHub,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.hub.lock().pending.remove(&self.id);
    }
}

/// A connection's membership of the hub; leaving it on drop.
struct Registration {
    hub: Arc<ViewerHub>,
    instance: String,
    conn: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.hub.deregister(&self.instance, self.conn);
    }
}

impl ViewerHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record that the listener is up, or why it is not.
    pub fn set_listener(&self, state: ListenerState) {
        self.lock().listener = state;
    }

    fn register(
        self: &Arc<Self>,
        instance: &str,
    ) -> (Registration, mpsc::UnboundedReceiver<Outbound>) {
        let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut s = self.lock();
            let conn = Some(Conn { id, tx });
            match s.instances.iter_mut().find(|i| i.instance == instance) {
                // A reconnect keeps its place and does not move control.
                Some(slot) => slot.conn = conn,
                None => {
                    s.instances.push(InstanceSlot {
                        instance: instance.to_string(),
                        conn,
                    });
                    s.controlled = Some(instance.to_string());
                }
            }
        }
        self.registered.notify_waiters();
        (
            Registration {
                hub: self.clone(),
                instance: instance.to_string(),
                conn: id,
            },
            rx,
        )
    }

    fn deregister(&self, instance: &str, conn: u64) {
        let mut s = self.lock();
        if let Some(slot) = s.instances.iter_mut().find(|i| i.instance == instance) {
            // A newer connection of the same instance may already have replaced it.
            if slot.conn.as_ref().is_some_and(|c| c.id == conn) {
                slot.conn = None;
            }
        }
        let dropped: Vec<u64> = s
            .pending
            .iter()
            .filter(|(_, p)| p.conn == conn)
            .map(|(id, _)| *id)
            .collect();
        for id in dropped {
            if let Some(p) = s.pending.remove(&id) {
                let _ = p.reply.send(Reply::Dropped);
            }
        }
    }

    /// Hand a response to the request waiting for it. One whose request is
    /// gone (timed out, or cancelled) is dropped.
    fn resolve(&self, id: u64, response: Response) {
        let pending = self.lock().pending.remove(&id);
        match pending {
            Some(p) => {
                let _ = p.reply.send(Reply::Response(response));
            }
            None => debug!(id, "response for a request that is no longer waiting"),
        }
    }

    /// The connection a request should go to: the named instance, or the
    /// controlled one (or, failing that, the most recent connected one, which
    /// then becomes the controlled one).
    fn pick(&self, instance: Option<&str>) -> Option<(String, Conn)> {
        let mut s = self.lock();
        let connected = |slot: &InstanceSlot| slot.conn.clone().map(|c| (slot.instance.clone(), c));
        if let Some(name) = instance {
            return s
                .instances
                .iter()
                .find(|i| i.instance == name)
                .and_then(connected);
        }
        let current = s
            .controlled
            .as_deref()
            .and_then(|name| s.instances.iter().find(|i| i.instance == name))
            .and_then(connected);
        if current.is_some() {
            return current;
        }
        let next = s.instances.iter().rev().find_map(connected);
        if let Some((name, _)) = &next {
            s.controlled = Some(name.clone());
        }
        next
    }

    /// Wait until `instance` is connected, or the deadline.
    async fn wait_for_instance(&self, instance: &str, deadline: Instant) -> Option<Conn> {
        loop {
            // Create the wake-up before checking, so a registration between
            // the check and the wait is not missed.
            let notified = self.registered.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some((_, conn)) = self.pick(Some(instance)) {
                return Some(conn);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            if tokio::time::timeout(left, notified).await.is_err() {
                return self.pick(Some(instance)).map(|(_, c)| c);
            }
        }
    }

    /// Send one request line and wait for its answer.
    async fn send_once(
        &self,
        conn: &Conn,
        op: &ViewerOp,
        deadline: Instant,
    ) -> std::result::Result<Reply, ViewerOutcome> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        // `op` serializes as {"op": name, "args": {…}}; the line is
        // {"request": {"id": n, "op": name, "args": {…}}}.
        let mut body = match serde_json::to_value(op) {
            Ok(Value::Object(m)) => m,
            _ => {
                return Err(ViewerOutcome::Rejected {
                    code: "invalid_request".into(),
                    message: "the command could not be encoded".into(),
                })
            }
        };
        body.insert("id".into(), json!(id));
        let mut line = match serde_json::to_string(&json!({ "request": body })) {
            Ok(l) => l,
            Err(e) => {
                return Err(ViewerOutcome::Rejected {
                    code: "invalid_request".into(),
                    message: e.to_string(),
                })
            }
        };
        if line.len() > MAX_REQUEST_LINE {
            return Err(ViewerOutcome::Rejected {
                code: "request_too_large".into(),
                message: format!(
                    "the request is {} bytes; the limit is {MAX_REQUEST_LINE}",
                    line.len()
                ),
            });
        }
        line.push('\n');
        let (reply_tx, reply_rx) = oneshot::channel();
        self.lock().pending.insert(
            id,
            Pending {
                conn: conn.id,
                reply: reply_tx,
            },
        );
        let _guard = PendingGuard { hub: self, id };
        if conn.tx.send(Outbound { line }).is_err() {
            return Ok(Reply::Dropped);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, reply_rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Ok(Reply::Dropped),
            Err(_) => Err(ViewerOutcome::Timeout),
        }
    }

    async fn request_inner(&self, op: ViewerOp, deadline: Instant) -> ViewerOutcome {
        let Some((instance, mut conn)) = self.pick(None) else {
            return ViewerOutcome::NotConnected;
        };
        let mut retried = false;
        loop {
            match self.send_once(&conn, &op, deadline).await {
                Err(outcome) => return outcome,
                Ok(Reply::Response(r)) => return outcome_of(r),
                Ok(Reply::Dropped) => {
                    // The connection closed. Wait for the same viewer to come
                    // back and, only for a command that can be repeated, ask again.
                    if retried || !op.is_idempotent() {
                        return ViewerOutcome::NotConnected;
                    }
                    retried = true;
                    match self.wait_for_instance(&instance, deadline).await {
                        Some(c) => conn = c,
                        None => return ViewerOutcome::NotConnected,
                    }
                }
            }
        }
    }
}

fn outcome_of(r: Response) -> ViewerOutcome {
    if r.ok {
        ViewerOutcome::Applied {
            result: r.result,
            user: r.user,
            reason: r.reason,
        }
    } else {
        let (code, message) = r
            .error
            .unwrap_or_else(|| ("unknown".into(), "the viewer refused the command".into()));
        ViewerOutcome::Rejected { code, message }
    }
}

impl ViewerControl for ViewerHub {
    fn request(&self, op: ViewerOp, deadline: Instant) -> OutcomeFuture<'_> {
        Box::pin(self.request_inner(op, deadline))
    }

    fn info(&self) -> HubInfo {
        let s = self.lock();
        let controlled = s
            .controlled
            .as_deref()
            .and_then(|name| s.instances.iter().find(|i| i.instance == name))
            .filter(|slot| slot.conn.is_some())
            .map(|slot| slot.instance.clone());
        HubInfo {
            listener: s.listener.clone(),
            instance: controlled,
            controllable_connections: s.instances.iter().filter(|i| i.conn.is_some()).count(),
        }
    }
}

// ── The listener ──────────────────────────────────────────────────────────────

/// Accept connections on `bind` forever, serving each on its own task.
/// The viewer side-channel is additive: callers should log and continue
/// without the channel on error rather than treat a bind failure as fatal
/// to the whole server (see `main.rs`). A bind failure is also recorded in
/// `hub`, so `viewer_status` can say why no viewer connects.
pub async fn spawn_viewer_channel(
    bind: SocketAddr,
    store: Arc<dyn SparqlStore>,
    active_graph: Arc<RwLock<String>>,
    events: broadcast::Receiver<StoreEvent>,
    hub: Arc<ViewerHub>,
) -> Result<()> {
    let listener = match TcpListener::bind(bind).await {
        Ok(l) => l,
        Err(e) => {
            hub.set_listener(ListenerState::BindFailed(e.to_string()));
            return Err(e).with_context(|| format!("binding viewer side-channel on {bind}"));
        }
    };
    info!(%bind, "viewer side-channel listening");
    serve_viewer_channel(listener, store, active_graph, events, hub).await
}

/// Accept loop split out from [`spawn_viewer_channel`] so a test can bind
/// an ephemeral `127.0.0.1:0` port itself, read back the assigned port via
/// `TcpListener::local_addr`, and only then start serving.
pub async fn serve_viewer_channel(
    listener: TcpListener,
    store: Arc<dyn SparqlStore>,
    active_graph: Arc<RwLock<String>>,
    events: broadcast::Receiver<StoreEvent>,
    hub: Arc<ViewerHub>,
) -> Result<()> {
    hub.set_listener(ListenerState::Listening);
    loop {
        let (socket, peer) = listener.accept().await?;
        debug!(%peer, "viewer connected");
        let store = store.clone();
        let active_graph = active_graph.clone();
        let events = events.resubscribe();
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, store, active_graph, events, hub).await {
                warn!(%peer, error = %e, "viewer connection ended");
            }
        });
    }
}

/// The first line of a connection.
struct Handshake {
    /// `Some(instance)` when the viewer can be controlled.
    controllable: Option<String>,
}

fn parse_handshake(line: &str) -> Result<Handshake> {
    let cmd: Value = serde_json::from_str(line.trim())
        .with_context(|| format!("invalid first line from viewer client: {line:?}"))?;
    if cmd.get("cmd").and_then(Value::as_str) != Some("snapshot") {
        anyhow::bail!("expected {{\"cmd\":\"snapshot\"}}, got {line:?}");
    }
    let control = cmd.get("control").and_then(Value::as_u64).unwrap_or(0);
    let instance = cmd
        .get("instance")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 128);
    Ok(Handshake {
        controllable: (control >= 1)
            .then_some(instance)
            .flatten()
            .map(str::to_string),
    })
}

/// Decode one line the viewer sent after the handshake.
enum Inbound {
    Response(u64, Response),
    SnapshotCommand,
    Other,
}

fn decode_inbound(line: &str) -> Inbound {
    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
        return Inbound::Other;
    };
    if let Some(r) = value.get("response") {
        let Some(id) = r.get("id").and_then(Value::as_u64) else {
            return Inbound::Other;
        };
        let error = r.get("error").map(|e| {
            (
                e.get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                e.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            )
        });
        return Inbound::Response(
            id,
            Response {
                ok: r.get("ok").and_then(Value::as_bool).unwrap_or(false),
                result: r.get("result").cloned().unwrap_or(Value::Null),
                user: r
                    .get("user")
                    .and_then(|u| serde_json::from_value(u.clone()).ok())
                    .unwrap_or_default(),
                reason: r.get("reason").and_then(Value::as_str).map(str::to_string),
                error,
            },
        );
    }
    if value.get("cmd").and_then(Value::as_str) == Some("snapshot") {
        return Inbound::SnapshotCommand;
    }
    Inbound::Other
}

/// Read everything the viewer sends after the handshake. Ends when the
/// connection does. A line over the cap is dropped and reading goes on.
async fn read_loop(
    mut frames: FramedRead<tokio::net::tcp::OwnedReadHalf, LinesCodec>,
    hub: Option<Arc<ViewerHub>>,
    snapshot_requests: mpsc::UnboundedSender<()>,
) {
    // After an error `FramedRead` yields one `None` and then reads on, so a
    // `None` straight after an error is not the end of the connection.
    let mut after_error = false;
    loop {
        let Some(next) = frames.next().await else {
            if after_error {
                after_error = false;
                continue;
            }
            return;
        };
        after_error = false;
        match next {
            Ok(line) => {
                // A viewer that cannot be controlled sends nothing we act on.
                let Some(hub) = &hub else { continue };
                match decode_inbound(&line) {
                    Inbound::Response(id, r) => hub.resolve(id, r),
                    Inbound::SnapshotCommand => {
                        let _ = snapshot_requests.send(());
                    }
                    Inbound::Other => debug!("ignored a line from the viewer"),
                }
            }
            Err(LinesCodecError::MaxLineLengthExceeded) => {
                after_error = true;
                warn!(
                    cap = MAX_INBOUND_LINE,
                    "a viewer line was over the cap and was dropped"
                );
            }
            Err(LinesCodecError::Io(e)) => {
                debug!(error = %e, "viewer read error");
                return;
            }
        }
    }
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    store: Arc<dyn SparqlStore>,
    active_graph: Arc<RwLock<String>>,
    events_template: broadcast::Receiver<StoreEvent>,
    hub: Arc<ViewerHub>,
) -> Result<()> {
    let (read_half, mut writer) = socket.into_split();
    let mut frames = FramedRead::new(read_half, LinesCodec::new_with_max_length(MAX_INBOUND_LINE));

    let first = match frames.next().await {
        None => return Ok(()), // disconnected before sending anything
        Some(Ok(line)) => line,
        Some(Err(e)) => anyhow::bail!("reading the first line from a viewer client: {e}"),
    };
    let handshake = parse_handshake(&first)?;

    // Register before the snapshot, so a request issued as soon as the viewer
    // is up already has somewhere to go.
    let (registration, mut requests) = match &handshake.controllable {
        Some(instance) => {
            let (reg, rx) = hub.register(instance);
            (Some(reg), Some(rx))
        }
        None => (None, None),
    };
    let controllable = registration.is_some();

    // Subscribed by the caller (and resubscribed below) before each
    // snapshot query runs, so tokio's broadcast buffering — not a
    // hand-rolled buffer-and-replay — covers any write that commits while
    // the query executes.
    let mut events = events_template.resubscribe();
    let mut current_graph = read_active_graph(&active_graph);
    send_snapshot(&mut writer, &store, &current_graph, controllable).await?;

    let (snapshot_tx, mut snapshot_rx) = mpsc::unbounded_channel::<()>();
    let mut reader = tokio::spawn(read_loop(
        frames,
        controllable.then(|| hub.clone()),
        snapshot_tx,
    ));

    let mut resync = tokio::time::interval(RESYNC_POLL);
    resync.tick().await; // first tick fires immediately; consume it

    let outcome: Result<()> = async {
        loop {
            tokio::select! {
                // Store events first: one emitted before a request is applied
                // by the viewer before that request.
                biased;
                received = events.recv() => {
                    match received {
                        Ok(event) => write_line(&mut writer, &event).await?,
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            if !controllable {
                                warn!(skipped, "viewer connection fell behind; closing");
                                return Ok(());
                            }
                            // A controllable viewer keeps its connection (a
                            // request may be in flight): start over from a snapshot.
                            warn!(skipped, "viewer connection fell behind; re-sending a snapshot");
                            events = events_template.resubscribe();
                            send_snapshot(&mut writer, &store, &current_graph, controllable).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    }
                }
                request = next_request(&mut requests) => {
                    let Some(request) = request else { return Ok(()) };
                    // Everything already queued goes out ahead of the request.
                    loop {
                        match events.try_recv() {
                            Ok(event) => write_line(&mut writer, &event).await?,
                            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                                events = events_template.resubscribe();
                                send_snapshot(&mut writer, &store, &current_graph, controllable).await?;
                            }
                            Err(_) => break,
                        }
                    }
                    writer.write_all(request.line.as_bytes()).await?;
                }
                Some(()) = snapshot_rx.recv() => {
                    events = events_template.resubscribe();
                    current_graph = read_active_graph(&active_graph);
                    send_snapshot(&mut writer, &store, &current_graph, controllable).await?;
                }
                _ = resync.tick() => {
                    let graph_now = read_active_graph(&active_graph);
                    if graph_now != current_graph {
                        current_graph = graph_now;
                        // Drop anything buffered for the old graph and start
                        // clean from the new snapshot.
                        events = events_template.resubscribe();
                        send_snapshot(&mut writer, &store, &current_graph, controllable).await?;
                    }
                }
                _ = &mut reader => return Ok(()),
            }
        }
    }
    .await;

    reader.abort();
    drop(registration);
    outcome
}

/// The next outbound request, or never for a connection that takes none.
async fn next_request(
    requests: &mut Option<mpsc::UnboundedReceiver<Outbound>>,
) -> Option<Outbound> {
    match requests {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

fn read_active_graph(active_graph: &Arc<RwLock<String>>) -> String {
    active_graph
        .read()
        .expect("active_graph lock poisoned")
        .clone()
}

async fn send_snapshot(
    writer: &mut (impl AsyncWrite + Unpin),
    store: &Arc<dyn SparqlStore>,
    graph: &str,
    control: bool,
) -> Result<()> {
    // `graph` is always a server-assigned `urn:project:<id>` IRI (`id` is
    // validated to alphanumerics/hyphens/underscores by
    // `project::validate_id`), so direct interpolation is safe — the same
    // pattern `project_list`'s query already uses.
    let sparql = format!("SELECT ?s ?p ?o WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}");
    let QueryResult::Solutions(sol) = store
        .query(&sparql)
        .with_context(|| format!("snapshot query for graph {graph}"))?
    else {
        anyhow::bail!("expected SELECT results for the snapshot query");
    };

    let triples: Vec<[String; 3]> = sol
        .rows
        .iter()
        .map(|row| {
            [
                row.get("s").expect("?s is always bound").to_ntriples(),
                row.get("p").expect("?p is always bound").to_ntriples(),
                row.get("o").expect("?o is always bound").to_ntriples(),
            ]
        })
        .collect();
    let kinds = classify_subjects(&triples);
    let snapshot = Snapshot {
        graph: graph.to_string(),
        triples,
        kinds,
    };

    // `control` tells the viewer that this core understands requests and
    // in-band resync, so that it does not have to reconnect on `changed`.
    let line = if control {
        serde_json::json!({ "snapshot": snapshot, "control": 1 })
    } else {
        serde_json::json!({ "snapshot": snapshot })
    };
    write_line(writer, &line).await
}

async fn write_line(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl serde::Serialize,
) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    Ok(())
}
