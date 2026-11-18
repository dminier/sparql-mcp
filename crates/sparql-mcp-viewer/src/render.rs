//! Bevy rendering: entity reconciliation against `ViewerGraph`, shared
//! mesh/material handles, one `LineList` mesh per edge style, base
//! lights, arrival/departure scale animation, and the
//! `layout_step_system` / `write_transforms` wrappers around
//! `LayoutState`. HDR, bloom, tonemapping, fog and MSAA are Phase 6.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::picking::Pickable;
use bevy::prelude::*;

use crate::arrow::edge_segments;
use crate::graph::{EdgeStyle, NodeId, NodeKind, ViewerGraph};
use crate::ingest::LastBatch;
use crate::interact::{ClassFilter, Selection};
use crate::layout::{LayoutParams, LayoutState};

/// Links a spawned entity to the graph node it draws.
#[derive(Component, Debug, Clone, Copy)]
pub struct NodeRef(pub NodeId);

/// One mesh handle per shape and one material handle per style (class,
/// instance, selected, faded, last batch) — the viewer never creates a
/// material per node.
#[derive(Resource)]
pub struct ViewerStyle {
    pub class_mesh: Handle<Mesh>,
    pub instance_mesh: Handle<Mesh>,
    pub class_material: Handle<StandardMaterial>,
    pub instance_material: Handle<StandardMaterial>,
    pub selected_material: Handle<StandardMaterial>,
    pub faded_material: Handle<StandardMaterial>,
    pub last_batch_material: Handle<StandardMaterial>,
    /// How long the last batch pulses.
    pub pulse_secs: f32,
}

/// The `LineList` edge meshes: one per [`EdgeStyle`], each in a normal
/// and a faded variant (an edge is faded if either end is filtered out).
#[derive(Resource)]
pub struct EdgeMeshes(pub [Handle<Mesh>; 6]);

fn edge_slot(style: EdgeStyle, faded: bool) -> usize {
    let base = match style {
        EdgeStyle::Type => 0,
        EdgeStyle::SubClass => 2,
        EdgeStyle::Relation => 4,
    };
    base + usize::from(faded)
}

/// Entity spawned for each node, plus the term it was spawned for — a
/// snapshot rebuild reuses ids, so the term tells a stale entity apart.
#[derive(Resource, Default)]
pub struct Spawned(HashMap<NodeId, (Entity, String, NodeKind)>);

/// World-space radius of a class sphere and of an instance sphere. Labels use
/// them to sit just outside the node they name.
pub const CLASS_RADIUS: f32 = 0.6;
pub const INSTANCE_RADIUS: f32 = 0.25;

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Spawned>()
            .insert_resource(GlobalAmbientLight {
                brightness: 300.0,
                ..default()
            })
            .add_systems(Startup, (setup_style, setup_edges, setup_lights));
    }
}

fn setup_style(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(ViewerStyle {
        class_mesh: meshes.add(
            Sphere::new(CLASS_RADIUS)
                .mesh()
                .ico(3)
                .expect("valid ico sphere"),
        ),
        instance_mesh: meshes.add(
            Sphere::new(INSTANCE_RADIUS)
                .mesh()
                .ico(2)
                .expect("valid ico sphere"),
        ),
        class_material: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.55, 0.15),
            emissive: LinearRgba::rgb(2.0, 0.9, 0.2),
            ..default()
        }),
        instance_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.65, 0.95),
            perceptual_roughness: 0.9,
            ..default()
        }),
        selected_material: materials.add(StandardMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(2.5, 2.5, 2.5),
            ..default()
        }),
        faded_material: materials.add(StandardMaterial {
            base_color: Color::srgba(0.5, 0.55, 0.65, 0.12),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        }),
        last_batch_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.4, 1.0, 0.55),
            emissive: LinearRgba::rgb(0.4, 1.6, 0.6),
            ..default()
        }),
        pulse_secs: 2.0,
    });
}

fn empty_line_list() -> Mesh {
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new())
}

fn setup_edges(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let edge_material = |color: Color| StandardMaterial {
        base_color: color,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    };
    const FADED_ALPHA: f32 = 0.05;
    let colors = [
        (EdgeStyle::Type, Color::srgba(1.0, 0.7, 0.3, 0.45)),
        (EdgeStyle::SubClass, Color::srgba(1.0, 1.0, 1.0, 0.6)),
        (EdgeStyle::Relation, Color::srgba(0.5, 0.75, 1.0, 0.3)),
    ];
    let handles: [Handle<Mesh>; 6] = std::array::from_fn(|_| meshes.add(empty_line_list()));
    for (style, color) in colors {
        for faded in [false, true] {
            let color = if faded {
                color.with_alpha(FADED_ALPHA)
            } else {
                color
            };
            commands.spawn((
                Mesh3d(handles[edge_slot(style, faded)].clone()),
                MeshMaterial3d(materials.add(edge_material(color))),
                Transform::IDENTITY,
                Pickable::IGNORE,
            ));
        }
    }
    commands.insert_resource(EdgeMeshes(handles));
}

fn setup_lights(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            ..default()
        },
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Spawn/despawn entities so there is exactly one per live node, and keep
/// each entity's mesh/material matching its node's kind. Runs only when
/// the graph's `generation` moved.
pub fn sync_entities(
    mut commands: Commands,
    graph: Res<ViewerGraph>,
    style: Option<Res<ViewerStyle>>,
    mut spawned: ResMut<Spawned>,
    mut last_generation: Local<Option<u64>>,
) {
    let Some(style) = style else { return };
    if *last_generation == Some(graph.generation) {
        return;
    }
    *last_generation = Some(graph.generation);

    let handles_for = |kind: NodeKind| match kind {
        NodeKind::Class => (style.class_mesh.clone(), style.class_material.clone()),
        NodeKind::Instance => (style.instance_mesh.clone(), style.instance_material.clone()),
    };

    spawned.0.retain(|id, (entity, term, kind)| {
        let Some(node) = graph
            .nodes
            .get(id.0 as usize)
            .and_then(Option::as_ref)
            .filter(|node| node.term == *term)
        else {
            // Gone, or its id now names another node after a rebuild:
            // shrink it out where it stands.
            commands
                .entity(*entity)
                .remove::<(NodeRef, Spawning)>()
                .insert(Despawning::default());
            return false;
        };
        if node.kind != *kind {
            // Same node, newly a class: swap the shared handles in place.
            let (mesh, material) = handles_for(node.kind);
            commands
                .entity(*entity)
                .insert((Mesh3d(mesh), MeshMaterial3d(material)));
            *kind = node.kind;
        }
        true
    });

    for (i, slot) in graph.nodes.iter().enumerate() {
        let Some(node) = slot else { continue };
        let id = NodeId(i as u32);
        if spawned.0.contains_key(&id) {
            continue;
        }
        let (mesh, material) = handles_for(node.kind);
        let entity = commands
            .spawn((
                NodeRef(id),
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_scale(Vec3::ZERO),
                Spawning::default(),
            ))
            .id();
        spawned.0.insert(id, (entity, node.term.clone(), node.kind));
    }
}

/// Arrival/departure animation length.
pub const ANIM_SECS: f32 = 0.3;

/// A new node growing from scale 0 to 1.
#[derive(Component, Default)]
pub struct Spawning {
    elapsed: f32,
}

/// A removed node shrinking to 0, then despawned. Its `NodeRef` is gone,
/// so the layout no longer moves it.
#[derive(Component, Default)]
pub struct Despawning {
    elapsed: f32,
    from: Option<f32>,
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Drives [`Spawning`] and [`Despawning`]. Acts only on
/// `Transform.scale`, never on materials.
pub fn animate_spawn_despawn(
    mut commands: Commands,
    time: Res<Time>,
    mut arriving: Query<(Entity, &mut Spawning, &mut Transform), Without<Despawning>>,
    mut leaving: Query<(Entity, &mut Despawning, &mut Transform), Without<Spawning>>,
) {
    let dt = time.delta_secs();
    for (entity, mut spawning, mut transform) in &mut arriving {
        spawning.elapsed += dt;
        let t = spawning.elapsed / ANIM_SECS;
        transform.scale = Vec3::splat(ease_out_cubic(t));
        if t >= 1.0 {
            commands.entity(entity).remove::<Spawning>();
        }
    }
    for (entity, mut despawning, mut transform) in &mut leaving {
        let from = *despawning.from.get_or_insert(transform.scale.x);
        despawning.elapsed += dt;
        let t = despawning.elapsed / ANIM_SECS;
        transform.scale = Vec3::splat(from * (1.0 - ease_out_cubic(t)));
        if t >= 1.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// `FixedUpdate` wrapper around [`LayoutState::step`].
pub fn layout_step_system(
    graph: Res<ViewerGraph>,
    params: Res<LayoutParams>,
    time: Res<Time>,
    mut layout: ResMut<LayoutState>,
) {
    layout.step(&graph, time.delta_secs(), &params);
}

/// Copy layout positions into node transforms.
pub fn write_transforms(layout: Res<LayoutState>, mut nodes: Query<(&NodeRef, &mut Transform)>) {
    for (node_ref, mut transform) in &mut nodes {
        if let Some(pos) = layout.pos.get(node_ref.0 .0 as usize) {
            if transform.translation != *pos {
                transform.translation = *pos;
            }
        }
    }
}

/// The directed `(from, to)` pairs of the graph, with the generation they were
/// built for.
#[derive(Default)]
pub struct DirectedPairs(Option<u64>, std::collections::HashSet<(NodeId, NodeId)>);

/// Rewrite the edge meshes' vertex positions. Only does work while the
/// layout is awake, the graph changed, or the class filter changed.
pub fn update_edge_meshes(
    graph: Res<ViewerGraph>,
    layout: Res<LayoutState>,
    edges: Option<Res<EdgeMeshes>>,
    filter: Option<Res<ClassFilter>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last_generation: Local<Option<u64>>,
    mut pairs: Local<DirectedPairs>,
) {
    let Some(edges) = edges else { return };
    let graph_changed = *last_generation != Some(graph.generation);
    let filter_changed = filter.as_ref().is_some_and(|f| f.is_changed());
    if !layout.awake && !graph_changed && !filter_changed {
        return;
    }
    *last_generation = Some(graph.generation);

    let visible = |id: NodeId| match (&filter, graph.nodes.get(id.0 as usize)) {
        (Some(filter), Some(Some(node))) => filter.is_visible(node),
        _ => true,
    };
    // The directed pairs, rebuilt only when the graph changes: a pair linked
    // both ways is drawn once, with a head at each end.
    if pairs.0 != Some(graph.generation) {
        pairs.0 = Some(graph.generation);
        pairs.1 = graph.edges.iter().map(|e| (e.from, e.to)).collect();
    }
    let radius = |id: NodeId| match graph.nodes.get(id.0 as usize) {
        Some(Some(node)) if node.kind == NodeKind::Class => CLASS_RADIUS,
        _ => INSTANCE_RADIUS,
    };
    let mut buckets: [Vec<[f32; 3]>; 6] = Default::default();
    for edge in &graph.edges {
        let (Some(a), Some(b)) = (
            layout.pos.get(edge.from.0 as usize),
            layout.pos.get(edge.to.0 as usize),
        ) else {
            continue;
        };
        let reverse = pairs.1.contains(&(edge.to, edge.from));
        // The two edges of a two-way pair share one line: the one that goes
        // from the lower id draws it, its twin draws nothing.
        if reverse && edge.from > edge.to {
            continue;
        }
        let faded = !visible(edge.from) || !visible(edge.to);
        let bucket = &mut buckets[edge_slot(edge.style, faded)];
        for [p, q] in edge_segments(*a, *b, radius(edge.from), radius(edge.to), true, reverse) {
            bucket.push(p.to_array());
            bucket.push(q.to_array());
        }
    }

    for (handle, positions) in edges.0.iter().zip(buckets) {
        if let Some(mut mesh) = meshes.get_mut(handle) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        }
    }
}

/// The amplitude of the last-batch pulse: scale oscillates in `[1, 1.3]`.
const PULSE_AMPLITUDE: f32 = 0.3;
const PULSE_HZ: f32 = 1.5;

/// Pulse scale at `t` seconds into the pulse: 1.0 at t = 0, peaks at 1.3.
pub fn pulse_scale(t: f32) -> f32 {
    let phase = (t * PULSE_HZ * std::f32::consts::TAU).cos();
    1.0 + PULSE_AMPLITUDE * (0.5 - 0.5 * phase)
}

/// Pick each node's shared material — selected, then faded (filtered out),
/// then last batch, then its kind — and drive the last-batch pulse on
/// `Transform.scale`. Handles and scales are only written when they change.
#[allow(clippy::type_complexity)]
pub fn apply_node_materials(
    graph: Res<ViewerGraph>,
    style: Option<Res<ViewerStyle>>,
    selection: Option<Res<Selection>>,
    filter: Option<Res<ClassFilter>>,
    last_batch: Option<Res<LastBatch>>,
    mut nodes: Query<
        (
            &NodeRef,
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Transform,
        ),
        (Without<Spawning>, Without<Despawning>),
    >,
) {
    let Some(style) = style else { return };
    let selected = selection
        .as_ref()
        .and_then(|s| s.0.as_deref())
        .and_then(|t| graph.index.get(t))
        .copied();
    let pulse_t = last_batch.as_ref().and_then(|b| {
        let t = b.started?.elapsed().as_secs_f32();
        (t < style.pulse_secs).then_some((b.seq?, t))
    });

    for (node_ref, mut material, mut transform) in &mut nodes {
        let Some(Some(node)) = graph.nodes.get(node_ref.0 .0 as usize) else {
            continue;
        };
        let pulsing = pulse_t.filter(|(seq, _)| node.last_touched == *seq);
        let filtered_out = filter.as_ref().is_some_and(|f| !f.is_visible(node));
        let wanted = if selected == Some(node_ref.0) {
            &style.selected_material
        } else if filtered_out {
            &style.faded_material
        } else if pulsing.is_some() {
            &style.last_batch_material
        } else {
            match node.kind {
                NodeKind::Class => &style.class_material,
                NodeKind::Instance => &style.instance_material,
            }
        };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }

        let scale = pulsing.map_or(1.0, |(_, t)| pulse_scale(t));
        if (transform.scale.x - scale).abs() > 1e-4 {
            transform.scale = Vec3::splat(scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_starts_at_rest_and_stays_between_1_and_1_3() {
        assert!((pulse_scale(0.0) - 1.0).abs() < 1e-6);
        for i in 0..200 {
            let s = pulse_scale(i as f32 * 0.01);
            assert!((1.0..=1.3 + 1e-6).contains(&s), "scale {s} at step {i}");
        }
        let peak = pulse_scale(1.0 / (2.0 * PULSE_HZ));
        assert!((peak - 1.3).abs() < 1e-4);
    }

    #[test]
    fn edge_slots_are_distinct() {
        let mut slots: Vec<usize> = [EdgeStyle::Type, EdgeStyle::SubClass, EdgeStyle::Relation]
            .into_iter()
            .flat_map(|s| [edge_slot(s, false), edge_slot(s, true)])
            .collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 1, 2, 3, 4, 5]);
    }
}
