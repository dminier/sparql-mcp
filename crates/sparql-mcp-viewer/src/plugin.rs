//! Bevy wiring and the Bevy-side resources shared by the headless and
//! windowed paths: `CorePlugin` (connection + graph, both paths) and
//! `ViewerPlugin` (windowed path: layout, render, camera).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::camera::CameraPlugin;
use crate::connection::{self, ConnState, ShutdownHandle, ViewerMsg};
use crate::graph::{NodeId, Remap, ViewerGraph};
use crate::layout::{LayoutParams, LayoutState};
use crate::render::{
    animate_spawn_despawn, layout_step_system, sync_entities, update_edge_meshes, write_transforms,
    RenderPlugin,
};

#[derive(Resource)]
pub struct Inbox(pub crossbeam_channel::Receiver<ViewerMsg>);

#[derive(Resource, Default)]
pub struct FrameMessages(pub Vec<ViewerMsg>);

#[derive(Resource)]
pub struct ConnStatus(pub ConnState);

/// Kept for the whole App lifetime so dropping the App stops the
/// connection thread's runtime.
#[derive(Resource)]
pub struct ConnectionHandle(#[allow(dead_code)] pub ShutdownHandle);

/// `Update` sets, chained in this order. Bevy never infers order from data
/// access, so every required order is written out.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreSet {
    Net,
    Graph,
    Sync,
}

/// Connects to the viewer side-channel and keeps `ViewerGraph` in sync.
/// Used by both the headless and windowed paths.
pub struct CorePlugin {
    pub addr: SocketAddr,
    pub connect_timeout: Duration,
}

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        let (rx, handle) = connection::spawn(self.addr, self.connect_timeout);
        app.insert_resource(Inbox(rx));
        app.insert_resource(FrameMessages::default());
        app.insert_resource(ConnStatus(ConnState::Connecting));
        app.insert_resource(ConnectionHandle(handle));
        app.insert_resource(ViewerGraph::empty());

        app.configure_sets(
            Update,
            (CoreSet::Net, CoreSet::Graph, CoreSet::Sync).chain(),
        );
        app.add_systems(
            Update,
            (
                drain_inbox.in_set(CoreSet::Net),
                apply_to_graph.in_set(CoreSet::Graph),
            ),
        );
    }
}

/// Windowed path: `CorePlugin` plus layout, rendering and the camera.
/// Phase 5 adds `Selection`, `ClassFilter`, `ActivityFeed` and the
/// interact/ingest plugins here.
pub struct ViewerPlugin {
    pub addr: SocketAddr,
    pub connect_timeout: Duration,
}

/// Fixed seed so the same graph always lays out the same way.
const LAYOUT_SEED: u64 = 0x059A_2C4D;

impl Plugin for ViewerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CorePlugin {
            addr: self.addr,
            connect_timeout: self.connect_timeout,
        })
        .insert_resource(LayoutParams::default())
        .insert_resource(LayoutState::new(LAYOUT_SEED))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .add_plugins((RenderPlugin, CameraPlugin))
        .add_systems(Update, sync_entities.in_set(CoreSet::Sync))
        .add_systems(FixedUpdate, layout_step_system)
        .add_systems(
            PostUpdate,
            (write_transforms, animate_spawn_despawn, update_edge_meshes)
                .chain()
                .before(TransformSystems::Propagate),
        );
    }
}

/// Moves every pending message from [`Inbox`] into [`FrameMessages`].
fn drain_inbox(inbox: Res<Inbox>, mut frame: ResMut<FrameMessages>) {
    frame.0.clear();
    while let Ok(msg) = inbox.0.try_recv() {
        frame.0.push(msg);
    }
}

/// Applies each of this frame's messages to [`ViewerGraph`] and
/// [`ConnStatus`]. When a [`LayoutState`] exists (windowed path), new
/// nodes are placed and a same-graph rebuild carries positions over.
fn apply_to_graph(
    frame: Res<FrameMessages>,
    mut graph: ResMut<ViewerGraph>,
    mut status: ResMut<ConnStatus>,
    mut layout: Option<ResMut<LayoutState>>,
) {
    for msg in &frame.0 {
        match msg {
            ViewerMsg::Snapshot(snapshot) => {
                let (rebuilt, remap) = ViewerGraph::from_snapshot(snapshot, Some(&graph));
                if let Some(layout) = layout.as_deref_mut() {
                    let remap = remap.unwrap_or(Remap(Default::default()));
                    layout.remap(&remap, rebuilt.nodes.len());
                    let carried: HashSet<NodeId> = remap.0.values().copied().collect();
                    let fresh = live_ids(&rebuilt, 0)
                        .into_iter()
                        .filter(|id| !carried.contains(id))
                        .collect::<Vec<_>>();
                    layout.place_new(&rebuilt, &fresh);
                }
                *graph = rebuilt;
            }
            ViewerMsg::Event(event) => {
                // Node ids are never reused within one `ViewerGraph`, so
                // anything new is appended past the old length.
                let old_len = graph.nodes.len();
                graph.apply_event(event);
                if let Some(layout) = layout.as_deref_mut() {
                    layout.place_new(&graph, &live_ids(&graph, old_len));
                }
            }
            ViewerMsg::State(state) => {
                status.0 = state.clone();
            }
        }
    }
}

fn live_ids(graph: &ViewerGraph, from: usize) -> Vec<NodeId> {
    graph
        .nodes
        .iter()
        .enumerate()
        .skip(from)
        .filter(|(_, n)| n.is_some())
        .map(|(i, _)| NodeId(i as u32))
        .collect()
}
