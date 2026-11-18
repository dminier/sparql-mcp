//! Hovering a relation shows its name.
//!
//! Edges are not entities: all the edges of one style are merged into a single
//! line mesh (see `render.rs`), so Bevy's picking cannot tell which one the
//! pointer is over. Instead, each frame the nodes are projected to the screen
//! and the edge whose segment passes closest to the cursor, within a few
//! pixels, is the hovered one. That costs one projection per node and one
//! point-to-segment distance per edge, well under a millisecond at the sizes
//! the viewer targets (5,000 nodes, 10,000 edges).
//!
//! The hovered edge is drawn as a bright line over the scene and named in a
//! plate next to the cursor: the predicate, then `source → target`.
//!
//! A node under the pointer wins (its own label shows), and so does a panel.
//! While a mouse button is held (orbiting, panning) nothing is shown.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::camera::SceneCamera;
use crate::graph::{Edge, NodeId, ViewerGraph};
use crate::interact::{short, ClassFilter, HoveredNode};
use crate::layout::LayoutState;
use crate::plugin::CoreSet;

/// How close, in pixels, the cursor must be to an edge to hover it.
const PICK_PX: f32 = 7.0;
/// Longest node label shown in the plate, in characters.
const CLIP_CHARS: usize = 32;

/// Same plate as the node labels, so the two read as one family.
const PLATE: Color = Color::srgba(0.02, 0.02, 0.06, 0.88);
const TEXT: Color = Color::srgb(0.97, 0.98, 1.0);
const HIGHLIGHT: Color = Color::srgba(1.0, 1.0, 1.0, 0.95);

/// The edge under the cursor, with where it is on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeHit {
    /// Index into `ViewerGraph::edges`.
    pub index: usize,
    /// The edge's two ends, projected to the screen.
    pub a: Vec2,
    pub b: Vec2,
    /// The point of the edge closest to the cursor.
    pub closest: Vec2,
}

#[derive(Resource, Default)]
pub struct HoveredEdge(pub Option<EdgeHit>);

#[derive(Component)]
struct EdgeTooltip;

#[derive(Component)]
struct EdgeHighlight;

/// Distance from `p` to the segment `a`-`b`, and the closest point on it.
pub fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> (f32, Vec2) {
    let ab = b - a;
    let len_sq = ab.length_squared();
    let t = if len_sq <= f32::EPSILON {
        0.0
    } else {
        ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
    };
    let closest = a + ab * t;
    ((p - closest).length(), closest)
}

/// The edge whose on-screen segment passes closest to `cursor`, if one is
/// within `max_px`. `screen[i]` is node `i` projected to the screen (`None`
/// for a node that does not exist or is off-camera). An edge for which
/// `visible` is false, one with an unprojected end, and a self-loop are never
/// hit.
pub fn nearest_edge(
    screen: &[Option<Vec2>],
    edges: &[Edge],
    visible: impl Fn(&Edge) -> bool,
    cursor: Vec2,
    max_px: f32,
) -> Option<EdgeHit> {
    let mut best: Option<(f32, EdgeHit)> = None;
    for (index, edge) in edges.iter().enumerate() {
        if edge.from == edge.to || !visible(edge) {
            continue;
        }
        let (Some(Some(a)), Some(Some(b))) = (
            screen.get(edge.from.0 as usize),
            screen.get(edge.to.0 as usize),
        ) else {
            continue;
        };
        let (distance, closest) = distance_to_segment(cursor, *a, *b);
        if distance <= max_px && best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((
                distance,
                EdgeHit {
                    index,
                    a: *a,
                    b: *b,
                    closest,
                },
            ));
        }
    }
    best.map(|(_, hit)| hit)
}

/// A label cut to `CLIP_CHARS` characters, with an ellipsis.
fn clip(label: &str) -> String {
    if label.chars().count() <= CLIP_CHARS {
        label.to_string()
    } else {
        let mut out: String = label.chars().take(CLIP_CHARS - 1).collect();
        out.push('…');
        out
    }
}

/// Most lines the plate shows before it says "+N more".
const MAX_LINES: usize = 6;

/// The distinct relation names from `from` to `to`, in graph order.
fn relations(graph: &ViewerGraph, from: NodeId, to: NodeId) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for e in graph.edges.iter().filter(|e| e.from == from && e.to == to) {
        let name = short(&e.predicate);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The plate's text for the hovered edge.
///
/// One-way: the relation, then `source → target`.
/// Two-way (the same two nodes linked in both directions, drawn as one arrow
/// with a head at each end): `A ↔ B`, then each direction with its own
/// relations, so both names are on the one arrow.
pub fn tooltip_text(graph: &ViewerGraph, edge: &Edge) -> String {
    let name = |id: NodeId| {
        graph
            .nodes
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .map_or_else(|| "?".to_string(), |n| clip(&n.label))
    };
    let (a, b) = (edge.from, edge.to);
    let mut forward = relations(graph, a, b);
    if forward.is_empty() {
        // The edge was just removed; show what we were given.
        forward.push(short(&edge.predicate));
    }
    let backward = relations(graph, b, a);

    if backward.is_empty() {
        return format!("{}\n{} → {}", forward.join(", "), name(a), name(b));
    }
    // One arrow, one reading: order the ends by id so hovering either of the
    // two edges shows the same text.
    let (lo, hi, down, up) = if a <= b {
        (a, b, &forward, &backward)
    } else {
        (b, a, &backward, &forward)
    };
    let mut lines = vec![format!("{} ↔ {}", name(lo), name(hi))];
    for rel in down {
        lines.push(format!("{} → {}: {rel}", name(lo), name(hi)));
    }
    for rel in up {
        lines.push(format!("{} → {}: {rel}", name(hi), name(lo)));
    }
    if lines.len() > MAX_LINES {
        let more = lines.len() - (MAX_LINES - 1);
        lines.truncate(MAX_LINES - 1);
        lines.push(format!("… +{more} more"));
    }
    lines.join("\n")
}

pub struct EdgeHoverPlugin;

impl Plugin for EdgeHoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoveredEdge>()
            .add_systems(Startup, spawn_tooltip)
            .add_systems(
                Update,
                (pick_edge, update_tooltip).chain().in_set(CoreSet::Ui),
            );
    }
}

fn spawn_tooltip(mut commands: Commands) {
    // The highlight first, so the plate is drawn on top of it.
    commands.spawn((
        EdgeHighlight,
        Node {
            position_type: PositionType::Absolute,
            height: Val::Px(3.0),
            ..default()
        },
        UiTransform::IDENTITY,
        BackgroundColor(HIGHLIGHT),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
    commands.spawn((
        EdgeTooltip,
        Node {
            position_type: PositionType::Absolute,
            padding: UiRect::axes(Val::Px(7.0), Val::Px(3.0)),
            border_radius: BorderRadius::all(Val::Px(5.0)),
            ..default()
        },
        // `left`/`top` hold the anchor (bottom centre of the plate).
        UiTransform::from_translation(Val2 {
            x: Val::Percent(-50.0),
            y: Val::Percent(-100.0),
        }),
        BackgroundColor(PLATE),
        Text::new(String::new()),
        TextFont {
            font_size: FontSize::Px(13.0),
            weight: FontWeight::SEMIBOLD,
            ..default()
        },
        TextColor(TEXT),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

#[allow(clippy::too_many_arguments)]
fn pick_edge(
    graph: Res<ViewerGraph>,
    layout: Res<LayoutState>,
    filter: Res<ClassFilter>,
    node_hover: Res<HoveredNode>,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<SceneCamera>>,
    ui_hover: Query<&Hovered, With<Node>>,
    mut hovered: ResMut<HoveredEdge>,
) {
    let hit = (|| {
        // A node under the pointer, a panel, or a drag in progress wins.
        if node_hover.0.is_some()
            || ui_hover.iter().any(Hovered::get)
            || buttons.get_pressed().next().is_some()
        {
            return None;
        }
        let cursor = window.single().ok()?.cursor_position()?;
        let (camera, transform) = camera.single().ok()?;
        let screen: Vec<Option<Vec2>> = layout
            .pos
            .iter()
            .enumerate()
            .map(|(i, p)| {
                graph.nodes.get(i).and_then(Option::as_ref)?;
                camera.world_to_viewport(transform, *p).ok()
            })
            .collect();
        let shown = |e: &Edge| {
            [e.from, e.to].iter().all(|id| {
                graph
                    .nodes
                    .get(id.0 as usize)
                    .and_then(Option::as_ref)
                    .is_some_and(|n| filter.is_visible(n))
            })
        };
        nearest_edge(&screen, &graph.edges, shown, cursor, PICK_PX)
    })();
    // Write only on a change, so `is_changed` stays meaningful.
    if hovered.0 != hit {
        hovered.0 = hit;
    }
}

#[allow(clippy::type_complexity)]
fn update_tooltip(
    graph: Res<ViewerGraph>,
    hovered: Res<HoveredEdge>,
    mut tooltip: Query<
        (&mut Node, &mut Text, &mut Visibility),
        (With<EdgeTooltip>, Without<EdgeHighlight>),
    >,
    mut highlight: Query<
        (&mut Node, &mut UiTransform, &mut Visibility),
        (With<EdgeHighlight>, Without<EdgeTooltip>),
    >,
) {
    let shown = hovered
        .0
        .as_ref()
        .and_then(|hit| graph.edges.get(hit.index).map(|edge| (hit, edge)));

    for (mut node, mut text, mut visibility) in &mut tooltip {
        match shown {
            Some((hit, edge)) => {
                let wanted = tooltip_text(&graph, edge);
                if **text != wanted {
                    **text = wanted;
                }
                node.left = Val::Px(hit.closest.x);
                node.top = Val::Px(hit.closest.y - 14.0);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    for (mut node, mut transform, mut visibility) in &mut highlight {
        match shown {
            Some((hit, _)) => {
                let delta = hit.b - hit.a;
                let mid = (hit.a + hit.b) / 2.0;
                let length = delta.length();
                // A thin rectangle as long as the edge, centred on its
                // middle and turned to its angle (rotation is about the
                // node's own centre).
                node.width = Val::Px(length);
                node.left = Val::Px(mid.x - length / 2.0);
                node.top = Val::Px(mid.y - 1.5);
                transform.rotation = Rot2::radians(delta.y.atan2(delta.x));
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeStyle, NodeId};

    fn edge(from: u32, to: u32) -> Edge {
        Edge {
            from: NodeId(from),
            to: NodeId(to),
            predicate: "<https://example.org/e2e#dependsOn>".into(),
            style: EdgeStyle::Relation,
        }
    }

    fn at(x: f32, y: f32) -> Option<Vec2> {
        Some(Vec2::new(x, y))
    }

    #[test]
    fn distance_to_a_segment_is_perpendicular_inside_and_to_the_end_beyond() {
        let (a, b) = (Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        let (d, c) = distance_to_segment(Vec2::new(4.0, 3.0), a, b);
        assert!((d - 3.0).abs() < 1e-5 && c == Vec2::new(4.0, 0.0));
        // Past the end, the closest point is the end, not the infinite line.
        let (d, c) = distance_to_segment(Vec2::new(14.0, 3.0), a, b);
        assert!((d - 5.0).abs() < 1e-5 && c == b);
        // A zero-length segment is a point.
        let (d, _) = distance_to_segment(Vec2::new(3.0, 4.0), a, a);
        assert!((d - 5.0).abs() < 1e-5);
    }

    #[test]
    fn the_closest_edge_within_the_threshold_is_hit() {
        // Two horizontal edges, 20 px apart.
        let screen = [at(0.0, 0.0), at(100.0, 0.0), at(0.0, 20.0), at(100.0, 20.0)];
        let edges = [edge(0, 1), edge(2, 3)];
        let hit = nearest_edge(&screen, &edges, |_| true, Vec2::new(50.0, 16.0), 7.0).unwrap();
        assert_eq!(hit.index, 1);
        assert_eq!(hit.closest, Vec2::new(50.0, 20.0));
        // Between the two, farther than the threshold from both: no hit.
        assert!(nearest_edge(&screen, &edges, |_| true, Vec2::new(50.0, 10.0), 7.0).is_none());
    }

    #[test]
    fn filtered_off_screen_and_self_edges_are_never_hit() {
        let screen = [at(0.0, 0.0), at(100.0, 0.0), None];
        let cursor = Vec2::new(50.0, 1.0);
        // Filtered out by the class filter.
        assert!(nearest_edge(&screen, &[edge(0, 1)], |_| false, cursor, 7.0).is_none());
        // One end is behind the camera.
        assert!(nearest_edge(&screen, &[edge(0, 2)], |_| true, cursor, 7.0).is_none());
        // A self-loop has no segment to point at.
        assert!(nearest_edge(&screen, &[edge(0, 0)], |_| true, Vec2::new(0.0, 0.0), 7.0).is_none());
    }

    #[test]
    fn the_nearer_of_two_crossing_edges_wins() {
        let screen = [
            at(0.0, 0.0),
            at(100.0, 100.0),
            at(0.0, 100.0),
            at(100.0, 0.0),
        ];
        let edges = [edge(0, 1), edge(2, 3)];
        // Just off the diagonal 0-1, far from the other one.
        let hit = nearest_edge(&screen, &edges, |_| true, Vec2::new(20.0, 22.0), 7.0).unwrap();
        assert_eq!(hit.index, 0);
    }

    fn named(label: &str) -> crate::graph::Node {
        crate::graph::Node {
            term: format!("<urn:{label}>"),
            kind: crate::graph::NodeKind::Instance,
            label: label.to_string(),
            attributes: Default::default(),
            types: Default::default(),
            mentions: 1,
            last_touched: 0,
        }
    }

    fn graph_with(edges: Vec<Edge>) -> ViewerGraph {
        let mut g = ViewerGraph::empty();
        g.nodes = vec![Some(named("A")), Some(named("B"))];
        g.edges = edges;
        g
    }

    fn rel(from: u32, to: u32, name: &str) -> Edge {
        Edge {
            from: NodeId(from),
            to: NodeId(to),
            predicate: format!("<https://example.org/e2e#{name}>"),
            style: EdgeStyle::Relation,
        }
    }

    #[test]
    fn a_one_way_edge_names_the_relation_then_its_ends() {
        let g = graph_with(vec![rel(0, 1, "dependsOn")]);
        assert_eq!(tooltip_text(&g, &g.edges[0]), "dependsOn\nA → B");
        // Unknown ends degrade to "?" instead of panicking.
        let empty = ViewerGraph::empty();
        assert_eq!(tooltip_text(&empty, &edge(0, 1)), "dependsOn\n? → ?");
    }

    #[test]
    fn a_two_way_pair_shows_both_relations_whichever_edge_is_hovered() {
        let g = graph_with(vec![rel(0, 1, "dependsOn"), rel(1, 0, "notifies")]);
        let expected = "A ↔ B\nA → B: dependsOn\nB → A: notifies";
        assert_eq!(tooltip_text(&g, &g.edges[0]), expected);
        // Hovering the other edge of the pair reads exactly the same.
        assert_eq!(tooltip_text(&g, &g.edges[1]), expected);
    }

    #[test]
    fn several_relations_one_way_are_joined_and_duplicates_collapse() {
        let g = graph_with(vec![
            rel(0, 1, "dependsOn"),
            rel(0, 1, "uses"),
            rel(0, 1, "uses"),
        ]);
        assert_eq!(tooltip_text(&g, &g.edges[0]), "dependsOn, uses\nA → B");
    }

    #[test]
    fn a_long_two_way_list_is_capped() {
        let mut edges = Vec::new();
        for i in 0..4 {
            edges.push(rel(0, 1, &format!("a{i}")));
            edges.push(rel(1, 0, &format!("b{i}")));
        }
        let g = graph_with(edges);
        let text = tooltip_text(&g, &g.edges[0]);
        assert_eq!(text.lines().count(), MAX_LINES);
        assert!(text.ends_with("… +4 more"), "{text}");
    }

    #[test]
    fn long_labels_are_clipped() {
        assert_eq!(clip("short"), "short");
        let long = "x".repeat(60);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), CLIP_CHARS);
        assert!(clipped.ends_with('…'));
    }
}
