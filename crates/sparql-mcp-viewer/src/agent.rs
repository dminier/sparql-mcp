//! Answering the core's commands (`docs/designs/sparql-mcp-viewer-agent-control.md`).
//!
//! The connection thread hands each `{"request": …}` line to Bevy as
//! [`ViewerMsg::Request`]; [`answer_core_requests`] answers it, once per frame,
//! through [`AgentOutbox`]. Phase 1 answers `status`, which works in the headless
//! path too; every other op is refused with `unknown_op` until its phase lands.

use bevy::prelude::*;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::connection::{AgentRequest, AgentResponse, ConnState, ViewerMsg};
use crate::graph::ViewerGraph;
use crate::plugin::{ConnStatus, CoreSet, FrameMessages};

/// Where answers go, and this process's instance id.
#[derive(Resource)]
pub struct AgentOutbox {
    pub responses: UnboundedSender<AgentResponse>,
    pub instance: String,
}

/// The `user` block of every successful response. Phase 1 does not watch the
/// user yet, so it always reports no activity.
pub fn user_block() -> Value {
    json!({ "active": false, "epoch": 0, "overrode_last_agent_command": false })
}

/// What `status` reports without a window: who this viewer is, which graph it
/// shows and how big it is. (Camera, selection and filter join it with the
/// windowed phases.)
pub fn status_snapshot(instance: &str, graph: &ViewerGraph, state: &ConnState) -> Value {
    let nodes = graph.nodes.iter().filter(|n| n.is_some()).count();
    json!({
        "instance": instance,
        "graph": graph.graph,
        "nodes": nodes,
        "edges": graph.edges.len(),
        "connection": match state {
            ConnState::Streaming => "streaming",
            ConnState::Connecting => "connecting",
            ConnState::AwaitSnapshot => "await_snapshot",
            ConnState::Backoff { .. } => "backoff",
        },
    })
}

/// Answer one request, or say why not.
pub fn answer(
    request: &AgentRequest,
    instance: &str,
    graph: &ViewerGraph,
    state: &ConnState,
) -> AgentResponse {
    match request.op.as_str() {
        "status" => AgentResponse::ok(
            request.id,
            status_snapshot(instance, graph, state),
            user_block(),
        ),
        other => AgentResponse::err(
            request.id,
            "unknown_op",
            &format!("this viewer does not support the op {other:?}"),
        ),
    }
}

/// Answers every request that arrived this frame.
pub fn answer_core_requests(
    frame: Res<FrameMessages>,
    outbox: Res<AgentOutbox>,
    graph: Res<ViewerGraph>,
    status: Res<ConnStatus>,
) {
    for msg in &frame.0 {
        if let ViewerMsg::Request(request) = msg {
            let response = answer(request, &outbox.instance, &graph, &status.0);
            // A closed channel means the connection thread is gone: nothing to do.
            let _ = outbox.responses.send(response);
        }
    }
}

/// Adds the answering system. `CorePlugin` adds it, so the headless path
/// answers too.
pub struct AgentPlugin;

impl Plugin for AgentPlugin {
    fn build(&self, app: &mut App) {
        // After the graph has applied this frame's snapshots and events, so an
        // answer describes the graph the user sees now.
        app.add_systems(
            Update,
            answer_core_requests
                .in_set(CoreSet::Graph)
                .after(crate::plugin::apply_to_graph),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparql_mcp::application::event_sink::Snapshot;

    fn graph_with(triples: &[[&str; 3]]) -> ViewerGraph {
        let snapshot = Snapshot {
            graph: "urn:project:default".into(),
            triples: triples.iter().map(|t| t.map(String::from)).collect(),
            kinds: Default::default(),
        };
        ViewerGraph::from_snapshot(&snapshot, None).0
    }

    fn request(op: &str) -> AgentRequest {
        AgentRequest {
            id: 9,
            op: op.into(),
            args: Value::Null,
        }
    }

    #[test]
    fn status_reports_instance_graph_counts_and_connection() {
        let g = graph_with(&[["<urn:a>", "<urn:p>", "<urn:b>"]]);
        let r = answer(&request("status"), "abc", &g, &ConnState::Streaming);
        let body = &r.0;
        assert_eq!(body["id"], 9);
        assert_eq!(body["ok"], true);
        assert_eq!(body["result"]["instance"], "abc");
        assert_eq!(body["result"]["graph"], "urn:project:default");
        assert_eq!(body["result"]["nodes"], 2);
        assert_eq!(body["result"]["edges"], 1);
        assert_eq!(body["result"]["connection"], "streaming");
        assert_eq!(body["user"]["epoch"], 0);
    }

    #[test]
    fn an_empty_graph_has_zero_nodes() {
        let r = answer(
            &request("status"),
            "abc",
            &ViewerGraph::empty(),
            &ConnState::Connecting,
        );
        assert_eq!(r.0["result"]["nodes"], 0);
        assert_eq!(r.0["result"]["connection"], "connecting");
    }

    #[test]
    fn an_op_this_phase_does_not_know_is_refused_with_its_name() {
        let r = answer(
            &request("show"),
            "abc",
            &ViewerGraph::empty(),
            &ConnState::Streaming,
        );
        assert_eq!(r.0["ok"], false);
        assert_eq!(r.0["id"], 9);
        assert_eq!(r.0["error"]["code"], "unknown_op");
        assert!(r.0["error"]["message"].as_str().unwrap().contains("show"));
    }
}
