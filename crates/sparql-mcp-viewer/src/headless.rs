//! Windowless mode — `--headless`. No renderer, no window: `MinimalPlugins`
//! plus [`CorePlugin`], driven by a fixed-rate schedule runner.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use sparql_mcp::application::event_sink::StoreEvent;

use crate::connection::{ConnState, ViewerMsg};
use crate::graph::ViewerGraph;
use crate::plugin::{ConnStatus, CorePlugin, CoreSet, FrameMessages};

#[derive(Resource)]
struct StartedAt(Instant);

#[derive(Resource)]
struct ConnectTimeout(Duration);

#[derive(Resource, Default)]
struct ReachedStreaming(bool);

/// Connects to `addr`, prints one line per applied message, and exits
/// with code 2 if the connection has never reached `Streaming` within
/// `connect_timeout` of startup.
pub fn run(addr: SocketAddr, connect_timeout: Duration) -> AppExit {
    let mut app = App::new();
    app.add_plugins(
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / 30.0,
        ))),
    );
    app.add_plugins(CorePlugin {
        addr,
        connect_timeout,
    });
    app.insert_resource(StartedAt(Instant::now()));
    app.insert_resource(ConnectTimeout(connect_timeout));
    app.insert_resource(ReachedStreaming::default());
    app.add_systems(Update, print_messages.after(CoreSet::Graph));
    app.add_systems(Update, check_connect_timeout);
    app.run()
}

/// One line per applied snapshot/insert/delete/changed message — not per
/// connection-state transition, which carries no graph content.
fn print_messages(frame: Res<FrameMessages>, graph: Res<ViewerGraph>) {
    for msg in &frame.0 {
        let kind = match msg {
            ViewerMsg::Snapshot(_) => "snapshot",
            ViewerMsg::Event(StoreEvent::Insert { .. }) => "insert",
            ViewerMsg::Event(StoreEvent::Delete { .. }) => "delete",
            ViewerMsg::Event(StoreEvent::Changed { .. }) => "changed",
            ViewerMsg::State(_) | ViewerMsg::Request(_) => continue,
        };
        let nodes = graph.nodes.iter().filter(|n| n.is_some()).count();
        println!("{kind} nodes={nodes} edges={}", graph.edges.len());
    }
}

fn check_connect_timeout(
    started: Res<StartedAt>,
    timeout: Res<ConnectTimeout>,
    status: Res<ConnStatus>,
    mut reached: ResMut<ReachedStreaming>,
    mut exit: MessageWriter<AppExit>,
) {
    if matches!(status.0, ConnState::Streaming) {
        reached.0 = true;
    }
    if !reached.0 && started.0.elapsed() >= timeout.0 {
        exit.write(AppExit::from_code(2));
    }
}
