//! Network client — no Bevy dependency. Runs the connection state machine
//! on its own thread with a `tokio` current-thread runtime, and pushes
//! decoded messages to the Bevy side over an `std`-friendly channel.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sparql_mcp::application::event_sink::{Snapshot, StoreEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

/// A decoded line from the viewer side-channel, or a connection-state
/// transition — `plugin::drain_inbox` moves these into `FrameMessages`
/// each frame.
#[derive(Debug, Clone)]
pub enum ViewerMsg {
    Snapshot(Snapshot),
    Event(StoreEvent),
    State(ConnState),
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

const AWAIT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Starts the connection on its own thread and returns a channel of
/// [`ViewerMsg`] plus a handle to stop it. `connect_timeout` bounds the
/// initial TCP `connect()`; the snapshot wait once connected has its own
/// fixed 30 s timeout (see the Technical Plan).
pub fn spawn(
    addr: SocketAddr,
    connect_timeout: Duration,
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
            rt.block_on(run(addr, connect_timeout, tx, shutdown_rx));
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

async fn run(
    addr: SocketAddr,
    connect_timeout: Duration,
    tx: crossbeam_channel::Sender<ViewerMsg>,
    mut shutdown_rx: oneshot::Receiver<()>,
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
        if write_half
            .write_all(b"{\"cmd\":\"snapshot\"}\n")
            .await
            .is_err()
        {
            backoff = enter_backoff(&tx, backoff, &mut shutdown_rx).await;
            continue;
        }

        set_state(&tx, ConnState::AwaitSnapshot);
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();

        let first_read =
            tokio::time::timeout(AWAIT_SNAPSHOT_TIMEOUT, reader.read_line(&mut line)).await;
        let got_snapshot = match first_read {
            Ok(Ok(n)) if n > 0 => match decode_line(&line) {
                Decoded::Snapshot(snap) => {
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

        let mut reconnect_without_backoff = false;
        loop {
            line.clear();
            tokio::select! {
                _ = &mut shutdown_rx => return,
                res = reader.read_line(&mut line) => {
                    match res {
                        Ok(0) => break, // EOF — reconnect with backoff
                        Ok(_) => match decode_line(&line) {
                            Decoded::Snapshot(snap) => {
                                let _ = tx.send(ViewerMsg::Snapshot(snap));
                            }
                            Decoded::Event(event) => {
                                let is_changed = matches!(event, StoreEvent::Changed { .. });
                                let _ = tx.send(ViewerMsg::Event(event));
                                if is_changed {
                                    // Forwarded, then the connection is closed and
                                    // reopened at once, without backoff.
                                    reconnect_without_backoff = true;
                                    break;
                                }
                            }
                            Decoded::Malformed => {
                                tracing::warn!(line = %line.trim(), "malformed viewer channel line; skipped");
                            }
                        },
                        Err(e) => {
                            tracing::warn!(error = %e, "viewer channel read error");
                            break;
                        }
                    }
                }
            }
        }

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
    Snapshot(Snapshot),
    Event(StoreEvent),
    Malformed,
}

/// Decode one NDJSON line from the viewer side-channel: `{"snapshot": …}`
/// or `{"event": …, …}` (the `StoreEvent` wire tag). A line that parses as
/// neither — including one with an unknown `event` value, which fails
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
        match serde_json::from_value::<SnapshotWrapper>(value) {
            Ok(w) => Decoded::Snapshot(w.snapshot),
            Err(_) => Decoded::Malformed,
        }
    } else if value.get("event").is_some() {
        match serde_json::from_value::<StoreEvent>(value) {
            Ok(event) => Decoded::Event(event),
            Err(_) => Decoded::Malformed,
        }
    } else {
        Decoded::Malformed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn decodes_a_snapshot_line() {
        let line = r#"{"snapshot":{"graph":"urn:project:default","triples":[],"kinds":{}}}"#;
        match decode_line(line) {
            Decoded::Snapshot(s) => assert_eq!(s.graph, "urn:project:default"),
            _ => panic!("expected Decoded::Snapshot"),
        }
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
    fn backoff_sequence_is_1_2_4_8_16_30_30() {
        let mut d = INITIAL_BACKOFF;
        for expected in [1u64, 2, 4, 8, 16, 30, 30] {
            assert_eq!(d.as_secs(), expected);
            d = next_backoff(d);
        }
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
}
