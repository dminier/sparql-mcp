//! Picking, selection, the class filter and labels (`bevy_ui`).
//!
//! `Selection` holds the selected node's *term*, not its `NodeId`: every
//! snapshot rebuild reassigns ids, and a selection by id would silently
//! jump to another node.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use bevy::picking::events::{PointerClick, PointerOut, PointerOver};
use bevy::picking::hover::Hovered;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;

use crate::camera::SceneCamera;
use crate::graph::{Node as GraphNode, NodeId, NodeKind, ViewerGraph};
use crate::layout::LayoutState;
use crate::plugin::CoreSet;
use crate::render::{NodeRef, CLASS_RADIUS, INSTANCE_RADIUS};

/// A press+release longer than this is a camera drag, not a click.
const CLICK_MAX: Duration = Duration::from_millis(250);
/// The selection panel lists at most this many edges per direction.
const MAX_EDGE_LINES: usize = 25;
/// The class filter panel lists at most this many classes.
const MAX_FILTER_ROWS: usize = 30;
/// How many class labels are shown at once (nearest to the camera).
const LABEL_BUDGET: usize = 30;
const LABEL_REFRESH_SECS: f32 = 0.25;

/// Label text: near-white with a hint of blue, to read on the dark plate.
const LABEL_TEXT: Color = Color::srgb(0.97, 0.98, 1.0);
/// The plate behind a label. It is nearly opaque so the text keeps its
/// contrast over the brightest part of the scene (a glowing class sphere and
/// its bloom). See the contrast test below.
const LABEL_PLATE: Color = Color::srgba(0.02, 0.02, 0.06, 0.88);
const LABEL_FONT_PX: f32 = 14.0;
/// Space between a node's edge and the bottom of its label, in pixels.
const LABEL_GAP_PX: f32 = 8.0;
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

#[derive(Resource, Default)]
pub struct Selection(pub Option<String>);

#[derive(Resource, Default)]
pub struct HoveredNode(pub Option<NodeId>);

/// Visible set = union of the selected classes. Every class is selected at
/// start (empty `hidden`); class nodes are never filtered out; untyped
/// instances follow `show_untyped`, on at start.
#[derive(Resource)]
pub struct ClassFilter {
    pub hidden: HashSet<String>,
    pub show_untyped: bool,
}

impl Default for ClassFilter {
    fn default() -> Self {
        Self {
            hidden: HashSet::new(),
            show_untyped: true,
        }
    }
}

impl ClassFilter {
    pub fn is_visible(&self, node: &GraphNode) -> bool {
        if node.kind == NodeKind::Class {
            return true;
        }
        if node.types.is_empty() {
            return self.show_untyped;
        }
        node.types.iter().any(|t| !self.hidden.contains(t))
    }
}

/// One edge as the selection panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeLine {
    pub predicate: String,
    pub other_term: String,
    pub other_label: String,
}

/// Everything the selection panel shows for one node.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionView {
    pub label: String,
    pub term: String,
    pub attributes: BTreeMap<String, Vec<String>>,
    pub outgoing: Vec<EdgeLine>,
    pub incoming: Vec<EdgeLine>,
}

pub fn short(term: &str) -> String {
    let iri = term
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(term);
    iri.rsplit(['#', '/']).next().unwrap_or(iri).to_string()
}

/// An N-Triples literal for display: `"Alice"^^<…#string>` → `Alice`,
/// `"chat"@fr` → `chat @fr`, `"3"^^<…#integer>` → `3 (integer)`.
pub fn display_literal(nt: &str) -> String {
    let Some(body) = nt.strip_prefix('"') else {
        return nt.to_string();
    };
    let Some(end) = body.rfind('"') else {
        return nt.to_string();
    };
    let value = body[..end].replace("\\\"", "\"").replace("\\\\", "\\");
    let suffix = &body[end + 1..];
    if let Some(lang) = suffix.strip_prefix('@') {
        format!("{value} @{lang}")
    } else if let Some(dt) = suffix.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
        if dt == XSD_STRING {
            value
        } else {
            format!("{value} ({})", short(dt))
        }
    } else {
        value
    }
}

pub fn selection_view(graph: &ViewerGraph, term: &str) -> Option<SelectionView> {
    let id = *graph.index.get(term)?;
    let node = graph.nodes.get(id.0 as usize)?.as_ref()?;
    let line = |predicate: &str, other: NodeId| {
        let other = graph.nodes.get(other.0 as usize)?.as_ref()?;
        Some(EdgeLine {
            predicate: short(predicate),
            other_term: other.term.clone(),
            other_label: other.label.clone(),
        })
    };
    Some(SelectionView {
        label: node.label.clone(),
        term: node.term.clone(),
        attributes: node
            .attributes
            .iter()
            .map(|(p, values)| (short(p), values.iter().cloned().collect()))
            .collect(),
        outgoing: graph
            .edges
            .iter()
            .filter(|e| e.from == id)
            .filter_map(|e| line(&e.predicate, e.to))
            .collect(),
        incoming: graph
            .edges
            .iter()
            .filter(|e| e.to == id)
            .filter_map(|e| line(&e.predicate, e.from))
            .collect(),
    })
}

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Selection>()
            .init_resource::<HoveredNode>()
            .init_resource::<ClassFilter>()
            .add_observer(on_node_click)
            .add_observer(on_node_over)
            .add_observer(on_node_out)
            .add_systems(Startup, spawn_panels)
            .add_systems(
                Update,
                (
                    clear_selection_on_escape,
                    update_selection_panel,
                    update_filter_panel,
                    update_labels,
                    position_labels,
                )
                    .chain()
                    .in_set(CoreSet::Ui),
            );
    }
}

fn on_node_click(
    click: On<PointerClick>,
    nodes: Query<&NodeRef>,
    graph: Res<ViewerGraph>,
    mut selection: ResMut<Selection>,
) {
    if click.button != PointerButton::Primary || click.duration > CLICK_MAX {
        return;
    }
    let Ok(node_ref) = nodes.get(click.entity) else {
        return;
    };
    if let Some(Some(node)) = graph.nodes.get(node_ref.0 .0 as usize) {
        selection.0 = Some(node.term.clone());
    }
}

fn on_node_over(over: On<PointerOver>, nodes: Query<&NodeRef>, mut hovered: ResMut<HoveredNode>) {
    if let Ok(node_ref) = nodes.get(over.entity) {
        hovered.0 = Some(node_ref.0);
    }
}

fn on_node_out(out: On<PointerOut>, nodes: Query<&NodeRef>, mut hovered: ResMut<HoveredNode>) {
    if let Ok(node_ref) = nodes.get(out.entity) {
        if hovered.0 == Some(node_ref.0) {
            hovered.0 = None;
        }
    }
}

fn clear_selection_on_escape(keys: Res<ButtonInput<KeyCode>>, mut selection: ResMut<Selection>) {
    if keys.just_pressed(KeyCode::Escape) && selection.0.is_some() {
        selection.0 = None;
    }
}

// ── Panels ─────────────────────────────────────────────────────────────────

#[derive(Component)]
struct SelectionPanel;

#[derive(Component)]
struct FilterPanel;

const PANEL_BG: Color = Color::srgba(0.04, 0.05, 0.09, 0.85);
const BUTTON_BG: Color = Color::srgba(0.15, 0.2, 0.32, 0.9);
const TEXT: Color = Color::srgb(0.85, 0.88, 0.95);
const DIM: Color = Color::srgb(0.55, 0.6, 0.7);

fn panel_node(right: bool) -> Node {
    Node {
        position_type: PositionType::Absolute,
        top: Val::Px(12.0),
        left: if right { Val::Auto } else { Val::Px(12.0) },
        right: if right { Val::Px(12.0) } else { Val::Auto },
        max_width: Val::Px(380.0),
        max_height: Val::Percent(70.0),
        padding: UiRect::all(Val::Px(10.0)),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(3.0),
        overflow: Overflow::clip(),
        ..default()
    }
}

fn spawn_panels(mut commands: Commands) {
    commands.spawn((
        SelectionPanel,
        panel_node(true),
        BackgroundColor(PANEL_BG),
        Hovered::default(),
        Visibility::Hidden,
    ));
    commands.spawn((
        FilterPanel,
        panel_node(false),
        BackgroundColor(PANEL_BG),
        Hovered::default(),
    ));
}

fn text(content: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(content),
        TextFont {
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(color),
    )
}

fn button_node() -> Node {
    Node {
        padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
        align_self: AlignSelf::FlexStart,
        ..default()
    }
}

fn update_selection_panel(
    mut commands: Commands,
    selection: Res<Selection>,
    graph: Res<ViewerGraph>,
    mut panel: Query<(Entity, &mut Visibility), With<SelectionPanel>>,
) {
    if !selection.is_changed() && !graph.is_changed() {
        return;
    }
    let Ok((panel, mut visibility)) = panel.single_mut() else {
        return;
    };
    commands.entity(panel).despawn_related::<Children>();
    let Some(view) = selection
        .0
        .as_deref()
        .and_then(|t| selection_view(&graph, t))
    else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Inherited;

    commands.entity(panel).with_children(|p| {
        p.spawn(text(view.label.clone(), 18.0, Color::WHITE));
        let iri = view
            .term
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string();
        p.spawn(text(iri.clone(), 12.0, DIM));
        p.spawn((button_node(), BackgroundColor(BUTTON_BG)))
            .with_child(text("copy IRI", 12.0, TEXT))
            .observe(move |_: On<PointerClick>| {
                if let Err(e) = arboard::Clipboard::new().and_then(|mut c| c.set_text(iri.clone()))
                {
                    tracing::warn!(error = %e, "copying the IRI to the clipboard failed");
                }
            });

        if !view.attributes.is_empty() {
            p.spawn(text("attributes", 13.0, DIM));
            for (predicate, values) in &view.attributes {
                let shown: Vec<String> = values.iter().map(|v| display_literal(v)).collect();
                p.spawn(text(
                    format!("  {predicate}: {}", shown.join(", ")),
                    13.0,
                    TEXT,
                ));
            }
        }
        // Bevy's default font has no arrow or box glyphs; stick to ASCII.
        for (heading, lines, arrow) in [
            ("outgoing", &view.outgoing, "->"),
            ("incoming", &view.incoming, "<-"),
        ] {
            if lines.is_empty() {
                continue;
            }
            p.spawn(text(format!("{heading} ({})", lines.len()), 13.0, DIM));
            for line in lines.iter().take(MAX_EDGE_LINES) {
                let target = line.other_term.clone();
                p.spawn((button_node(), BackgroundColor(BUTTON_BG)))
                    .with_child(text(
                        format!("{} {arrow} {}", line.predicate, line.other_label),
                        13.0,
                        TEXT,
                    ))
                    .observe(
                        move |_: On<PointerClick>, mut selection: ResMut<Selection>| {
                            selection.0 = Some(target.clone());
                        },
                    );
            }
            if lines.len() > MAX_EDGE_LINES {
                p.spawn(text(
                    format!("  ... {} more not listed", lines.len() - MAX_EDGE_LINES),
                    12.0,
                    DIM,
                ));
            }
        }
    });
}

/// One row of the class filter: a type that at least one instance has.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEntry {
    pub term: String,
    pub label: String,
    pub instances: usize,
}

/// The types worth filtering on: those that type at least one instance,
/// most-used first. Filtering only ever hides instances, so a class that
/// types none (or a property) would be a dead toggle — on a real ontology
/// like Schema.org that is the vast majority of class-kind nodes.
pub fn filter_entries(graph: &ViewerGraph) -> Vec<FilterEntry> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for node in graph.nodes.iter().flatten() {
        if node.kind == NodeKind::Instance {
            for t in &node.types {
                *counts.entry(t.as_str()).or_default() += 1;
            }
        }
    }
    let mut entries: Vec<FilterEntry> = counts
        .into_iter()
        .map(|(term, instances)| FilterEntry {
            term: term.to_string(),
            label: graph
                .index
                .get(term)
                .and_then(|id| graph.nodes.get(id.0 as usize)?.as_ref())
                .map_or_else(|| short(term), |n| n.label.clone()),
            instances,
        })
        .collect();
    entries.sort_by(|a, b| b.instances.cmp(&a.instances).then(a.label.cmp(&b.label)));
    entries
}

fn update_filter_panel(
    mut commands: Commands,
    filter: Res<ClassFilter>,
    graph: Res<ViewerGraph>,
    panel: Query<Entity, With<FilterPanel>>,
    mut last_entries: Local<Vec<FilterEntry>>,
) {
    if !filter.is_changed() && !graph.is_changed() {
        return;
    }
    let entries = filter_entries(&graph);
    if !filter.is_changed() && entries == *last_entries {
        return;
    }
    *last_entries = entries.clone();
    let Ok(panel) = panel.single() else { return };
    commands.entity(panel).despawn_related::<Children>();
    commands.entity(panel).with_children(|p| {
        p.spawn(text("instances by class (click to filter)", 13.0, DIM));
        if entries.is_empty() {
            p.spawn(text("  (no typed instances)", 12.0, DIM));
        }
        let hidden_rows = entries.len().saturating_sub(MAX_FILTER_ROWS);
        for FilterEntry {
            term,
            label,
            instances,
        } in entries.into_iter().take(MAX_FILTER_ROWS)
        {
            let label = format!("{label} ({instances})");
            let on = !filter.hidden.contains(&term);
            p.spawn((
                button_node(),
                BackgroundColor(if on {
                    BUTTON_BG
                } else {
                    Color::srgba(0.1, 0.1, 0.12, 0.6)
                }),
            ))
            .with_child(text(
                format!("{} {label}", if on { "[x]" } else { "[ ]" }),
                13.0,
                if on { TEXT } else { DIM },
            ))
            .observe(
                move |_: On<PointerClick>, mut filter: ResMut<ClassFilter>| {
                    if !filter.hidden.remove(&term) {
                        filter.hidden.insert(term.clone());
                    }
                },
            );
        }
        if hidden_rows > 0 {
            p.spawn(text(format!("  ... {hidden_rows} more classes"), 12.0, DIM));
        }
        let on = filter.show_untyped;
        p.spawn((button_node(), BackgroundColor(BUTTON_BG)))
            .with_child(text(
                format!("{} untyped", if on { "[x]" } else { "[ ]" }),
                13.0,
                if on { TEXT } else { DIM },
            ))
            .observe(|_: On<PointerClick>, mut filter: ResMut<ClassFilter>| {
                filter.show_untyped = !filter.show_untyped;
            });
    });
}

// ── Labels ─────────────────────────────────────────────────────────────────

/// A floating label for one node: the classes nearest the camera, the
/// selected node and the hovered node.
#[derive(Component)]
struct NodeLabel(NodeId);

/// Up to `k` class nodes closest to `eye`. Labelling every class does not
/// scale: a real ontology has thousands, which is both unreadable and
/// thousands of UI nodes to reposition every frame.
pub fn nearest_classes(
    graph: &ViewerGraph,
    positions: &[Vec3],
    eye: Vec3,
    k: usize,
) -> Vec<NodeId> {
    let mut classes: Vec<(f32, NodeId)> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, n)| {
            n.as_ref().is_some_and(|n| n.kind == NodeKind::Class) && *i < positions.len()
        })
        .map(|(i, _)| (positions[i].distance_squared(eye), NodeId(i as u32)))
        .collect();
    if classes.len() > k {
        classes.select_nth_unstable_by(k, |a, b| a.0.total_cmp(&b.0));
        classes.truncate(k);
    }
    classes.into_iter().map(|(_, id)| id).collect()
}

#[allow(clippy::too_many_arguments)]
fn update_labels(
    mut commands: Commands,
    graph: Res<ViewerGraph>,
    layout: Res<LayoutState>,
    selection: Res<Selection>,
    hovered: Res<HoveredNode>,
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<SceneCamera>>,
    mut labels: Query<(Entity, &NodeLabel, &mut Text)>,
    mut nearest: Local<(f32, Vec<NodeId>)>,
) {
    // Re-pick the nearest classes a few times a second, not every frame.
    nearest.0 -= time.delta_secs();
    if nearest.0 <= 0.0 || graph.is_changed() {
        nearest.0 = LABEL_REFRESH_SECS;
        nearest.1 = camera.single().map_or_else(
            |_| Vec::new(),
            |eye| nearest_classes(&graph, &layout.pos, eye.translation(), LABEL_BUDGET),
        );
    }
    let mut wanted: HashSet<NodeId> = nearest.1.iter().copied().collect();
    if let Some(id) = selection.0.as_deref().and_then(|t| graph.index.get(t)) {
        wanted.insert(*id);
    }
    if let Some(id) = hovered.0 {
        wanted.insert(id);
    }

    for (entity, label, mut text) in &mut labels {
        match graph
            .nodes
            .get(label.0 .0 as usize)
            .and_then(Option::as_ref)
        {
            Some(node) if wanted.remove(&label.0) => {
                if **text != node.label {
                    **text = node.label.clone();
                }
            }
            _ => commands.entity(entity).despawn(),
        }
    }
    for id in wanted {
        let Some(Some(node)) = graph.nodes.get(id.0 as usize) else {
            continue;
        };
        commands.spawn((
            NodeLabel(id),
            Node {
                position_type: PositionType::Absolute,
                padding: UiRect::axes(Val::Px(7.0), Val::Px(3.0)),
                border_radius: BorderRadius::all(Val::Px(5.0)),
                ..default()
            },
            // `left`/`top` hold the anchor (the middle of the label's bottom
            // edge); this moves the label so its bottom centre sits there.
            UiTransform::from_translation(Val2 {
                x: Val::Percent(-50.0),
                y: Val::Percent(-100.0),
            }),
            BackgroundColor(LABEL_PLATE),
            Text::new(node.label.clone()),
            TextFont {
                font_size: FontSize::Px(LABEL_FONT_PX),
                weight: FontWeight::SEMIBOLD,
                ..default()
            },
            TextColor(LABEL_TEXT),
            Pickable::IGNORE,
        ));
    }
}

/// Where a label's bottom-centre anchor goes: straight above the node's
/// centre on screen, clear of the node's own circle. `radius_px` is the node's
/// projected radius, so the gap holds at any distance, unlike a fixed offset in
/// world units (which shrinks to nothing, and into the glowing sphere, far away).
fn label_anchor(centre: Vec2, radius_px: f32) -> Vec2 {
    Vec2::new(centre.x, centre.y - radius_px - LABEL_GAP_PX)
}

fn position_labels(
    graph: Res<ViewerGraph>,
    layout: Res<LayoutState>,
    camera: Query<(&Camera, &GlobalTransform), With<SceneCamera>>,
    mut labels: Query<(&NodeLabel, &mut Node, &mut Visibility)>,
) {
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    for (label, mut node, mut visibility) in &mut labels {
        let world_radius = match graph
            .nodes
            .get(label.0 .0 as usize)
            .and_then(Option::as_ref)
        {
            Some(n) if n.kind == NodeKind::Class => CLASS_RADIUS,
            _ => INSTANCE_RADIUS,
        };
        let screen = layout.pos.get(label.0 .0 as usize).and_then(|p| {
            let centre = camera.world_to_viewport(camera_transform, *p).ok()?;
            // The projected radius: how far a point one radius to the side of
            // the centre lands from it on screen.
            let side = *p + camera_transform.right() * world_radius;
            let radius_px = camera
                .world_to_viewport(camera_transform, side)
                .map_or(0.0, |s| (s - centre).length());
            Some(label_anchor(centre, radius_px))
        });
        match screen {
            Some(at) => {
                node.left = Val::Px(at.x);
                node.top = Val::Px(at.y);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparql_mcp::application::event_sink::{Snapshot, StoreEvent};

    /// Relative luminance of an sRGB colour, per WCAG 2.x.
    fn luminance(c: Color) -> f32 {
        let c = c.to_srgba();
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.red) + 0.7152 * lin(c.green) + 0.0722 * lin(c.blue)
    }

    fn contrast(a: f32, b: f32) -> f32 {
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The plate is translucent, so what shows through it matters. Take the
    /// brightest thing the scene can put behind a label (pure white, a
    /// saturated bloom) and check the text still clears WCAG AAA (7:1).
    #[test]
    fn label_text_keeps_a_7_to_1_contrast_over_the_brightest_background() {
        let plate = LABEL_PLATE.to_srgba();
        let text = luminance(LABEL_TEXT);
        for behind in [
            Color::WHITE,
            Color::srgb(1.0, 0.55, 0.15),  // the class sphere's colour
            Color::srgb(0.35, 0.65, 0.95), // the instance sphere's colour
        ] {
            let b = behind.to_srgba();
            // Blend in sRGB space, as the UI pass does.
            let mix = |p: f32, b: f32| p * plate.alpha + b * (1.0 - plate.alpha);
            let seen = Color::srgb(
                mix(plate.red, b.red),
                mix(plate.green, b.green),
                mix(plate.blue, b.blue),
            );
            let ratio = contrast(text, luminance(seen));
            assert!(
                ratio >= 7.0,
                "label contrast {ratio:.1}:1 over {behind:?}, below 7:1"
            );
        }
    }

    #[test]
    fn the_anchor_is_centred_above_the_node_and_clears_its_circle() {
        let at = label_anchor(Vec2::new(300.0, 200.0), 12.0);
        assert_eq!(at.x, 300.0, "centred on the node");
        assert!(at.y <= 200.0 - 12.0, "inside the node's own circle");
        // A far node has a tiny radius, yet keeps the same gap above it.
        let far = label_anchor(Vec2::new(300.0, 200.0), 1.0);
        assert!(far.y < 200.0 && (200.0 - far.y) >= LABEL_GAP_PX);
    }

    fn t(s: &str, p: &str, o: &str) -> [String; 3] {
        [s.into(), p.into(), o.into()]
    }

    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    fn sample() -> ViewerGraph {
        let snapshot = Snapshot {
            graph: "g".into(),
            triples: vec![
                t("<urn:C>", TYPE, "<http://www.w3.org/2002/07/owl#Class>"),
                t("<urn:D>", TYPE, "<http://www.w3.org/2002/07/owl#Class>"),
                t("<urn:a>", TYPE, "<urn:C>"),
                t("<urn:a>", "<urn:ns#name>", "\"Alice\""),
                t("<urn:a>", "<urn:ns#knows>", "<urn:b>"),
                t("<urn:b>", TYPE, "<urn:D>"),
                t("<urn:u>", "<urn:ns#name>", "\"untyped\""),
            ],
            kinds: Default::default(),
        };
        ViewerGraph::from_snapshot(&snapshot, None).0
    }

    #[test]
    fn selection_view_lists_attributes_and_both_edge_directions() {
        let graph = sample();
        let view = selection_view(&graph, "<urn:a>").unwrap();
        assert_eq!(view.attributes["name"], vec!["\"Alice\"".to_string()]);
        assert!(view
            .outgoing
            .iter()
            .any(|e| e.predicate == "knows" && e.other_term == "<urn:b>"));
        assert!(view
            .outgoing
            .iter()
            .any(|e| e.predicate == "type" && e.other_term == "<urn:C>"));

        let b = selection_view(&graph, "<urn:b>").unwrap();
        assert!(b
            .incoming
            .iter()
            .any(|e| e.predicate == "knows" && e.other_term == "<urn:a>"));
        assert!(selection_view(&graph, "<urn:missing>").is_none());
    }

    #[test]
    fn class_filter_rules() {
        let graph = sample();
        let node = |term: &str| {
            graph.nodes[graph.index[term].0 as usize]
                .as_ref()
                .unwrap()
                .clone()
        };
        let mut filter = ClassFilter::default();
        assert!(
            filter.is_visible(&node("<urn:a>")),
            "every class selected at start"
        );
        assert!(
            filter.is_visible(&node("<urn:u>")),
            "untyped shown at start"
        );

        filter.hidden.insert("<urn:C>".into());
        assert!(
            !filter.is_visible(&node("<urn:a>")),
            "a's only class is hidden"
        );
        assert!(
            filter.is_visible(&node("<urn:b>")),
            "b's class D is still selected"
        );
        assert!(
            filter.is_visible(&node("<urn:C>")),
            "class nodes are never filtered"
        );

        filter.show_untyped = false;
        assert!(!filter.is_visible(&node("<urn:u>")));
    }

    #[test]
    fn filter_lists_only_types_with_instances_most_used_first() {
        let mut graph = sample();
        let extra = StoreEvent::Insert {
            graph: "g".into(),
            triples: vec![t("<urn:c2>", TYPE, "<urn:C>")],
            kinds: Default::default(),
        };
        graph.apply_event(&extra);

        let entries = filter_entries(&graph);
        let rows: Vec<(&str, usize)> = entries
            .iter()
            .map(|e| (e.term.as_str(), e.instances))
            .collect();
        // C types a and c2; D types b. owl:Class only types class nodes,
        // so it is not a filter row.
        assert_eq!(rows, vec![("<urn:C>", 2), ("<urn:D>", 1)]);
    }

    #[test]
    fn nearest_classes_picks_the_k_closest_class_nodes() {
        let graph = sample();
        let mut positions = vec![Vec3::ZERO; graph.nodes.len()];
        let c = graph.index["<urn:C>"].0 as usize;
        let d = graph.index["<urn:D>"].0 as usize;
        let a = graph.index["<urn:a>"].0 as usize;
        positions[c] = Vec3::new(100.0, 0.0, 0.0);
        positions[d] = Vec3::new(1.0, 0.0, 0.0);
        positions[a] = Vec3::new(0.5, 0.0, 0.0); // an instance: never picked

        let picked = nearest_classes(&graph, &positions, Vec3::ZERO, 1);
        assert_eq!(picked, vec![NodeId(d as u32)]);
        assert_eq!(nearest_classes(&graph, &positions, Vec3::ZERO, 10).len(), 2);
    }

    #[test]
    fn display_literal_hides_xsd_string_and_keeps_lang_and_other_types() {
        assert_eq!(
            display_literal("\"Project\"^^<http://www.w3.org/2001/XMLSchema#string>"),
            "Project"
        );
        assert_eq!(display_literal("\"chat\"@fr"), "chat @fr");
        assert_eq!(
            display_literal("\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            "3 (integer)"
        );
        assert_eq!(display_literal("\"say \\\"hi\\\"\""), "say \"hi\"");
        assert_eq!(
            display_literal("<urn:not-a-literal>"),
            "<urn:not-a-literal>"
        );
    }

    #[test]
    fn short_strips_brackets_and_namespace() {
        assert_eq!(short("<https://sparql-mcp.dev/ns#label>"), "label");
        assert_eq!(short("<urn:x/y/z>"), "z");
    }
}
