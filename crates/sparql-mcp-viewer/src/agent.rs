//! Answering the core's commands (`docs/designs/sparql-mcp-viewer-agent-control.md`).
//!
//! The connection thread hands each `{"request": …}` line to Bevy as
//! [`ViewerMsg::Request`], and the requests are answered once per frame through
//! [`AgentOutbox`]:
//!
//! *   [`answer_core_requests`] (every build, also headless) answers `status`,
//!     and refuses the ops that need a window with `unsupported_headless`;
//! *   [`handle_agent_requests`] (windowed build only) answers everything,
//!     changing the highlight, the selection and the class filter.
//!
//! The decisions are made by [`handle_windowed`], which takes plain values and
//! no Bevy resources, so they are tested without a window.
//!
//! **The agent never locks the UI.** It writes the same [`Selection`] and
//! [`ClassFilter`] the user clicks on, so the user can undo any of it with the
//! same panels. [`UserActivityTracker`] tells the agent when the user changed
//! something (`user.epoch`), and [`UserViewCheckpoint`] remembers the user's
//! view so that `Esc` (or the *Restore my view* chip, or `viewer_clear` with
//! `restore`) can give it back.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use bevy::camera_controller::pan_orbit_camera::prelude::PanOrbitCamera;

use crate::camera::{
    bounding_sphere, estimate_look_at, fov_of, frame_pose, zoom_pose, CameraFocus, CameraMove,
    CameraPose, PendingCameraMove, SceneCamera,
};
use crate::connection::{AgentRequest, AgentResponse, ConnState, ViewerMsg};
use crate::graph::ViewerGraph;
use crate::ingest::{ActivityFeed, AgentFeedOp, FeedKind};
use crate::interact::{filter_entries, ClassFilter, Selection};
use crate::layout::LayoutState;
use crate::plugin::{ConnStatus, CoreSet, FrameMessages};

/// The user counts as active for this long after the last input.
const USER_ACTIVE_FOR: Duration = Duration::from_secs(3);
/// Most IRIs listed in an answer.
const LISTED_MAX: usize = 50;

/// Where answers go, and this process's instance id.
#[derive(Resource)]
pub struct AgentOutbox {
    pub responses: UnboundedSender<AgentResponse>,
    pub instance: String,
}

/// Present when this build has a window, so that [`answer_core_requests`]
/// leaves every request to [`handle_agent_requests`].
#[derive(Resource)]
pub struct WindowedAgent;

// ── The agent's marks on the scene ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HighlightMode {
    /// The nodes glow; the rest is left as it was.
    #[default]
    Highlight,
    /// The nodes glow and the rest fades.
    Isolate,
}

impl HighlightMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Highlight => "highlight",
            Self::Isolate => "isolate",
        }
    }
}

/// The nodes the agent is pointing at, keyed by N-Triples term (`<iri>`) so the
/// highlight survives a snapshot rebuild, as [`Selection`] does.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct AgentHighlight {
    pub terms: HashSet<String>,
    pub mode: HighlightMode,
}

impl AgentHighlight {
    pub fn is_active(&self) -> bool {
        !self.terms.is_empty()
    }

    pub fn contains(&self, term: &str) -> bool {
        self.terms.contains(term)
    }

    /// Whether, in isolate mode, `term` is one of the nodes left in the light.
    pub fn isolates(&self) -> bool {
        self.is_active() && self.mode == HighlightMode::Isolate
    }
}

/// What the user sees that an agent command may change: the selection and the
/// class filter.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ViewState {
    pub selection: Option<String>,
    pub hidden: HashSet<String>,
    pub show_untyped: bool,
}

impl ViewState {
    pub fn capture(selection: &Selection, filter: &ClassFilter) -> Self {
        Self {
            selection: selection.0.clone(),
            hidden: filter.hidden.clone(),
            show_untyped: filter.show_untyped,
        }
    }

    pub fn apply(&self, selection: &mut Selection, filter: &mut ClassFilter) {
        selection.0 = self.selection.clone();
        filter.hidden = self.hidden.clone();
        filter.show_untyped = self.show_untyped;
    }
}

/// Counts what the user does, so the agent can tell when to step back.
///
/// A change to the selection or the filter that the agent did not make is a
/// user change: `epoch` goes up and `overrode` is set. The agent's own writes
/// are recorded with [`Self::agent_wrote`] and do not count.
#[derive(Resource, Debug, Default)]
pub struct UserActivityTracker {
    pub epoch: u64,
    /// The user changed the view after the agent's last command.
    pub overrode: bool,
    pub last_input: Option<Instant>,
    /// The view as of the last look, to tell a change from no change.
    known: Option<ViewState>,
    /// The epoch when the agent last wrote.
    epoch_at_last_agent: Option<u64>,
}

impl UserActivityTracker {
    pub fn note_input(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    /// Compare the view with the last look. Returns whether it is a user change.
    pub fn observe(&mut self, view: &ViewState) -> bool {
        match &self.known {
            None => {
                self.known = Some(view.clone());
                false
            }
            Some(known) if known == view => false,
            Some(_) => {
                self.known = Some(view.clone());
                self.epoch += 1;
                self.overrode = true;
                true
            }
        }
    }

    /// The user moved the camera (an orbit, a pan, a zoom, or `F`).
    pub fn user_camera_moved(&mut self) {
        self.epoch += 1;
        self.overrode = true;
    }

    /// The agent wrote `view`: that is not the user's doing.
    pub fn agent_wrote(&mut self, view: &ViewState) {
        self.known = Some(view.clone());
        self.overrode = false;
        self.epoch_at_last_agent = Some(self.epoch);
    }

    /// The user changed the view since the agent last wrote (or the agent has
    /// not written yet).
    pub fn user_moved_since_agent(&self) -> bool {
        self.epoch_at_last_agent != Some(self.epoch)
    }

    /// The `user` block of a response, as of `now`.
    pub fn user_block(&self, now: Instant) -> Value {
        let active = self
            .last_input
            .is_some_and(|t| now.saturating_duration_since(t) < USER_ACTIVE_FOR);
        json!({
            "active": active,
            "epoch": self.epoch,
            "overrode_last_agent_command": self.overrode,
        })
    }
}

/// One level of "the user's view before the agent changed it".
#[derive(Resource, Default, Debug)]
pub struct UserViewCheckpoint {
    pub saved: Option<ViewState>,
    /// Where the camera was, if an agent command has moved it since.
    pub camera: Option<Transform>,
}

impl UserViewCheckpoint {
    /// Save the view before an agent command changes it, unless the agent has
    /// been the only one changing it since the last save.
    ///
    /// `camera` is the camera's pose when the command is about to move it. The
    /// pose is kept only if the user's own pose is still what the camera has
    /// (nothing of the agent's has moved it since the last save).
    pub fn save_before_agent_change(
        &mut self,
        tracker: &UserActivityTracker,
        view: ViewState,
        camera: Option<Transform>,
    ) {
        if self.saved.is_none() || tracker.user_moved_since_agent() {
            self.saved = Some(view);
            self.camera = camera;
        } else if self.camera.is_none() {
            self.camera = camera;
        }
    }
}

/// What a restore gave back.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Restored {
    /// There was a restore point.
    pub had_checkpoint: bool,
    /// The pose to move the camera back to, if an agent command had moved it.
    pub camera: Option<Transform>,
}

/// How long the camera takes to go back when the user's view is restored.
pub const RESTORE_MS: u64 = 600;

impl Restored {
    /// The camera move that puts the camera back, if there is one.
    pub fn camera_move(&self) -> Option<CameraMove> {
        self.camera.map(|end| CameraMove {
            end,
            look_at: None,
            duration_ms: RESTORE_MS,
        })
    }
}

/// Give the user's view back: drop the highlight, and return the selection and
/// the filter to what the checkpoint holds. Consumes the checkpoint. The camera
/// is not touched here: the pose to go back to is in the result.
pub fn restore_view(
    selection: &mut Selection,
    filter: &mut ClassFilter,
    highlight: &mut AgentHighlight,
    checkpoint: &mut UserViewCheckpoint,
) -> Restored {
    highlight.terms.clear();
    let camera = checkpoint.camera.take();
    match checkpoint.saved.take() {
        Some(view) => {
            view.apply(selection, filter);
            Restored {
                had_checkpoint: true,
                camera,
            }
        }
        None => Restored::default(),
    }
}

/// What one `Esc` does, one thing per press: clear a selection; else give the
/// user's view back; else clear the agent highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeStep {
    ClearSelection,
    Restore,
    ClearHighlight,
    Nothing,
}

pub fn escape_step(
    selection: &Selection,
    checkpoint: &UserViewCheckpoint,
    highlight: &AgentHighlight,
) -> EscapeStep {
    if selection.0.is_some() {
        EscapeStep::ClearSelection
    } else if checkpoint.saved.is_some() {
        EscapeStep::Restore
    } else if highlight.is_active() {
        EscapeStep::ClearHighlight
    } else {
        EscapeStep::Nothing
    }
}

// ── Answering ────────────────────────────────────────────────────────────────

fn term_of(iri: &str) -> String {
    format!("<{iri}>")
}

fn iri_of(term: &str) -> &str {
    term.strip_prefix('<')
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(term)
}

/// Split `iris` into those the graph draws (as terms) and those it does not.
pub fn resolve_iris(graph: &ViewerGraph, iris: &[String]) -> (Vec<String>, Vec<String>) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for iri in iris {
        if !seen.insert(iri.as_str()) {
            continue;
        }
        let term = term_of(iri);
        if graph.index.contains_key(&term) {
            found.push(term);
        } else {
            missing.push(iri.clone());
        }
    }
    (found, missing)
}

/// The connection state as the string `status` reports.
fn connection_name(state: &ConnState) -> &'static str {
    match state {
        ConnState::Streaming => "streaming",
        ConnState::Connecting => "connecting",
        ConnState::AwaitSnapshot => "await_snapshot",
        ConnState::Backoff { .. } => "backoff",
    }
}

/// What `status` reports in every build: who this viewer is, which graph it
/// shows and how big it is.
pub fn status_snapshot(instance: &str, graph: &ViewerGraph, state: &ConnState) -> Value {
    let nodes = graph.nodes.iter().filter(|n| n.is_some()).count();
    json!({
        "instance": instance,
        "graph": graph.graph,
        "nodes": nodes,
        "edges": graph.edges.len(),
        "connection": connection_name(state),
        "window": false,
    })
}

/// The ops the core can send.
const WINDOW_OPS: [&str; 4] = ["camera", "show", "clear", "filter"];

/// Answer one request in a build without a window.
pub fn answer(
    request: &AgentRequest,
    instance: &str,
    graph: &ViewerGraph,
    state: &ConnState,
    user: Value,
) -> AgentResponse {
    match request.op.as_str() {
        "status" => AgentResponse::ok(request.id, status_snapshot(instance, graph, state), user),
        op if WINDOW_OPS.contains(&op) || op == "screenshot" => AgentResponse::err(
            request.id,
            "unsupported_headless",
            &format!("the op {op:?} needs a window, and this viewer runs without one"),
        ),
        other => AgentResponse::err(
            request.id,
            "unknown_op",
            &format!("this viewer does not support the op {other:?}"),
        ),
    }
}

/// What the viewer's state looks like to [`handle_windowed`].
pub struct Ctx<'a> {
    pub instance: &'a str,
    pub graph: &'a ViewerGraph,
    pub conn: &'a ConnState,
    pub selection: &'a mut Selection,
    pub filter: &'a mut ClassFilter,
    pub highlight: &'a mut AgentHighlight,
    pub tracker: &'a mut UserActivityTracker,
    pub checkpoint: &'a mut UserViewCheckpoint,
    /// The scene camera, when there is one (a viewer with a window).
    pub camera: Option<CameraView<'a>>,
    pub now: Instant,
}

/// What the camera commands need to know, as plain values.
pub struct CameraView<'a> {
    pub transform: Transform,
    pub fov_y: f32,
    /// The user is orbiting, panning or zooming right now.
    pub user_controlled: bool,
    /// The point the viewer last framed, if the user has not moved since.
    pub focus: Option<Vec3>,
    /// Where each node is, by node id.
    pub positions: &'a [Vec3],
}

impl CameraView<'_> {
    fn all_positions(&self, graph: &ViewerGraph) -> Vec<Vec3> {
        graph
            .nodes
            .iter()
            .zip(self.positions)
            .filter(|(node, _)| node.is_some())
            .map(|(_, p)| *p)
            .collect()
    }

    fn positions_of(&self, graph: &ViewerGraph, terms: &[String]) -> Vec<Vec3> {
        terms
            .iter()
            .filter_map(|t| graph.index.get(t))
            .filter_map(|id| self.positions.get(id.0 as usize).copied())
            .collect()
    }

    /// The pose the camera has now.
    fn pose(&self, graph: &ViewerGraph) -> CameraPose {
        let centre = bounding_sphere(&self.all_positions(graph)).map(|(c, _)| c);
        CameraPose {
            position: self.transform.translation,
            look_at: estimate_look_at(&self.transform, self.focus, centre),
        }
    }
}

fn vec3_json(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

/// A pose as the answers show it.
fn pose_json(pose: CameraPose) -> Value {
    json!({
        "position": vec3_json(pose.position),
        "look_at": vec3_json(pose.look_at),
        "distance": pose.distance(),
    })
}

/// The answer, and what to show in the activity feed.
pub struct Handled {
    pub response: AgentResponse,
    pub feed: Option<(AgentFeedOp, u64)>,
    /// A camera move to start.
    pub camera_move: Option<CameraMove>,
}

impl Handled {
    fn answer(response: AgentResponse) -> Self {
        Self {
            response,
            feed: None,
            camera_move: None,
        }
    }

    fn with_feed(response: AgentResponse, op: AgentFeedOp, count: u64) -> Self {
        Self {
            response,
            feed: Some((op, count)),
            camera_move: None,
        }
    }

    fn moving(mut self, camera_move: Option<CameraMove>) -> Self {
        self.camera_move = camera_move;
        self
    }
}

fn listed(items: &[String]) -> Vec<&String> {
    items.iter().take(LISTED_MAX).collect()
}

fn args_strings(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A mutating request carrying `if_user_epoch` is refused when the user has
/// changed something since.
fn stale(request: &AgentRequest, tracker: &UserActivityTracker) -> Option<AgentResponse> {
    let wanted = request.args.get("if_user_epoch").and_then(Value::as_u64)?;
    (wanted != tracker.epoch).then(|| {
        AgentResponse::err(
            request.id,
            "user_changed",
            &format!(
                "the user changed the view (epoch {} now, {wanted} when you looked); \
                 call viewer_status and decide whether to go on",
                tracker.epoch
            ),
        )
    })
}

/// Answer one request in a build with a window, changing what it asks to change.
pub fn handle_windowed(request: &AgentRequest, ctx: &mut Ctx) -> Handled {
    // Computed before this request is applied.
    let user = ctx.tracker.user_block(ctx.now);
    match request.op.as_str() {
        "status" => {
            let mut result = status_snapshot(ctx.instance, ctx.graph, ctx.conn);
            result["window"] = json!(true);
            result["selection"] = json!(ctx.selection.0.as_deref().map(iri_of));
            let mut hidden: Vec<&str> = ctx.filter.hidden.iter().map(|t| iri_of(t)).collect();
            hidden.sort_unstable();
            result["filter"] = json!({
                "hidden_classes": hidden,
                "show_untyped": ctx.filter.show_untyped,
            });
            result["highlight"] = json!({
                "active": ctx.highlight.is_active(),
                "mode": ctx.highlight.mode.name(),
                "count": ctx.highlight.terms.len(),
            });
            result["has_restore_point"] =
                json!(ctx.checkpoint.saved.is_some() || ctx.checkpoint.camera.is_some());
            if let Some(cam) = &ctx.camera {
                result["camera"] = pose_json(cam.pose(ctx.graph));
                result["camera"]["user_controlling"] = json!(cam.user_controlled);
            }
            Handled::answer(AgentResponse::ok(request.id, result, user))
        }
        "show" => show(request, ctx, user),
        "clear" => clear(request, ctx, user),
        "filter" => filter(request, ctx, user),
        "camera" => camera(request, ctx, user),
        other => Handled::answer(AgentResponse::err(
            request.id,
            "unknown_op",
            &format!("this viewer does not support the op {other:?} yet"),
        )),
    }
}

fn show(request: &AgentRequest, ctx: &mut Ctx, user: Value) -> Handled {
    if let Some(refusal) = stale(request, ctx.tracker) {
        return Handled::answer(refusal);
    }
    let args = &request.args;
    let iris = args_strings(args, "iris");
    let mode = match args.get("mode").and_then(Value::as_str) {
        Some("isolate") => HighlightMode::Isolate,
        _ => HighlightMode::Highlight,
    };
    let focus = args.get("focus").and_then(Value::as_bool).unwrap_or(true);
    let select = args.get("select").and_then(Value::as_str).map(term_of);

    let (found, missing) = resolve_iris(ctx.graph, &iris);
    if found.is_empty() {
        return Handled::answer(AgentResponse::err(
            request.id,
            "not_drawn",
            &format!(
                "none of the {} IRIs is drawn (for example {:?}); nothing changed",
                iris.len(),
                missing.first().map(String::as_str).unwrap_or("")
            ),
        ));
    }
    if let Some(term) = &select {
        if !ctx.graph.index.contains_key(term) {
            return Handled::answer(AgentResponse::err(
                request.id,
                "not_drawn",
                &format!(
                    "{:?} cannot be selected: it is not drawn; nothing changed",
                    iri_of(term)
                ),
            ));
        }
    }

    // The camera: frame what is shown, unless the user has it.
    let mut camera_note: Option<&str> = None;
    let mut camera_move = None;
    let mut pose = None;
    if focus {
        match &ctx.camera {
            None => camera_note = Some("camera_unavailable: this viewer has no camera"),
            Some(cam) if cam.user_controlled => {
                camera_note = Some("user_controlling_camera: the user is moving the camera");
            }
            Some(cam) => match frame_pose(
                cam.transform.back().as_vec3(),
                &cam.positions_of(ctx.graph, &found),
                cam.fov_y,
            ) {
                Some(p) => {
                    pose = Some(p);
                    camera_move = Some(CameraMove::to(p, DEFAULT_MOVE_MS));
                }
                None => camera_note = Some("camera_not_moved: the nodes have no position yet"),
            },
        }
    }

    // Everything is checked: now change things.
    if select.is_some() || camera_move.is_some() {
        let before = ViewState::capture(ctx.selection, ctx.filter);
        let from = camera_move
            .as_ref()
            .and_then(|_| ctx.camera.as_ref().map(|c| c.transform));
        ctx.checkpoint
            .save_before_agent_change(ctx.tracker, before, from);
    }
    ctx.highlight.terms = found.iter().cloned().collect();
    ctx.highlight.mode = mode;
    if let Some(term) = &select {
        ctx.selection.0 = Some(term.clone());
    }
    if select.is_some() || camera_move.is_some() {
        ctx.tracker
            .agent_wrote(&ViewState::capture(ctx.selection, ctx.filter));
    }

    let mut result = json!({
        "shown": found.len(),
        "missing": listed(&missing),
        "missing_count": missing.len(),
        "mode": mode.name(),
        "camera_moved": camera_move.is_some(),
    });
    if let Some(term) = &select {
        result["selected"] = json!(iri_of(term));
    }
    if let Some(p) = pose {
        result["camera"] = pose_json(p);
    }
    let response = match camera_note {
        Some(note) => AgentResponse::ok_with_reason(request.id, result, user, note),
        None => AgentResponse::ok(request.id, result, user),
    };
    let op = match mode {
        HighlightMode::Highlight => AgentFeedOp::Highlight,
        HighlightMode::Isolate => AgentFeedOp::Isolate,
    };
    Handled::with_feed(response, op, found.len() as u64).moving(camera_move)
}

fn clear(request: &AgentRequest, ctx: &mut Ctx, user: Value) -> Handled {
    let restore = request
        .args
        .get("restore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let had_highlight = ctx.highlight.is_active();
    if !restore {
        ctx.highlight.terms.clear();
        return Handled::with_feed(
            AgentResponse::ok(
                request.id,
                json!({ "cleared": had_highlight, "restored": false, "camera_restored": false }),
                user,
            ),
            AgentFeedOp::Clear,
            0,
        );
    }
    let back = restore_view(ctx.selection, ctx.filter, ctx.highlight, ctx.checkpoint);
    ctx.tracker
        .agent_wrote(&ViewState::capture(ctx.selection, ctx.filter));
    // The camera goes back too, unless the user has it in their hands.
    let user_has_camera = ctx.camera.as_ref().is_some_and(|c| c.user_controlled);
    let camera_move = if user_has_camera || ctx.camera.is_none() {
        None
    } else {
        back.camera_move()
    };
    let restored = back.had_checkpoint;
    let result = json!({
        "cleared": had_highlight,
        "restored": restored,
        "camera_restored": camera_move.is_some(),
    });
    let response = if restored {
        AgentResponse::ok(request.id, result, user)
    } else {
        AgentResponse::ok_with_reason(
            request.id,
            result,
            user,
            "no_restore_point: the agent had not changed the user's view",
        )
    };
    Handled::with_feed(response, AgentFeedOp::Restore, 0).moving(camera_move)
}

/// How long a move takes when nobody says.
pub const DEFAULT_MOVE_MS: u64 = 600;

fn camera(request: &AgentRequest, ctx: &mut Ctx, user: Value) -> Handled {
    if let Some(refusal) = stale(request, ctx.tracker) {
        return Handled::answer(refusal);
    }
    let Some(cam) = ctx.camera.as_ref() else {
        return Handled::answer(AgentResponse::err(
            request.id,
            "unsupported_headless",
            "this viewer has no camera",
        ));
    };
    if cam.user_controlled {
        return Handled::answer(AgentResponse::err(
            request.id,
            "user_controlling_camera",
            "the user is moving the camera; nothing was moved",
        ));
    }
    let args = &request.args;
    let duration_ms = args
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MOVE_MS);
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    let back = cam.transform.back().as_vec3();
    let (pose, framed) = match action {
        "frame_all" => match frame_pose(back, &cam.all_positions(ctx.graph), cam.fov_y) {
            Some(p) => (p, ctx.graph.index.len()),
            None => {
                return Handled::answer(AgentResponse::err(
                    request.id,
                    "nothing_drawn",
                    "there is nothing to frame",
                ))
            }
        },
        "frame" => {
            let iris = args_strings(args, "iris");
            let (found, missing) = resolve_iris(ctx.graph, &iris);
            if found.is_empty() {
                return Handled::answer(AgentResponse::err(
                    request.id,
                    "not_drawn",
                    &format!(
                        "none of the {} IRIs is drawn (for example {:?}); the camera did not move",
                        iris.len(),
                        missing.first().map(String::as_str).unwrap_or("")
                    ),
                ));
            }
            match frame_pose(back, &cam.positions_of(ctx.graph, &found), cam.fov_y) {
                Some(p) => (p, found.len()),
                None => {
                    return Handled::answer(AgentResponse::err(
                        request.id,
                        "nothing_drawn",
                        "the nodes have no position yet",
                    ))
                }
            }
        }
        "zoom" => {
            let factor = args.get("factor").and_then(Value::as_f64).unwrap_or(0.0);
            if !(factor.is_finite() && factor > 0.0) {
                return Handled::answer(AgentResponse::err(
                    request.id,
                    "invalid_argument",
                    "zoom needs a factor above 0",
                ));
            }
            (zoom_pose(cam.pose(ctx.graph), factor as f32), 0)
        }
        other => {
            return Handled::answer(AgentResponse::err(
                request.id,
                "invalid_argument",
                &format!("unknown camera action {other:?}"),
            ))
        }
    };

    let from = cam.transform;
    ctx.checkpoint.save_before_agent_change(
        ctx.tracker,
        ViewState::capture(ctx.selection, ctx.filter),
        Some(from),
    );
    ctx.tracker
        .agent_wrote(&ViewState::capture(ctx.selection, ctx.filter));
    let mut result = json!({
        "camera": pose_json(pose),
        "duration_ms": duration_ms,
        "camera_moved": true,
    });
    if framed > 0 {
        result["framed"] = json!(framed);
    }
    Handled::answer(AgentResponse::ok(request.id, result, user))
        .moving(Some(CameraMove::to(pose, duration_ms)))
}

fn filter(request: &AgentRequest, ctx: &mut Ctx, user: Value) -> Handled {
    if let Some(refusal) = stale(request, ctx.tracker) {
        return Handled::answer(refusal);
    }
    let args = &request.args;
    let hide = args_strings(args, "hide_classes");
    let show = args_strings(args, "show_classes");
    let show_untyped = args.get("show_untyped").and_then(Value::as_bool);
    let reset = args.get("reset").and_then(Value::as_bool).unwrap_or(false);

    // Only classes that type at least one drawn instance can be toggled: any
    // other would be a dead entry in the user's panel.
    let known: HashSet<String> = filter_entries(ctx.graph)
        .into_iter()
        .map(|e| e.term)
        .collect();
    let (mut unknown, mut changed) = (Vec::new(), 0u64);

    let before = ViewState::capture(ctx.selection, ctx.filter);
    ctx.checkpoint
        .save_before_agent_change(ctx.tracker, before, None);
    if reset {
        ctx.filter.hidden.clear();
        ctx.filter.show_untyped = true;
        changed += 1;
    }
    for iri in &hide {
        let term = term_of(iri);
        if known.contains(&term) {
            ctx.filter.hidden.insert(term);
            changed += 1;
        } else {
            unknown.push(iri.clone());
        }
    }
    for iri in &show {
        let term = term_of(iri);
        if known.contains(&term) || ctx.filter.hidden.contains(&term) {
            ctx.filter.hidden.remove(&term);
            changed += 1;
        } else {
            unknown.push(iri.clone());
        }
    }
    if let Some(on) = show_untyped {
        ctx.filter.show_untyped = on;
        changed += 1;
    }
    ctx.tracker
        .agent_wrote(&ViewState::capture(ctx.selection, ctx.filter));

    let mut hidden: Vec<String> = ctx
        .filter
        .hidden
        .iter()
        .map(|t| iri_of(t).to_string())
        .collect();
    hidden.sort_unstable();
    let result = json!({
        "hidden_classes": listed(&hidden),
        "hidden_count": hidden.len(),
        "show_untyped": ctx.filter.show_untyped,
        "unknown": listed(&unknown),
    });
    Handled::with_feed(
        AgentResponse::ok(request.id, result, user),
        AgentFeedOp::Filter,
        changed,
    )
}

// ── Bevy systems ─────────────────────────────────────────────────────────────

/// Answers every request that arrived this frame, in a build without a window.
pub fn answer_core_requests(
    frame: Res<FrameMessages>,
    outbox: Res<AgentOutbox>,
    graph: Res<ViewerGraph>,
    status: Res<ConnStatus>,
    windowed: Option<Res<WindowedAgent>>,
) {
    if windowed.is_some() {
        return; // `handle_agent_requests` answers them
    }
    for msg in &frame.0 {
        if let ViewerMsg::Request(request) = msg {
            let user = json!({ "active": false, "epoch": 0, "overrode_last_agent_command": false });
            let response = answer(request, &outbox.instance, &graph, &status.0, user);
            // A closed channel means the connection thread is gone: nothing to do.
            let _ = outbox.responses.send(response);
        }
    }
}

/// Answers every request that arrived this frame, and applies it.
#[allow(clippy::too_many_arguments)]
pub fn handle_agent_requests(
    frame: Res<FrameMessages>,
    outbox: Res<AgentOutbox>,
    graph: Res<ViewerGraph>,
    status: Res<ConnStatus>,
    mut selection: ResMut<Selection>,
    mut filter: ResMut<ClassFilter>,
    mut highlight: ResMut<AgentHighlight>,
    mut tracker: ResMut<UserActivityTracker>,
    mut checkpoint: ResMut<UserViewCheckpoint>,
    mut feed: ResMut<ActivityFeed>,
    layout: Option<Res<LayoutState>>,
    focus: Res<CameraFocus>,
    camera: Query<(&Transform, &Projection, &PanOrbitCamera), With<SceneCamera>>,
    mut pending: ResMut<PendingCameraMove>,
) {
    let cam = camera.single().ok();
    let positions: &[Vec3] = layout.as_deref().map_or(&[], |l| l.pos.as_slice());
    for msg in &frame.0 {
        let ViewerMsg::Request(request) = msg else {
            continue;
        };
        let handled = handle_windowed(
            request,
            &mut Ctx {
                instance: &outbox.instance,
                graph: &graph,
                conn: &status.0,
                selection: &mut selection,
                filter: &mut filter,
                highlight: &mut highlight,
                tracker: &mut tracker,
                checkpoint: &mut checkpoint,
                camera: cam.map(|(transform, projection, controller)| CameraView {
                    transform: *transform,
                    fov_y: fov_of(projection),
                    user_controlled: controller.current_motion.is_user_controlled(),
                    focus: focus.look_at,
                    positions,
                }),
                now: Instant::now(),
            },
        );
        if let Some(mv) = handled.camera_move {
            pending.0 = Some(mv);
        }
        if let Some((op, count)) = handled.feed {
            feed.push(FeedKind::Agent(op), count);
        }
        let _ = outbox.responses.send(handled.response);
    }
}

/// Notices what the user does: input (for `user.active`), and any change to the
/// selection or the filter that the agent did not make (for `user.epoch`).
/// Runs before the agent's systems, so a request sees the user's latest move.
#[allow(clippy::too_many_arguments)]
pub fn track_user_changes(
    selection: Res<Selection>,
    filter: Res<ClassFilter>,
    mut tracker: ResMut<UserActivityTracker>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    motion: Option<Res<bevy::input::mouse::AccumulatedMouseMotion>>,
    scroll: Option<Res<bevy::input::mouse::AccumulatedMouseScroll>>,
    camera: Query<&PanOrbitCamera, With<SceneCamera>>,
    mut was_moving: Local<bool>,
) {
    let input = keys.as_ref().is_some_and(|k| {
        k.get_just_pressed().next().is_some() || k.get_just_released().next().is_some()
    }) || buttons.is_some_and(|b| {
        b.get_pressed().next().is_some() || b.get_just_released().next().is_some()
    }) || motion.is_some_and(|m| m.delta != Vec2::ZERO)
        || scroll.is_some_and(|s| s.delta != Vec2::ZERO);
    if input {
        tracker.note_input(Instant::now());
    }
    // The user takes the camera (the start of an orbit, a pan or a zoom), or
    // frames the graph with `F`: either is a change the agent should know of.
    let moving = camera.iter().any(|c| c.current_motion.is_user_controlled());
    let framed = keys.as_ref().is_some_and(|k| k.just_pressed(KeyCode::KeyF));
    if (moving && !*was_moving) || framed {
        tracker.user_camera_moved();
    }
    *was_moving = moving;
    if selection.is_changed() || filter.is_changed() || tracker.is_added() {
        tracker.observe(&ViewState::capture(&selection, &filter));
    }
}

/// One `Esc` does one thing: clear the selection, else restore the user's view,
/// else clear the agent highlight.
pub fn escape_chain(
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<Selection>,
    mut filter: ResMut<ClassFilter>,
    mut highlight: ResMut<AgentHighlight>,
    mut checkpoint: ResMut<UserViewCheckpoint>,
    mut pending: ResMut<PendingCameraMove>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    match escape_step(&selection, &checkpoint, &highlight) {
        EscapeStep::ClearSelection => selection.0 = None,
        EscapeStep::Restore => {
            let back = restore_view(&mut selection, &mut filter, &mut highlight, &mut checkpoint);
            pending.0 = back.camera_move();
        }
        EscapeStep::ClearHighlight => highlight.terms.clear(),
        EscapeStep::Nothing => {}
    }
}

/// The *Agent view · Restore my view* chip, shown while the agent has a
/// highlight up or a restore point exists.
#[derive(Component)]
struct RestoreChip;

fn spawn_restore_chip(mut commands: Commands) {
    commands
        .spawn((
            RestoreChip,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                top: Val::Px(12.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.3, 0.35, 0.92)),
            bevy::picking::hover::Hovered::default(),
            Visibility::Hidden,
        ))
        .with_child((
            Text::new("Agent view · Restore my view"),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ))
        .observe(
            |_: On<PointerClick>,
             mut selection: ResMut<Selection>,
             mut filter: ResMut<ClassFilter>,
             mut highlight: ResMut<AgentHighlight>,
             mut checkpoint: ResMut<UserViewCheckpoint>,
             mut pending: ResMut<PendingCameraMove>| {
                let back =
                    restore_view(&mut selection, &mut filter, &mut highlight, &mut checkpoint);
                pending.0 = back.camera_move();
            },
        );
}

fn update_restore_chip(
    highlight: Res<AgentHighlight>,
    checkpoint: Res<UserViewCheckpoint>,
    mut chip: Query<&mut Visibility, With<RestoreChip>>,
) {
    let wanted =
        if highlight.is_active() || checkpoint.saved.is_some() || checkpoint.camera.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    for mut visibility in &mut chip {
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Adds the answering system for builds without a window; `CorePlugin` adds
/// it, so the headless path answers too.
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

/// The windowed side: the agent's marks, the user tracker, the restore chip and
/// the handler that applies requests. Added by `ViewerPlugin`.
pub struct AgentViewPlugin;

impl Plugin for AgentViewPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WindowedAgent)
            .init_resource::<AgentHighlight>()
            .init_resource::<UserActivityTracker>()
            .init_resource::<UserViewCheckpoint>()
            .init_resource::<CameraFocus>()
            .init_resource::<PendingCameraMove>()
            .add_systems(Startup, spawn_restore_chip)
            .add_systems(
                Update,
                (
                    track_user_changes
                        .in_set(CoreSet::Net)
                        .after(crate::plugin::drain_inbox),
                    handle_agent_requests
                        .in_set(CoreSet::Graph)
                        .after(crate::plugin::apply_to_graph),
                    update_restore_chip.in_set(CoreSet::Ui),
                ),
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

    /// Two classes (`A` types a1, a2; `B` types b1) and one untyped link.
    fn scene() -> ViewerGraph {
        const T: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        graph_with(&[
            ["<http://e/a1>", T, "<http://e/A>"],
            ["<http://e/a2>", T, "<http://e/A>"],
            ["<http://e/b1>", T, "<http://e/B>"],
            ["<http://e/a1>", "<http://e/knows>", "<http://e/b1>"],
            ["<http://e/lonely>", "<http://e/knows>", "<http://e/a2>"],
        ])
    }

    fn request(op: &str, args: Value) -> AgentRequest {
        AgentRequest {
            id: 9,
            op: op.into(),
            args,
        }
    }

    #[derive(Default)]
    struct World {
        selection: Selection,
        filter: ClassFilter,
        highlight: AgentHighlight,
        tracker: UserActivityTracker,
        checkpoint: UserViewCheckpoint,
        cam: Option<TestCamera>,
    }

    /// A camera, as plain values.
    struct TestCamera {
        transform: Transform,
        user_controlled: bool,
        positions: Vec<Vec3>,
    }

    const FOV: f32 = std::f32::consts::FRAC_PI_4;

    impl World {
        /// A camera 60 units from the origin, and nodes 10 apart along x.
        fn with_camera(graph: &ViewerGraph) -> Self {
            World {
                cam: Some(TestCamera {
                    transform: Transform::from_xyz(0.0, 0.0, 60.0).looking_at(Vec3::ZERO, Vec3::Y),
                    user_controlled: false,
                    positions: (0..graph.nodes.len())
                        .map(|i| Vec3::new(10.0 * i as f32, 0.0, 0.0))
                        .collect(),
                }),
                ..World::default()
            }
        }

        fn position_of(&self, graph: &ViewerGraph, iri: &str) -> Vec3 {
            let id = graph.index[&format!("<{iri}>")];
            self.cam.as_ref().unwrap().positions[id.0 as usize]
        }

        fn handle(&mut self, graph: &ViewerGraph, req: &AgentRequest) -> Handled {
            handle_windowed(
                req,
                &mut Ctx {
                    instance: "abc",
                    graph,
                    conn: &ConnState::Streaming,
                    selection: &mut self.selection,
                    filter: &mut self.filter,
                    highlight: &mut self.highlight,
                    tracker: &mut self.tracker,
                    checkpoint: &mut self.checkpoint,
                    camera: self.cam.as_ref().map(|c| CameraView {
                        transform: c.transform,
                        fov_y: FOV,
                        user_controlled: c.user_controlled,
                        focus: None,
                        positions: &c.positions,
                    }),
                    now: Instant::now(),
                },
            )
        }

        /// What the user's click or key would have done: a change the tracker sees.
        fn user_clicks(&mut self, term: &str) {
            // The first look only records where things stand, as at startup.
            self.tracker
                .observe(&ViewState::capture(&self.selection, &self.filter));
            self.selection.0 = Some(term.into());
            self.tracker
                .observe(&ViewState::capture(&self.selection, &self.filter));
        }
    }

    fn body(h: &Handled) -> &Value {
        &h.response.0
    }

    // ── headless ──

    #[test]
    fn status_reports_instance_graph_counts_and_connection() {
        let g = graph_with(&[["<urn:a>", "<urn:p>", "<urn:b>"]]);
        let r = answer(
            &request("status", Value::Null),
            "abc",
            &g,
            &ConnState::Streaming,
            json!({ "epoch": 0 }),
        );
        let b = &r.0;
        assert_eq!((b["id"].clone(), b["ok"].clone()), (json!(9), json!(true)));
        assert_eq!(b["result"]["instance"], "abc");
        assert_eq!(b["result"]["graph"], "urn:project:default");
        assert_eq!(
            (b["result"]["nodes"].clone(), b["result"]["edges"].clone()),
            (json!(2), json!(1))
        );
        assert_eq!(b["result"]["connection"], "streaming");
        assert_eq!(b["result"]["window"], false);
    }

    #[test]
    fn without_a_window_the_ops_that_need_one_say_so() {
        for op in ["show", "clear", "filter", "camera", "screenshot"] {
            let r = answer(
                &request(op, Value::Null),
                "abc",
                &ViewerGraph::empty(),
                &ConnState::Streaming,
                json!({}),
            );
            assert_eq!(r.0["error"]["code"], "unsupported_headless", "{op}");
        }
        let r = answer(
            &request("teleport", Value::Null),
            "abc",
            &ViewerGraph::empty(),
            &ConnState::Streaming,
            json!({}),
        );
        assert_eq!(r.0["error"]["code"], "unknown_op");
    }

    // ── resolving IRIs ──

    #[test]
    fn iris_are_resolved_against_what_is_drawn_without_duplicates() {
        let g = scene();
        let (found, missing) = resolve_iris(
            &g,
            &[
                "http://e/a1".into(),
                "http://e/nope".into(),
                "http://e/a1".into(),
            ],
        );
        assert_eq!(found, vec!["<http://e/a1>".to_string()]);
        assert_eq!(missing, vec!["http://e/nope".to_string()]);
    }

    // ── show ──

    #[test]
    fn show_highlights_what_is_drawn_and_reports_what_is_not() {
        let g = scene();
        let mut w = World::default();
        let h = w.handle(&g, &request("show", json!({ "iris": ["http://e/a1", "http://e/ghost"], "mode": "isolate", "focus": false })));
        let b = body(&h);
        assert_eq!(b["ok"], true);
        assert_eq!(b["result"]["shown"], 1);
        assert_eq!(b["result"]["missing"], json!(["http://e/ghost"]));
        assert_eq!(b["result"]["mode"], "isolate");
        assert_eq!(b["result"]["camera_moved"], false);
        assert!(
            b.get("reason").is_none(),
            "no focus asked, nothing to excuse"
        );
        assert!(w.highlight.contains("<http://e/a1>") && w.highlight.isolates());
        assert!(w.selection.0.is_none(), "no select asked");
        assert!(
            w.checkpoint.saved.is_none(),
            "a highlight alone changes nothing the user set"
        );
        assert!(matches!(h.feed, Some((AgentFeedOp::Isolate, 1))));
    }

    #[test]
    fn a_viewer_with_no_camera_says_so_when_focus_is_asked() {
        let g = scene();
        let mut w = World::default();
        let h = w.handle(
            &g,
            &request("show", json!({ "iris": ["http://e/a1"], "focus": true })),
        );
        assert_eq!(body(&h)["result"]["camera_moved"], false);
        assert!(body(&h)["reason"]
            .as_str()
            .unwrap()
            .starts_with("camera_unavailable"));
        assert!(h.camera_move.is_none());
    }

    #[test]
    fn show_with_nothing_drawn_changes_nothing_and_says_why() {
        let g = scene();
        let mut w = World::default();
        w.highlight.terms.insert("<http://e/b1>".into());
        let h = w.handle(&g, &request("show", json!({ "iris": ["http://e/ghost"] })));
        assert_eq!(body(&h)["ok"], false);
        assert_eq!(body(&h)["error"]["code"], "not_drawn");
        assert!(
            w.highlight.contains("<http://e/b1>"),
            "the earlier highlight stays"
        );
        assert!(h.feed.is_none());
    }

    #[test]
    fn show_replaces_the_previous_highlight() {
        let g = scene();
        let mut w = World::default();
        w.handle(&g, &request("show", json!({ "iris": ["http://e/a1"] })));
        w.handle(&g, &request("show", json!({ "iris": ["http://e/b1"] })));
        assert_eq!(w.highlight.terms.len(), 1);
        assert!(w.highlight.contains("<http://e/b1>"));
        assert_eq!(w.highlight.mode, HighlightMode::Highlight);
    }

    #[test]
    fn select_opens_the_node_and_remembers_the_view_it_replaced() {
        let g = scene();
        let mut w = World::default();
        w.selection.0 = Some("<http://e/a2>".into());
        w.tracker
            .observe(&ViewState::capture(&w.selection, &w.filter));
        let h = w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/a1"], "select": "http://e/a1", "focus": false }),
            ),
        );
        assert_eq!(body(&h)["result"]["selected"], "http://e/a1");
        assert_eq!(w.selection.0.as_deref(), Some("<http://e/a1>"));
        assert_eq!(
            w.checkpoint.saved.as_ref().unwrap().selection.as_deref(),
            Some("<http://e/a2>")
        );
    }

    #[test]
    fn a_select_that_is_not_drawn_changes_nothing() {
        let g = scene();
        let mut w = World::default();
        let h = w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/a1"], "select": "http://e/ghost" }),
            ),
        );
        assert_eq!(body(&h)["error"]["code"], "not_drawn");
        assert!(
            !w.highlight.is_active() && w.selection.0.is_none() && w.checkpoint.saved.is_none()
        );
    }

    // ── the user's side ──

    #[test]
    fn a_command_written_for_an_old_epoch_is_refused_and_changes_nothing() {
        let g = scene();
        let mut w = World::default();
        w.user_clicks("<http://e/b1>"); // epoch 1: the user did something
        let h = w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/a1"], "if_user_epoch": 0 }),
            ),
        );
        assert_eq!(body(&h)["error"]["code"], "user_changed");
        assert!(!w.highlight.is_active());
        let h = w.handle(
            &g,
            &request(
                "filter",
                json!({ "hide_classes": ["http://e/A"], "if_user_epoch": 0 }),
            ),
        );
        assert_eq!(body(&h)["error"]["code"], "user_changed");
        assert!(w.filter.hidden.is_empty());
        // With the current epoch it goes through.
        let h = w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/a1"], "if_user_epoch": 1 }),
            ),
        );
        assert_eq!(body(&h)["ok"], true);
    }

    #[test]
    fn the_agents_own_writes_are_not_user_changes_but_the_users_are() {
        let g = scene();
        let mut w = World::default();
        w.tracker
            .observe(&ViewState::capture(&w.selection, &w.filter));
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/A"] })),
        );
        // The tracker looks at the view next frame: nothing the user did.
        assert!(!w
            .tracker
            .observe(&ViewState::capture(&w.selection, &w.filter)));
        assert_eq!(w.tracker.epoch, 0);
        // A click is.
        w.user_clicks("<http://e/b1>");
        assert_eq!(w.tracker.epoch, 1);
        assert!(w.tracker.overrode);
        // The next agent command resets `overrode`; the epoch stays.
        let h = w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/a1"], "select": "http://e/a1" }),
            ),
        );
        assert_eq!(
            body(&h)["user"]["overrode_last_agent_command"],
            true,
            "reported as of before this request"
        );
        assert!(!w.tracker.overrode);
        assert_eq!(w.tracker.epoch, 1);
    }

    #[test]
    fn the_user_block_is_computed_before_the_request_is_applied() {
        let g = scene();
        let mut w = World::default();
        w.tracker.note_input(Instant::now());
        let h = w.handle(&g, &request("status", Value::Null));
        assert_eq!(body(&h)["user"]["active"], true);
        assert_eq!(body(&h)["user"]["epoch"], 0);
        let later = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            w.tracker.user_block(later)["active"],
            false,
            "idle after 3 s"
        );
    }

    // ── filter ──

    #[test]
    fn filter_hides_and_shows_classes_like_the_panel_and_lists_what_it_could_not_toggle() {
        let g = scene();
        let mut w = World::default();
        let h = w.handle(&g, &request("filter", json!({
            "hide_classes": ["http://e/A", "http://e/knows", "http://e/ghost"], "show_untyped": false })));
        let r = &body(&h)["result"];
        assert_eq!(r["hidden_classes"], json!(["http://e/A"]));
        assert_eq!(r["show_untyped"], false);
        assert_eq!(r["unknown"], json!(["http://e/knows", "http://e/ghost"]));
        assert!(w.filter.hidden.contains("<http://e/A>") && !w.filter.show_untyped);

        let h = w.handle(
            &g,
            &request("filter", json!({ "show_classes": ["http://e/A"] })),
        );
        assert_eq!(body(&h)["result"]["hidden_count"], 0);
        assert!(w.filter.hidden.is_empty());

        let h = w.handle(
            &g,
            &request(
                "filter",
                json!({ "hide_classes": ["http://e/B"], "reset": true }),
            ),
        );
        assert_eq!(
            body(&h)["result"]["hidden_classes"],
            json!(["http://e/B"]),
            "reset first, then hide"
        );
        assert!(w.filter.show_untyped);
    }

    // ── restore ──

    #[test]
    fn clear_with_restore_gives_the_users_view_back_and_without_it_keeps_it() {
        let g = scene();
        let mut w = World::default();
        w.selection.0 = Some("<http://e/a2>".into());
        w.tracker
            .observe(&ViewState::capture(&w.selection, &w.filter));
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/B"] })),
        );
        w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/a1"], "select": "http://e/a1" }),
            ),
        );
        assert!(w.highlight.is_active());

        // Without restore: the highlight goes, the view stays as the agent left it.
        let h = w.handle(&g, &request("clear", json!({})));
        assert_eq!(
            body(&h)["result"],
            json!({ "cleared": true, "restored": false, "camera_restored": false })
        );
        assert!(!w.highlight.is_active());
        assert_eq!(w.selection.0.as_deref(), Some("<http://e/a1>"));
        assert!(w.filter.hidden.contains("<http://e/B>"));
        assert!(
            w.checkpoint.saved.is_some(),
            "the restore point is still there"
        );

        // With restore: back to what the user had.
        let h = w.handle(&g, &request("clear", json!({ "restore": true })));
        assert_eq!(body(&h)["result"]["restored"], true);
        assert_eq!(w.selection.0.as_deref(), Some("<http://e/a2>"));
        assert!(w.filter.hidden.is_empty());
        assert!(w.checkpoint.saved.is_none(), "the checkpoint is consumed");
        // A second restore has nothing to give back, and says so.
        let h = w.handle(&g, &request("clear", json!({ "restore": true })));
        assert_eq!(body(&h)["result"]["restored"], false);
        assert!(body(&h)["reason"]
            .as_str()
            .unwrap()
            .starts_with("no_restore_point"));
    }

    #[test]
    fn the_restore_point_is_the_users_view_before_the_agents_first_change_not_a_later_one() {
        let g = scene();
        let mut w = World::default();
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/A"] })),
        );
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/B"] })),
        );
        let saved = w.checkpoint.saved.clone().unwrap();
        assert!(
            saved.hidden.is_empty(),
            "two agent changes in a row keep the first checkpoint"
        );
    }

    #[test]
    fn after_the_user_moves_the_next_agent_change_saves_a_new_restore_point() {
        let g = scene();
        let mut w = World::default();
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/A"] })),
        );
        w.user_clicks("<http://e/b1>");
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/B"] })),
        );
        let saved = w.checkpoint.saved.clone().unwrap();
        assert_eq!(saved.selection.as_deref(), Some("<http://e/b1>"));
        assert!(
            saved.hidden.contains("<http://e/A>"),
            "what the user was looking at when the agent acted again"
        );
    }

    // ── Esc ──

    #[test]
    fn each_escape_does_one_thing_in_order() {
        let mut selection = Selection(Some("<http://e/a1>".into()));
        let mut checkpoint = UserViewCheckpoint {
            saved: Some(ViewState::default()),
            camera: None,
        };
        let mut highlight = AgentHighlight {
            terms: ["<http://e/b1>".to_string()].into(),
            mode: HighlightMode::Highlight,
        };
        let mut filter = ClassFilter::default();

        assert_eq!(
            escape_step(&selection, &checkpoint, &highlight),
            EscapeStep::ClearSelection
        );
        selection.0 = None;
        assert_eq!(
            escape_step(&selection, &checkpoint, &highlight),
            EscapeStep::Restore
        );
        restore_view(&mut selection, &mut filter, &mut highlight, &mut checkpoint);
        assert!(
            !highlight.is_active(),
            "a restore also clears the highlight"
        );
        assert_eq!(
            escape_step(&selection, &checkpoint, &highlight),
            EscapeStep::Nothing
        );

        highlight.terms.insert("<http://e/b1>".into());
        assert_eq!(
            escape_step(&selection, &checkpoint, &highlight),
            EscapeStep::ClearHighlight
        );
    }

    // ── status with a window ──

    #[test]
    fn status_with_a_window_adds_selection_filter_highlight_and_the_restore_point() {
        let g = scene();
        let mut w = World::default();
        w.handle(
            &g,
            &request("filter", json!({ "hide_classes": ["http://e/A"] })),
        );
        w.handle(&g, &request("show", json!({ "iris": ["http://e/b1"], "mode": "isolate", "select": "http://e/b1", "focus": false })));
        let h = w.handle(&g, &request("status", Value::Null));
        let r = &body(&h)["result"];
        assert_eq!(r["window"], true);
        assert_eq!(r["selection"], "http://e/b1");
        assert_eq!(r["filter"]["hidden_classes"], json!(["http://e/A"]));
        assert_eq!(
            r["highlight"],
            json!({ "active": true, "mode": "isolate", "count": 1 })
        );
        assert_eq!(r["has_restore_point"], true);
    }

    #[test]
    fn an_op_of_a_later_phase_is_refused_with_its_name() {
        let g = scene();
        let mut w = World::default();
        let h = w.handle(&g, &request("search_view", json!({})));
        assert_eq!(body(&h)["error"]["code"], "unknown_op");
        assert!(body(&h)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("search_view"));
    }

    // ── the camera ──

    fn move_of(h: &Handled) -> &CameraMove {
        h.camera_move.as_ref().expect("a camera move was expected")
    }

    fn look_at_of(h: &Handled) -> Vec3 {
        let v = &body(h)["result"]["camera"]["look_at"];
        Vec3::new(
            v[0].as_f64().unwrap() as f32,
            v[1].as_f64().unwrap() as f32,
            v[2].as_f64().unwrap() as f32,
        )
    }

    #[test]
    fn frame_all_fits_every_drawn_node_and_keeps_the_viewing_direction() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let h = w.handle(
            &g,
            &request(
                "camera",
                json!({ "action": "frame_all", "duration_ms": 300 }),
            ),
        );
        assert_eq!(body(&h)["ok"], true);
        let mv = move_of(&h);
        assert_eq!(mv.duration_ms, 300);
        // The centre of every node (they sit 10 apart on x).
        let n = g.nodes.iter().filter(|n| n.is_some()).count() as f32;
        let centre = w
            .cam
            .as_ref()
            .unwrap()
            .positions
            .iter()
            .copied()
            .sum::<Vec3>()
            / n;
        assert!(
            (look_at_of(&h) - centre).length() < 1e-3,
            "{:?} vs {centre:?}",
            look_at_of(&h)
        );
        // Still looking along -z: the camera stays on the +z side of what it looks at.
        assert!(mv.end.translation.z > centre.z);
        assert_eq!(body(&h)["result"]["framed"], g.index.len());
        assert!(h.feed.is_none(), "a camera move is not in the feed");
    }

    #[test]
    fn frame_centres_on_the_named_nodes_only() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let h = w.handle(
            &g,
            &request(
                "camera",
                json!({
            "action": "frame", "iris": ["http://e/a1", "http://e/b1", "http://e/ghost"] }),
            ),
        );
        let expected = (w.position_of(&g, "http://e/a1") + w.position_of(&g, "http://e/b1")) / 2.0;
        assert!((look_at_of(&h) - expected).length() < 1e-3);
        assert_eq!(body(&h)["result"]["framed"], 2);
        assert!(move_of(&h).look_at.is_some());
    }

    #[test]
    fn frame_of_nodes_that_are_not_drawn_does_not_move() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let h = w.handle(
            &g,
            &request(
                "camera",
                json!({ "action": "frame", "iris": ["http://e/ghost"] }),
            ),
        );
        assert_eq!(body(&h)["error"]["code"], "not_drawn");
        assert!(h.camera_move.is_none() && w.checkpoint.camera.is_none());
    }

    #[test]
    fn zoom_scales_the_distance_to_what_the_camera_looks_at() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let before = w.cam.as_ref().unwrap().transform.translation;
        let h = w.handle(
            &g,
            &request("camera", json!({ "action": "zoom", "factor": 2.0 })),
        );
        let after = move_of(&h).end.translation;
        // The target is on the line of sight, so the camera moved straight along it.
        let target = look_at_of(&h);
        assert!(((after - target).length() * 2.0 - (before - target).length()).abs() < 1e-2);
        let dir_before = (before - target).normalize();
        let dir_after = (after - target).normalize();
        assert!((dir_before - dir_after).length() < 1e-4);
        let out = w.handle(
            &g,
            &request("camera", json!({ "action": "zoom", "factor": 0.5 })),
        );
        assert!(
            move_of(&out).end.translation.distance(look_at_of(&out))
                > before.distance(look_at_of(&out))
        );
    }

    #[test]
    fn the_user_having_the_camera_refuses_every_camera_command_and_changes_nothing() {
        let g = scene();
        let mut w = World::with_camera(&g);
        w.cam.as_mut().unwrap().user_controlled = true;
        for args in [
            json!({ "action": "frame_all" }),
            json!({ "action": "frame", "iris": ["http://e/a1"] }),
            json!({ "action": "zoom", "factor": 2 }),
        ] {
            let h = w.handle(&g, &request("camera", args));
            assert_eq!(body(&h)["error"]["code"], "user_controlling_camera");
            assert!(h.camera_move.is_none());
        }
        assert!(w.checkpoint.camera.is_none() && w.checkpoint.saved.is_none());
    }

    #[test]
    fn a_camera_command_for_an_old_epoch_is_refused() {
        let g = scene();
        let mut w = World::with_camera(&g);
        w.user_clicks("<http://e/b1>");
        let h = w.handle(
            &g,
            &request(
                "camera",
                json!({ "action": "frame_all", "if_user_epoch": 0 }),
            ),
        );
        assert_eq!(body(&h)["error"]["code"], "user_changed");
        assert!(h.camera_move.is_none());
    }

    #[test]
    fn without_a_camera_the_camera_op_says_so() {
        let g = scene();
        let mut w = World::default();
        let h = w.handle(&g, &request("camera", json!({ "action": "frame_all" })));
        assert_eq!(body(&h)["error"]["code"], "unsupported_headless");
    }

    #[test]
    fn show_with_focus_frames_what_it_shows_and_remembers_where_the_camera_was() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let from = w.cam.as_ref().unwrap().transform;
        let h = w.handle(
            &g,
            &request("show", json!({ "iris": ["http://e/a1", "http://e/b1"] })),
        );
        assert_eq!(body(&h)["result"]["camera_moved"], true);
        assert!(body(&h).get("reason").is_none());
        let expected = (w.position_of(&g, "http://e/a1") + w.position_of(&g, "http://e/b1")) / 2.0;
        assert!((look_at_of(&h) - expected).length() < 1e-3);
        assert_eq!(move_of(&h).duration_ms, DEFAULT_MOVE_MS);
        assert_eq!(
            w.checkpoint.camera,
            Some(from),
            "the user's pose is the restore point"
        );
        assert!(w.highlight.is_active());
    }

    #[test]
    fn show_with_focus_while_the_user_has_the_camera_highlights_but_does_not_move() {
        let g = scene();
        let mut w = World::with_camera(&g);
        w.cam.as_mut().unwrap().user_controlled = true;
        let h = w.handle(&g, &request("show", json!({ "iris": ["http://e/a1"] })));
        assert_eq!(body(&h)["ok"], true);
        assert_eq!(body(&h)["result"]["camera_moved"], false);
        assert!(body(&h)["reason"]
            .as_str()
            .unwrap()
            .starts_with("user_controlling_camera"));
        assert!(
            w.highlight.is_active(),
            "the part that is not the camera still applies"
        );
        assert!(h.camera_move.is_none() && w.checkpoint.camera.is_none());
    }

    #[test]
    fn show_without_focus_leaves_the_camera_alone() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let h = w.handle(
            &g,
            &request("show", json!({ "iris": ["http://e/a1"], "focus": false })),
        );
        assert!(h.camera_move.is_none() && w.checkpoint.camera.is_none());
        assert_eq!(body(&h)["result"]["camera_moved"], false);
    }

    #[test]
    fn restoring_the_view_moves_the_camera_back_unless_the_user_has_it() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let from = w.cam.as_ref().unwrap().transform;
        w.handle(
            &g,
            &request(
                "camera",
                json!({ "action": "frame", "iris": ["http://e/b1"] }),
            ),
        );
        let h = w.handle(&g, &request("clear", json!({ "restore": true })));
        assert_eq!(body(&h)["result"]["restored"], true);
        assert_eq!(body(&h)["result"]["camera_restored"], true);
        assert_eq!(move_of(&h).end, from);
        assert!(
            w.checkpoint.camera.is_none() && w.checkpoint.saved.is_none(),
            "consumed"
        );

        // The user has the camera: the view comes back, the camera stays put.
        let mut w = World::with_camera(&g);
        w.handle(
            &g,
            &request(
                "show",
                json!({ "iris": ["http://e/b1"], "select": "http://e/b1" }),
            ),
        );
        w.cam.as_mut().unwrap().user_controlled = true;
        let h = w.handle(&g, &request("clear", json!({ "restore": true })));
        assert_eq!(body(&h)["result"]["restored"], true);
        assert_eq!(body(&h)["result"]["camera_restored"], false);
        assert!(h.camera_move.is_none());
    }

    #[test]
    fn two_agent_moves_keep_the_first_pose_but_a_user_move_in_between_saves_a_new_one() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let first = w.cam.as_ref().unwrap().transform;
        w.handle(
            &g,
            &request(
                "camera",
                json!({ "action": "frame", "iris": ["http://e/a1"] }),
            ),
        );
        // The camera is now elsewhere, put there by the agent.
        w.cam.as_mut().unwrap().transform =
            Transform::from_xyz(5.0, 5.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y);
        w.handle(
            &g,
            &request(
                "camera",
                json!({ "action": "frame", "iris": ["http://e/b1"] }),
            ),
        );
        assert_eq!(
            w.checkpoint.camera,
            Some(first),
            "agent after agent: the user's own pose stays"
        );

        // The user takes the camera somewhere; the next agent move starts from there.
        let theirs = Transform::from_xyz(-9.0, 2.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y);
        w.cam.as_mut().unwrap().transform = theirs;
        w.tracker.user_camera_moved();
        w.handle(&g, &request("camera", json!({ "action": "frame_all" })));
        assert_eq!(w.checkpoint.camera, Some(theirs));
    }

    #[test]
    fn a_user_camera_move_counts_in_the_epoch_and_the_agent_hears_of_it() {
        let mut t = UserActivityTracker::default();
        t.user_camera_moved();
        assert_eq!((t.epoch, t.overrode), (1, true));
    }

    #[test]
    fn status_reports_the_camera_and_a_camera_only_restore_point() {
        let g = scene();
        let mut w = World::with_camera(&g);
        let h = w.handle(&g, &request("status", Value::Null));
        let cam = &body(&h)["result"]["camera"];
        assert!(
            (cam["distance"].as_f64().unwrap() - 60.0).abs() < 1.0,
            "{cam}"
        );
        assert_eq!(cam["user_controlling"], false);
        assert_eq!(body(&h)["result"]["has_restore_point"], false);
        w.handle(&g, &request("camera", json!({ "action": "frame_all" })));
        let h = w.handle(&g, &request("status", Value::Null));
        assert_eq!(body(&h)["result"]["has_restore_point"], true);
    }
}

/// The real systems, run in a minimal Bevy app (no window, no renderer): the
/// order of the sets, change detection across frames, the answer reaching the
/// outbox, the feed, and `Esc`.
#[cfg(test)]
mod system_tests {
    use super::*;
    use sparql_mcp::application::event_sink::Snapshot;
    use tokio::sync::mpsc::UnboundedReceiver;

    struct Harness {
        app: App,
        responses: UnboundedReceiver<AgentResponse>,
    }

    fn harness() -> Harness {
        const T: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let snapshot = Snapshot {
            graph: "urn:project:default".into(),
            triples: vec![
                ["<http://e/a1>".into(), T.into(), "<http://e/A>".into()],
                ["<http://e/b1>".into(), T.into(), "<http://e/B>".into()],
            ],
            kinds: Default::default(),
        };
        let (tx, responses) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .configure_sets(
                Update,
                (CoreSet::Net, CoreSet::Graph, CoreSet::Sync, CoreSet::Ui).chain(),
            )
            .insert_resource(FrameMessages::default())
            .insert_resource(ConnStatus(ConnState::Streaming))
            .insert_resource(ViewerGraph::from_snapshot(&snapshot, None).0)
            .insert_resource(AgentOutbox {
                responses: tx,
                instance: "test".into(),
            })
            .init_resource::<Selection>()
            .init_resource::<ClassFilter>()
            .init_resource::<ActivityFeed>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_plugins(AgentViewPlugin)
            .add_systems(Update, escape_chain.in_set(CoreSet::Ui));
        Harness { app, responses }
    }

    impl Harness {
        /// One frame in which `requests` arrive.
        fn frame(&mut self, requests: Vec<(u64, &str, Value)>) -> Vec<Value> {
            self.app.world_mut().resource_mut::<FrameMessages>().0 = requests
                .into_iter()
                .map(|(id, op, args)| {
                    ViewerMsg::Request(AgentRequest {
                        id,
                        op: op.into(),
                        args,
                    })
                })
                .collect();
            self.app.update();
            self.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            let mut out = Vec::new();
            while let Ok(r) = self.responses.try_recv() {
                out.push(r.0);
            }
            out
        }

        fn world(&self) -> &World {
            self.app.world()
        }
    }

    #[test]
    fn a_request_is_answered_in_its_frame_and_applied_to_the_real_resources() {
        let mut h = harness();
        let answers = h.frame(vec![(
            1,
            "show",
            json!({ "iris": ["http://e/a1"], "select": "http://e/a1", "mode": "isolate", "focus": false }),
        )]);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0]["id"], 1);
        assert_eq!(answers[0]["ok"], true);
        assert_eq!(answers[0]["result"]["shown"], 1);
        assert_eq!(
            h.world().resource::<Selection>().0.as_deref(),
            Some("<http://e/a1>")
        );
        let hl = h.world().resource::<AgentHighlight>();
        assert!(hl.contains("<http://e/a1>") && hl.isolates());
        let feed = h.world().resource::<ActivityFeed>();
        assert_eq!(
            feed.0.back().unwrap().kind,
            FeedKind::Agent(AgentFeedOp::Isolate)
        );
        assert!(h.world().resource::<UserViewCheckpoint>().saved.is_some());
    }

    #[test]
    fn two_requests_in_one_frame_see_each_others_effects_in_order() {
        let mut h = harness();
        let answers = h.frame(vec![
            (1, "filter", json!({ "hide_classes": ["http://e/A"] })),
            (2, "status", Value::Null),
        ]);
        assert_eq!(
            (answers[0]["id"].clone(), answers[1]["id"].clone()),
            (json!(1), json!(2))
        );
        assert_eq!(
            answers[1]["result"]["filter"]["hidden_classes"],
            json!(["http://e/A"])
        );
        assert_eq!(answers[1]["result"]["window"], true);
    }

    #[test]
    fn the_agents_own_writes_do_not_move_the_epoch_but_the_users_do() {
        let mut h = harness();
        h.frame(vec![]); // the tracker records the starting view
        h.frame(vec![(
            1,
            "show",
            json!({ "iris": ["http://e/a1"], "select": "http://e/a1", "focus": false }),
        )]);
        h.frame(vec![]); // a frame later the tracker has looked at the agent's change
        let answers = h.frame(vec![(2, "status", Value::Null)]);
        assert_eq!(
            answers[0]["user"]["epoch"], 0,
            "the agent's selection is not the user's"
        );

        // The user clicks another node.
        h.app.world_mut().resource_mut::<Selection>().0 = Some("<http://e/b1>".into());
        let answers = h.frame(vec![(3, "status", Value::Null)]);
        assert_eq!(
            answers[0]["user"]["epoch"], 1,
            "seen before this request is answered"
        );
        assert_eq!(answers[0]["user"]["overrode_last_agent_command"], true);

        // A command written for epoch 0 is now stale.
        let answers = h.frame(vec![(
            4,
            "show",
            json!({ "iris": ["http://e/a1"], "if_user_epoch": 0 }),
        )]);
        assert_eq!(answers[0]["error"]["code"], "user_changed");
        assert_eq!(
            h.world().resource::<Selection>().0.as_deref(),
            Some("<http://e/b1>"),
            "the user's choice stands"
        );
    }

    #[test]
    fn user_input_makes_the_user_active_for_a_few_seconds() {
        let mut h = harness();
        h.frame(vec![]);
        let answers = h.frame(vec![(1, "status", Value::Null)]);
        assert_eq!(answers[0]["user"]["active"], false);
        h.app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyF);
        h.app.update();
        h.app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        let answers = h.frame(vec![(2, "status", Value::Null)]);
        assert_eq!(answers[0]["user"]["active"], true);
    }

    fn press_escape(h: &mut Harness) {
        h.app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        h.frame(vec![]);
        // A key stays pressed until released; the next press must be a new one.
        let mut keys = h.app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Escape);
        keys.clear();
    }

    #[test]
    fn escape_clears_the_selection_then_restores_the_view_then_clears_the_highlight() {
        let mut h = harness();
        h.frame(vec![]);
        h.frame(vec![
            (1, "filter", json!({ "hide_classes": ["http://e/B"] })),
            (
                2,
                "show",
                json!({ "iris": ["http://e/a1"], "select": "http://e/a1", "focus": false }),
            ),
        ]);
        assert!(h.world().resource::<Selection>().0.is_some());

        press_escape(&mut h); // 1: the selection
        assert!(h.world().resource::<Selection>().0.is_none());
        assert!(
            h.world().resource::<AgentHighlight>().is_active(),
            "one effect per press"
        );
        assert!(!h.world().resource::<ClassFilter>().hidden.is_empty());

        press_escape(&mut h); // 2: the user's view comes back
        assert!(h.world().resource::<ClassFilter>().hidden.is_empty());
        assert!(!h.world().resource::<AgentHighlight>().is_active());
        assert!(h.world().resource::<UserViewCheckpoint>().saved.is_none());

        // And a highlight with no checkpoint goes on the third kind of press.
        h.frame(vec![(
            3,
            "show",
            json!({ "iris": ["http://e/b1"], "focus": false }),
        )]);
        assert!(h.world().resource::<AgentHighlight>().is_active());
        press_escape(&mut h);
        assert!(!h.world().resource::<AgentHighlight>().is_active());
    }

    #[test]
    fn the_user_pressing_escape_to_restore_counts_as_the_users_own_change() {
        let mut h = harness();
        h.frame(vec![]);
        h.frame(vec![(
            1,
            "filter",
            json!({ "hide_classes": ["http://e/A"] }),
        )]);
        h.frame(vec![]);
        press_escape(&mut h); // restore
        let answers = h.frame(vec![(2, "status", Value::Null)]);
        assert_eq!(answers[0]["user"]["epoch"], 1);
    }

    #[test]
    fn with_the_window_marker_a_request_is_answered_once_not_twice() {
        // `answer_core_requests` stands down when `WindowedAgent` is present.
        let mut h = harness();
        h.app.add_plugins(AgentPlugin); // the headless answerer, as `CorePlugin` adds it
        let answers = h.frame(vec![(1, "status", Value::Null)]);
        assert_eq!(answers.len(), 1, "answered once: {answers:?}");
    }
    // ── the camera, through the real systems ──

    use crate::camera::{apply_camera_moves, run_camera_tween, CameraTween};
    use crate::layout::LayoutState;
    use bevy::camera_controller::pan_orbit_camera::controller::inputs::MotionInputs;
    use bevy::camera_controller::pan_orbit_camera::controller::motion::CurrentMotion;

    /// The harness with a scene camera and node positions (10 apart on x), and a
    /// clock that advances 200 ms per frame.
    fn camera_harness() -> Harness {
        let mut h = harness();
        h.app
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(200),
            ))
            .add_systems(
                Update,
                (apply_camera_moves, run_camera_tween)
                    .chain()
                    .after(CoreSet::Ui),
            );
        let nodes = h.world().resource::<ViewerGraph>().nodes.len();
        let mut layout = LayoutState::new(1);
        layout.pos = (0..nodes)
            .map(|i| Vec3::new(10.0 * i as f32, 0.0, 0.0))
            .collect();
        h.app.insert_resource(layout);
        h.app.world_mut().spawn((
            SceneCamera,
            PanOrbitCamera::default(),
            Projection::from(PerspectiveProjection::default()),
            Transform::from_xyz(0.0, 0.0, 60.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        h
    }

    fn camera_transform(h: &mut Harness) -> Transform {
        let mut q = h
            .app
            .world_mut()
            .query_filtered::<&Transform, With<SceneCamera>>();
        *q.single(h.app.world()).unwrap()
    }

    fn tweening(h: &mut Harness) -> bool {
        let mut q = h.app.world_mut().query::<&CameraTween>();
        q.iter(h.app.world()).next().is_some()
    }

    fn node_position(h: &Harness, iri: &str) -> Vec3 {
        let id = h.world().resource::<ViewerGraph>().index[&format!("<{iri}>")];
        h.world().resource::<LayoutState>().pos[id.0 as usize]
    }

    fn run_until_still(h: &mut Harness) {
        for _ in 0..20 {
            h.frame(vec![]);
            if !tweening(h) {
                return;
            }
        }
        panic!("the camera never stopped moving");
    }

    #[test]
    fn show_with_focus_flies_the_camera_to_the_nodes_over_several_frames() {
        let mut h = camera_harness();
        let from = camera_transform(&mut h);
        let answers = h.frame(vec![(1, "show", json!({ "iris": ["http://e/b1"] }))]);
        assert_eq!(answers[0]["result"]["camera_moved"], true);
        h.frame(vec![]);
        assert!(tweening(&mut h), "the camera is on its way, not there");
        assert_ne!(camera_transform(&mut h).translation, from.translation);
        run_until_still(&mut h);

        let at = camera_transform(&mut h);
        let node = node_position(&h, "http://e/b1");
        let to_node = (node - at.translation).normalize();
        assert!(
            to_node.dot(at.forward().as_vec3()) > 0.99,
            "the node is dead ahead"
        );
        assert!(
            at.translation.distance(node) < 10.0,
            "and close: {}",
            at.translation.distance(node)
        );
    }

    #[test]
    fn the_user_taking_the_camera_halfway_stops_the_move_and_counts_as_a_change() {
        let mut h = camera_harness();
        h.frame(vec![]); // the tracker records the start
        h.frame(vec![(
            1,
            "camera",
            json!({ "action": "frame", "iris": ["http://e/b1"], "duration_ms": 2000 }),
        )]);
        h.frame(vec![]);
        assert!(tweening(&mut h));
        let mid = camera_transform(&mut h);

        {
            let mut q = h.app.world_mut().query::<&mut PanOrbitCamera>();
            q.single_mut(h.app.world_mut()).unwrap().current_motion =
                CurrentMotion::UserControlled {
                    anchor: bevy::math::DVec3::new(0.0, 0.0, -10.0),
                    motion_inputs: MotionInputs::Zoom {
                        zoom_inputs: Default::default(),
                    },
                };
        }
        h.frame(vec![]);
        assert!(!tweening(&mut h), "the move is dropped");
        assert_eq!(
            camera_transform(&mut h).translation,
            mid.translation,
            "nothing is written over the user"
        );

        // The agent is told, and is refused while the user holds the camera.
        let answers = h.frame(vec![
            (2, "status", Value::Null),
            (3, "camera", json!({ "action": "frame_all" })),
        ]);
        assert_eq!(answers[0]["user"]["epoch"], 1);
        assert_eq!(answers[0]["user"]["overrode_last_agent_command"], true);
        assert_eq!(answers[0]["result"]["camera"]["user_controlling"], true);
        assert_eq!(answers[1]["error"]["code"], "user_controlling_camera");
    }

    #[test]
    fn a_motion_that_goes_on_for_many_frames_is_one_change_not_one_per_frame() {
        let mut h = camera_harness();
        h.frame(vec![]);
        {
            let mut q = h.app.world_mut().query::<&mut PanOrbitCamera>();
            q.single_mut(h.app.world_mut()).unwrap().current_motion =
                CurrentMotion::UserControlled {
                    anchor: bevy::math::DVec3::new(0.0, 0.0, -10.0),
                    motion_inputs: MotionInputs::Zoom {
                        zoom_inputs: Default::default(),
                    },
                };
        }
        for _ in 0..30 {
            h.frame(vec![]);
        }
        let answers = h.frame(vec![(1, "status", Value::Null)]);
        assert_eq!(answers[0]["user"]["epoch"], 1);
    }

    #[test]
    fn escape_flies_the_camera_back_to_where_the_user_had_it() {
        let mut h = camera_harness();
        let from = camera_transform(&mut h);
        h.frame(vec![]);
        h.frame(vec![(
            1,
            "show",
            json!({ "iris": ["http://e/b1"], "select": "http://e/b1" }),
        )]);
        run_until_still(&mut h);
        assert_ne!(camera_transform(&mut h).translation, from.translation);

        press_escape(&mut h); // the selection
        press_escape(&mut h); // the user's view, camera included
        run_until_still(&mut h);
        let back = camera_transform(&mut h);
        assert!(
            back.translation.distance(from.translation) < 0.05,
            "{:?} vs {:?}",
            back.translation,
            from.translation
        );
        assert!(back.forward().as_vec3().dot(from.forward().as_vec3()) > 0.999);
    }

    #[test]
    fn clear_with_restore_flies_the_camera_back_too() {
        let mut h = camera_harness();
        let from = camera_transform(&mut h);
        h.frame(vec![(
            1,
            "camera",
            json!({ "action": "zoom", "factor": 4 }),
        )]);
        run_until_still(&mut h);
        assert!(
            camera_transform(&mut h)
                .translation
                .distance(from.translation)
                > 5.0
        );
        let answers = h.frame(vec![(2, "clear", json!({ "restore": true }))]);
        assert_eq!(answers[0]["result"]["camera_restored"], true);
        run_until_still(&mut h);
        assert!(
            camera_transform(&mut h)
                .translation
                .distance(from.translation)
                < 0.05
        );
    }
}
