//! Viewer side-channel — loopback TCP listener for `crates/sparql-mcp-viewer`.
//!
//! Wire protocol: newline-delimited JSON. On connect the client sends
//! `{"cmd":"snapshot"}`; the server replies with one `{"snapshot": …}`
//! line, then forwards every subsequent [`StoreEvent`] as one
//! `{"event": …}` line until the client disconnects.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::application::event_sink::{classify_subjects, Snapshot, StoreEvent};
use crate::domain::{QueryResult, RdfTerm, SparqlStore};

/// How often an open connection re-checks the active graph for a change
/// (e.g. from `project_switch`) and, if it changed, pushes a fresh
/// snapshot. There is no change-notification on `Arc<RwLock<String>>`, so
/// this bounds the latency instead of requiring one.
const RESYNC_POLL: Duration = Duration::from_millis(200);

/// Accept connections on `bind` forever, serving each on its own task.
/// The viewer side-channel is additive: callers should log and continue
/// without the channel on error rather than treat a bind failure as fatal
/// to the whole server (see `main.rs`).
pub async fn spawn_viewer_channel(
    bind: SocketAddr,
    store: Arc<dyn SparqlStore>,
    active_graph: Arc<RwLock<String>>,
    events: broadcast::Receiver<StoreEvent>,
) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding viewer side-channel on {bind}"))?;
    info!(%bind, "viewer side-channel listening");
    serve_viewer_channel(listener, store, active_graph, events).await
}

/// Accept loop split out from [`spawn_viewer_channel`] so a test can bind
/// an ephemeral `127.0.0.1:0` port itself, read back the assigned port via
/// `TcpListener::local_addr`, and only then start serving.
pub async fn serve_viewer_channel(
    listener: TcpListener,
    store: Arc<dyn SparqlStore>,
    active_graph: Arc<RwLock<String>>,
    events: broadcast::Receiver<StoreEvent>,
) -> Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        debug!(%peer, "viewer connected");
        let store = store.clone();
        let active_graph = active_graph.clone();
        let events = events.resubscribe();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, store, active_graph, events).await {
                warn!(%peer, error = %e, "viewer connection ended");
            }
        });
    }
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    store: Arc<dyn SparqlStore>,
    active_graph: Arc<RwLock<String>>,
    events_template: broadcast::Receiver<StoreEvent>,
) -> Result<()> {
    let (read_half, mut writer) = socket.into_split();
    let mut reader = BufReader::new(read_half);

    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(()); // disconnected before sending anything
    }
    let cmd: serde_json::Value = serde_json::from_str(line.trim())
        .with_context(|| format!("invalid first line from viewer client: {line:?}"))?;
    if cmd.get("cmd").and_then(|v| v.as_str()) != Some("snapshot") {
        anyhow::bail!("expected {{\"cmd\":\"snapshot\"}}, got {line:?}");
    }

    // Subscribed by the caller (and resubscribed below) before each
    // snapshot query runs, so tokio's broadcast buffering — not a
    // hand-rolled buffer-and-replay — covers any write that commits while
    // the query executes.
    let mut events = events_template.resubscribe();
    let mut current_graph = read_active_graph(&active_graph);
    send_snapshot(&mut writer, &store, &current_graph).await?;

    let mut resync = tokio::time::interval(RESYNC_POLL);
    resync.tick().await; // first tick fires immediately; consume it

    loop {
        tokio::select! {
            _ = resync.tick() => {
                let graph_now = read_active_graph(&active_graph);
                if graph_now != current_graph {
                    current_graph = graph_now;
                    // Drop anything buffered for the old graph and start
                    // clean from the new snapshot.
                    events = events_template.resubscribe();
                    send_snapshot(&mut writer, &store, &current_graph).await?;
                }
            }
            received = events.recv() => {
                match received {
                    Ok(event) => {
                        write_line(&mut writer, &event).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "viewer connection fell behind; closing");
                        return Ok(());
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
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
                rdf_term_to_nt(row.get("s").expect("?s is always bound")),
                rdf_term_to_nt(row.get("p").expect("?p is always bound")),
                rdf_term_to_nt(row.get("o").expect("?o is always bound")),
            ]
        })
        .collect();
    let kinds = classify_subjects(&triples);
    let snapshot = Snapshot {
        graph: graph.to_string(),
        triples,
        kinds,
    };

    write_line(writer, &serde_json::json!({ "snapshot": snapshot })).await
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

/// N-Triples encoding of a domain [`RdfTerm`] (see the Required Changes
/// note on term encoding: `<iri>`, `_:id`, or `"lex"["@lang" | "^^<dt>"]`).
fn rdf_term_to_nt(term: &RdfTerm) -> String {
    match term {
        RdfTerm::Iri(iri) => format!("<{iri}>"),
        RdfTerm::BlankNode(id) => format!("_:{id}"),
        RdfTerm::Literal {
            value,
            datatype,
            lang,
        } => {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            match lang {
                Some(l) => format!("\"{escaped}\"@{l}"),
                None => match datatype {
                    Some(dt) => format!("\"{escaped}\"^^<{dt}>"),
                    None => format!("\"{escaped}\""),
                },
            }
        }
    }
}
