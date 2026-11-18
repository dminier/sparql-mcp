//! What the core can ask of a connected viewer, and what comes back
//! (`docs/designs/sparql-mcp-viewer-agent-control.md`, *Viewer registry*).
//!
//! This module does no I/O. The side-channel (`infrastructure::viewer_channel`)
//! implements [`ViewerControl`]; the `viewer_*` tools depend only on the trait,
//! and a server built without a viewer gets [`NullViewerControl`].

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One command for the viewer. On the wire it is the `op` and `args` of a
/// `{"request": …}` line.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum ViewerOp {
    /// What the viewer shows now: graph, counts, camera, selection, filter.
    Status {},
}

impl ViewerOp {
    /// Whether sending the same command twice leaves the viewer as sending it
    /// once, so that the core may repeat it after a lost connection.
    pub fn is_idempotent(&self) -> bool {
        match self {
            ViewerOp::Status {} => true,
        }
    }
}

/// What the user is doing in the viewer, attached to every successful
/// response so the agent can tell when to step back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserActivity {
    /// The user changed the view very recently.
    #[serde(default)]
    pub active: bool,
    /// Counts the user's changes of camera, filter and selection.
    #[serde(default)]
    pub epoch: u64,
    /// The user changed the view after the agent's last command.
    #[serde(default)]
    pub overrode_last_agent_command: bool,
}

/// How a request ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerOutcome {
    /// The viewer answered `ok`.
    Applied {
        result: Value,
        user: UserActivity,
        reason: Option<String>,
    },
    /// No viewer that can be controlled is connected.
    NotConnected,
    /// The deadline passed. The viewer may still apply the command.
    Timeout,
    /// The viewer answered with an error.
    Rejected { code: String, message: String },
}

/// Whether the side-channel listener is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerState {
    /// No `[viewer]` side-channel in this process.
    Absent,
    Listening,
    BindFailed(String),
}

/// What the hub knows about itself, for `viewer_status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubInfo {
    pub listener: ListenerState,
    /// The instance id of the controlled viewer, if one is connected.
    pub instance: Option<String>,
    /// Viewers connected now that can be controlled.
    pub controllable_connections: usize,
}

pub type OutcomeFuture<'a> = Pin<Box<dyn Future<Output = ViewerOutcome> + Send + 'a>>;

pub trait ViewerControl: Send + Sync {
    /// Send `op` to the controlled viewer and wait for its answer until `deadline`.
    fn request(&self, op: ViewerOp, deadline: Instant) -> OutcomeFuture<'_>;

    /// The hub's own state, which needs no viewer to answer.
    fn info(&self) -> HubInfo;
}

/// The control used when there is no viewer side-channel.
pub struct NullViewerControl;

impl ViewerControl for NullViewerControl {
    fn request(&self, _op: ViewerOp, _deadline: Instant) -> OutcomeFuture<'_> {
        Box::pin(async { ViewerOutcome::NotConnected })
    }

    fn info(&self) -> HubInfo {
        HubInfo {
            listener: ListenerState::Absent,
            instance: None,
            controllable_connections: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_op_serializes_as_op_and_args() {
        let line = serde_json::to_value(ViewerOp::Status {}).unwrap();
        assert_eq!(line, serde_json::json!({ "op": "status", "args": {} }));
    }

    #[tokio::test]
    async fn the_null_control_is_never_connected() {
        let ctl = NullViewerControl;
        let out = ctl
            .request(
                ViewerOp::Status {},
                Instant::now() + std::time::Duration::from_secs(1),
            )
            .await;
        assert_eq!(out, ViewerOutcome::NotConnected);
        assert_eq!(ctl.info().listener, ListenerState::Absent);
    }
}
