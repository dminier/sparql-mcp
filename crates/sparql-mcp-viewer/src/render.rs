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

use crate::graph::{EdgeStyle, NodeId, NodeKind, ViewerGraph};
use crate::layout::{LayoutParams, LayoutState};

/// Links a spawned entity to the graph node it draws.
#[derive(Component, Debug, Clone, Copy)]
pub struct NodeRef(pub NodeId);

/// One mesh handle per shape and one material handle per style — the
/// viewer never creates a material per node.
#[derive(Resource)]
pub struct ViewerStyle {
    pub class_mesh: Handle<Mesh>,
    pub instance_mesh: Handle<Mesh>,
    pub class_material: Handle<StandardMaterial>,
    pub instance_material: Handle<StandardMaterial>,
}

/// The three `LineList` meshes, one per [`EdgeStyle`].
#[derive(Resource)]
pub struct EdgeMeshes {
    pub type_edges: Handle<Mesh>,
    pub subclass_edges: Handle<Mesh>,
    pub relation_edges: Handle<Mesh>,
}

/// Entity spawned for each node, plus the term it was spawned for — a
/// snapshot rebuild reuses ids, so the term tells a stale entity apart.
#[derive(Resource, Default)]
pub struct Spawned(HashMap<NodeId, (Entity, String, NodeKind)>);

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
        class_mesh: meshes.add(Sphere::new(0.6).mesh().ico(3).expect("valid ico sphere")),
        instance_mesh: meshes.add(Sphere::new(0.25).mesh().ico(2).expect("valid ico sphere")),
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
    let handles = EdgeMeshes {
        type_edges: meshes.add(empty_line_list()),
        subclass_edges: meshes.add(empty_line_list()),
        relation_edges: meshes.add(empty_line_list()),
    };
    for (mesh, color) in [
        (&handles.type_edges, Color::srgba(1.0, 0.7, 0.3, 0.45)),
        (&handles.subclass_edges, Color::srgba(1.0, 1.0, 1.0, 0.6)),
        (&handles.relation_edges, Color::srgba(0.5, 0.75, 1.0, 0.3)),
    ] {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(materials.add(edge_material(color))),
            Transform::IDENTITY,
            Pickable::IGNORE,
        ));
    }
    commands.insert_resource(handles);
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

/// Rewrite the three edge meshes' vertex positions. Only does work while
/// the layout is awake or the graph changed.
pub fn update_edge_meshes(
    graph: Res<ViewerGraph>,
    layout: Res<LayoutState>,
    edges: Option<Res<EdgeMeshes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last_generation: Local<Option<u64>>,
) {
    let Some(edges) = edges else { return };
    let graph_changed = *last_generation != Some(graph.generation);
    if !layout.awake && !graph_changed {
        return;
    }
    *last_generation = Some(graph.generation);

    let mut by_style: [Vec<[f32; 3]>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for edge in &graph.edges {
        let (Some(a), Some(b)) = (
            layout.pos.get(edge.from.0 as usize),
            layout.pos.get(edge.to.0 as usize),
        ) else {
            continue;
        };
        let bucket = match edge.style {
            EdgeStyle::Type => 0,
            EdgeStyle::SubClass => 1,
            EdgeStyle::Relation => 2,
        };
        by_style[bucket].push(a.to_array());
        by_style[bucket].push(b.to_array());
    }

    let [type_pos, subclass_pos, relation_pos] = by_style;
    for (handle, positions) in [
        (&edges.type_edges, type_pos),
        (&edges.subclass_edges, subclass_pos),
        (&edges.relation_edges, relation_pos),
    ] {
        if let Some(mut mesh) = meshes.get_mut(handle) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        }
    }
}
