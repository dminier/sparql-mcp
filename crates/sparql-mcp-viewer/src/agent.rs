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

use crate::connection::{AgentRequest, AgentResponse, ConnState, ViewerMsg};
use crate::graph::ViewerGraph;
use crate::ingest::{ActivityFeed, AgentFeedOp, FeedKind};
use crate::interact::{filter_entries, ClassFilter, Selection};
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
}

impl UserViewCheckpoint {
    /// Save the view before an agent command changes it, unless the agent has
    /// been the only one changing it since the last save.
    pub fn save_before_agent_change(&mut self, tracker: &UserActivityTracker, view: ViewState) {
        if self.saved.is_none() || tracker.user_moved_since_agent() {
            self.saved = Some(view);
        }
    }
}

/// Give the user's view back: drop the highlight, and return the selection and
/// the filter to what the checkpoint holds. Consumes the checkpoint. Returns
/// whether there was one.
pub fn restore_view(
    selection: &mut Selection,
    filter: &mut ClassFilter,
    highlight: &mut AgentHighlight,
    checkpoint: &mut UserViewCheckpoint,
) -> bool {
    highlight.terms.clear();
    match checkpoint.saved.take() {
        Some(view) => {
            view.apply(selection, filter);
            true
        }
        None => false,
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
    pub now: Instant,
}

/// The answer, and what to show in the activity feed.
pub struct Handled {
    pub response: AgentResponse,
    pub feed: Option<(AgentFeedOp, u64)>,
}

impl Handled {
    fn answer(response: AgentResponse) -> Self {
        Self {
            response,
            feed: None,
        }
    }

    fn with_feed(response: AgentResponse, op: AgentFeedOp, count: u64) -> Self {
        Self {
            response,
            feed: Some((op, count)),
        }
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
            result["has_restore_point"] = json!(ctx.checkpoint.saved.is_some());
            Handled::answer(AgentResponse::ok(request.id, result, user))
        }
        "show" => show(request, ctx, user),
        "clear" => clear(request, ctx, user),
        "filter" => filter(request, ctx, user),
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

    // Everything is checked: now change things.
    if select.is_some() {
        let before = ViewState::capture(ctx.selection, ctx.filter);
        ctx.checkpoint.save_before_agent_change(ctx.tracker, before);
    }
    ctx.highlight.terms = found.iter().cloned().collect();
    ctx.highlight.mode = mode;
    if let Some(term) = &select {
        ctx.selection.0 = Some(term.clone());
        ctx.tracker
            .agent_wrote(&ViewState::capture(ctx.selection, ctx.filter));
    }

    let mut result = json!({
        "shown": found.len(),
        "missing": listed(&missing),
        "missing_count": missing.len(),
        "mode": mode.name(),
        "camera_moved": false,
    });
    if let Some(term) = &select {
        result["selected"] = json!(iri_of(term));
    }
    let response = if focus {
        AgentResponse::ok_with_reason(
            request.id,
            result,
            user,
            "camera_not_moved: this build has no camera control yet",
        )
    } else {
        AgentResponse::ok(request.id, result, user)
    };
    let op = match mode {
        HighlightMode::Highlight => AgentFeedOp::Highlight,
        HighlightMode::Isolate => AgentFeedOp::Isolate,
    };
    Handled::with_feed(response, op, found.len() as u64)
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
                json!({ "cleared": had_highlight, "restored": false }),
                user,
            ),
            AgentFeedOp::Clear,
            0,
        );
    }
    let restored = restore_view(ctx.selection, ctx.filter, ctx.highlight, ctx.checkpoint);
    ctx.tracker
        .agent_wrote(&ViewState::capture(ctx.selection, ctx.filter));
    let result = json!({ "cleared": had_highlight, "restored": restored });
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
    Handled::with_feed(response, AgentFeedOp::Restore, 0)
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
    ctx.checkpoint.save_before_agent_change(ctx.tracker, before);
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
) {
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
                now: Instant::now(),
            },
        );
        if let Some((op, count)) = handled.feed {
            feed.push(FeedKind::Agent(op), count);
        }
        let _ = outbox.responses.send(handled.response);
    }
}

/// Notices what the user does: input (for `user.active`), and any change to the
/// selection or the filter that the agent did not make (for `user.epoch`).
/// Runs before the agent's systems, so a request sees the user's latest move.
pub fn track_user_changes(
    selection: Res<Selection>,
    filter: Res<ClassFilter>,
    mut tracker: ResMut<UserActivityTracker>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    motion: Option<Res<bevy::input::mouse::AccumulatedMouseMotion>>,
    scroll: Option<Res<bevy::input::mouse::AccumulatedMouseScroll>>,
) {
    let input = keys.is_some_and(|k| {
        k.get_just_pressed().next().is_some() || k.get_just_released().next().is_some()
    }) || buttons.is_some_and(|b| {
        b.get_pressed().next().is_some() || b.get_just_released().next().is_some()
    }) || motion.is_some_and(|m| m.delta != Vec2::ZERO)
        || scroll.is_some_and(|s| s.delta != Vec2::ZERO);
    if input {
        tracker.note_input(Instant::now());
    }
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
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    match escape_step(&selection, &checkpoint, &highlight) {
        EscapeStep::ClearSelection => selection.0 = None,
        EscapeStep::Restore => {
            restore_view(&mut selection, &mut filter, &mut highlight, &mut checkpoint);
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
             mut checkpoint: ResMut<UserViewCheckpoint>| {
                restore_view(&mut selection, &mut filter, &mut highlight, &mut checkpoint);
            },
        );
}

fn update_restore_chip(
    highlight: Res<AgentHighlight>,
    checkpoint: Res<UserViewCheckpoint>,
    mut chip: Query<&mut Visibility, With<RestoreChip>>,
) {
    let wanted = if highlight.is_active() || checkpoint.saved.is_some() {
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
    }

    impl World {
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
    fn asking_for_a_camera_move_is_answered_honestly_for_now() {
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
            .starts_with("camera_not_moved"));
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
            json!({ "cleared": true, "restored": false })
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
        let h = w.handle(&g, &request("camera", json!({})));
        assert_eq!(body(&h)["error"]["code"], "unknown_op");
        assert!(body(&h)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("camera"));
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
}
