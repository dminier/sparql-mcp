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
    /// Move the camera: frame everything, frame some nodes, or zoom.
    Camera {
        action: CameraAction,
        iris: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        factor: Option<f64>,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        if_user_epoch: Option<u64>,
    },
    /// Highlight nodes (or isolate them), and select one.
    Show {
        iris: Vec<String>,
        mode: ShowMode,
        focus: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        select: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        if_user_epoch: Option<u64>,
    },
    /// Drop the agent's highlight; with `restore`, give the user's view back.
    Clear { restore: bool },
    /// Edit the class filter the user sees in its panel.
    Filter {
        hide_classes: Vec<String>,
        show_classes: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        show_untyped: Option<bool>,
        reset: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        if_user_epoch: Option<u64>,
    },
}

/// What `camera` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraAction {
    /// Frame every node.
    FrameAll,
    /// Frame the nodes named in `iris`.
    Frame,
    /// Move closer or farther along the line of sight, by `factor`.
    Zoom,
}

/// How `show` marks its nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowMode {
    /// The nodes glow; everything else is left as it was.
    Highlight,
    /// The nodes glow and everything else fades.
    Isolate,
}

impl ViewerOp {
    /// Whether sending the same command twice leaves the viewer as sending it
    /// once, so that the core may repeat it after a lost connection.
    pub fn is_idempotent(&self) -> bool {
        // Each of these sets a state rather than changing it by an amount.
        match self {
            // A zoom is relative to where the camera is now: repeating it zooms again.
            ViewerOp::Camera {
                action: CameraAction::Zoom,
                ..
            } => false,
            ViewerOp::Status {}
            | ViewerOp::Camera { .. }
            | ViewerOp::Show { .. }
            | ViewerOp::Clear { .. }
            | ViewerOp::Filter { .. } => true,
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
    fn the_mutating_ops_serialize_with_their_arguments_and_skip_the_absent_ones() {
        let show = serde_json::to_value(ViewerOp::Show {
            iris: vec!["http://e/a".into()],
            mode: ShowMode::Isolate,
            focus: true,
            select: None,
            if_user_epoch: Some(4),
        })
        .unwrap();
        assert_eq!(
            show,
            serde_json::json!({ "op": "show", "args": {
                "iris": ["http://e/a"], "mode": "isolate", "focus": true, "if_user_epoch": 4 } })
        );
        let clear = serde_json::to_value(ViewerOp::Clear { restore: true }).unwrap();
        assert_eq!(
            clear,
            serde_json::json!({ "op": "clear", "args": { "restore": true } })
        );
        let filter = serde_json::to_value(ViewerOp::Filter {
            hide_classes: vec![],
            show_classes: vec!["http://e/C".into()],
            show_untyped: Some(false),
            reset: false,
            if_user_epoch: None,
        })
        .unwrap();
        assert_eq!(filter["args"]["show_untyped"], false);
        assert!(filter["args"].get("if_user_epoch").is_none());
    }

    #[test]
    fn a_camera_op_serializes_and_only_a_zoom_is_not_repeatable() {
        let frame = ViewerOp::Camera {
            action: CameraAction::Frame,
            iris: vec!["http://e/a".into()],
            factor: None,
            duration_ms: 600,
            if_user_epoch: None,
        };
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            serde_json::json!({ "op": "camera", "args": {
                "action": "frame", "iris": ["http://e/a"], "duration_ms": 600 } })
        );
        assert!(frame.is_idempotent());
        let all = ViewerOp::Camera {
            action: CameraAction::FrameAll,
            iris: vec![],
            factor: None,
            duration_ms: 0,
            if_user_epoch: None,
        };
        assert_eq!(
            serde_json::to_value(&all).unwrap()["args"]["action"],
            "frame_all"
        );
        assert!(all.is_idempotent());
        let zoom = ViewerOp::Camera {
            action: CameraAction::Zoom,
            iris: vec![],
            factor: Some(2.0),
            duration_ms: 600,
            if_user_epoch: Some(3),
        };
        assert!(!zoom.is_idempotent(), "a retried zoom would zoom twice");
        assert_eq!(serde_json::to_value(&zoom).unwrap()["args"]["factor"], 2.0);
    }

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
