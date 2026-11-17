//! Bevy wiring and the Bevy-side resources shared by the headless and
//! windowed paths. `ViewerPlugin` (windowed path, render/camera/layout)
//! is added in Phase 3 — this file only has `CorePlugin` for now.

use std::net::SocketAddr;
use std::time::Duration;

use bevy::prelude::*;

use crate::connection::{self, ConnState, ShutdownHandle, ViewerMsg};
use crate::graph::ViewerGraph;

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

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreSet {
    Net,
    Graph,
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

        app.configure_sets(Update, (CoreSet::Net, CoreSet::Graph).chain());
        app.add_systems(
            Update,
            (
                drain_inbox.in_set(CoreSet::Net),
                apply_to_graph.in_set(CoreSet::Graph),
            ),
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
/// [`ConnStatus`]. Phase 3 adds the `LayoutState` interaction (`place_new`
/// / `remap` after a rebuild) once that resource exists.
fn apply_to_graph(
    frame: Res<FrameMessages>,
    mut graph: ResMut<ViewerGraph>,
    mut status: ResMut<ConnStatus>,
) {
    for msg in &frame.0 {
        match msg {
            ViewerMsg::Snapshot(snapshot) => {
                let (rebuilt, _remap) = ViewerGraph::from_snapshot(snapshot, Some(&graph));
                *graph = rebuilt;
            }
            ViewerMsg::Event(event) => {
                graph.apply_event(event);
            }
            ViewerMsg::State(state) => {
                status.0 = state.clone();
            }
        }
    }
}
