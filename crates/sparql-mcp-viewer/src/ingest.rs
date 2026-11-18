//! Ingestion view: the activity feed, the last-batch pulse and the resync
//! banner (`bevy_ui`).

use std::collections::VecDeque;
use std::time::Instant;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use sparql_mcp::application::event_sink::StoreEvent;

use crate::connection::{ConnState, ViewerMsg};
use crate::graph::ViewerGraph;
use crate::plugin::{ConnStatus, CoreSet, FrameMessages};

pub const FEED_CAPACITY: usize = 50;
/// How many of the most recent feed entries the panel shows.
const FEED_VISIBLE: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    Snapshot,
    Insert,
    Delete,
    Changed,
    Resync,
    OtherGraph,
}

impl FeedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Insert => "insert",
            Self::Delete => "delete",
            Self::Changed => "changed",
            Self::Resync => "resync",
            Self::OtherGraph => "other graph",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeedEntry {
    pub received_at: Instant,
    pub kind: FeedKind,
    pub triple_count: u64,
}

/// Ring buffer of the last [`FEED_CAPACITY`] messages, newest at the back.
#[derive(Resource, Default)]
pub struct ActivityFeed(pub VecDeque<FeedEntry>);

impl ActivityFeed {
    pub fn push(&mut self, kind: FeedKind, triple_count: u64) {
        if self.0.len() == FEED_CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back(FeedEntry {
            received_at: Instant::now(),
            kind,
            triple_count,
        });
    }
}

/// The `event_seq` of the latest applied insert/delete batch and when it
/// arrived. Nodes whose `last_touched` equals `seq` pulse. A snapshot
/// clears it, so a rebuild never pulses the whole graph.
#[derive(Resource, Default)]
pub struct LastBatch {
    pub seq: Option<u64>,
    pub started: Option<Instant>,
}

/// Classify one frame message for the feed. `graph` is the graph *after*
/// `apply_to_graph`, which is how an event for another graph is told apart.
pub fn feed_entry_for(msg: &ViewerMsg, graph: &str) -> Option<(FeedKind, u64)> {
    match msg {
        ViewerMsg::Snapshot(s) => Some((FeedKind::Snapshot, s.triples.len() as u64)),
        ViewerMsg::Event(event) => {
            let (event_graph, kind, count) = match event {
                StoreEvent::Insert { graph, triples, .. } => {
                    (graph, FeedKind::Insert, triples.len() as u64)
                }
                StoreEvent::Delete { graph, triples, .. } => {
                    (graph, FeedKind::Delete, triples.len() as u64)
                }
                StoreEvent::Changed {
                    graph,
                    triples_before,
                    triples_after,
                } => (
                    graph,
                    FeedKind::Changed,
                    triples_before.abs_diff(*triples_after),
                ),
            };
            if event_graph != graph {
                Some((FeedKind::OtherGraph, count))
            } else {
                Some((kind, count))
            }
        }
        ViewerMsg::State(_) | ViewerMsg::Request(_) => None,
    }
}

pub struct IngestPlugin;

impl Plugin for IngestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActivityFeed>()
            .init_resource::<LastBatch>()
            .add_systems(Startup, spawn_panels)
            .add_systems(
                Update,
                record_feed
                    .in_set(CoreSet::Graph)
                    .after(crate::plugin::apply_to_graph),
            )
            .add_systems(
                Update,
                (update_feed_panel, update_banner).in_set(CoreSet::Ui),
            );
    }
}

/// Reads this frame's messages after `apply_to_graph`.
fn record_feed(
    frame: Res<FrameMessages>,
    graph: Res<ViewerGraph>,
    mut feed: ResMut<ActivityFeed>,
    mut last_batch: ResMut<LastBatch>,
    mut was_streaming: Local<bool>,
) {
    for msg in &frame.0 {
        match msg {
            ViewerMsg::State(state) => {
                let streaming = matches!(state, ConnState::Streaming);
                if *was_streaming && !streaming {
                    feed.push(FeedKind::Resync, 0);
                }
                *was_streaming = streaming;
            }
            ViewerMsg::Snapshot(_) => {
                last_batch.seq = None;
                last_batch.started = None;
            }
            ViewerMsg::Event(
                StoreEvent::Insert { graph: g, .. } | StoreEvent::Delete { graph: g, .. },
            ) if *g == graph.graph => {
                last_batch.seq = Some(graph.event_seq);
                last_batch.started = Some(Instant::now());
            }
            ViewerMsg::Event(_) | ViewerMsg::Request(_) => {}
        }
        if let Some((kind, count)) = feed_entry_for(msg, &graph.graph) {
            feed.push(kind, count);
        }
    }
}

#[derive(Component)]
struct FeedText;

#[derive(Component)]
struct BannerRoot;

#[derive(Component)]
struct BannerText;

fn panel_background() -> BackgroundColor {
    BackgroundColor(Color::srgba(0.04, 0.05, 0.09, 0.82))
}

fn spawn_panels(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                padding: UiRect::all(Val::Px(10.0)),
                min_width: Val::Px(260.0),
                ..default()
            },
            panel_background(),
            Hovered::default(),
        ))
        .with_children(|panel| {
            panel.spawn((
                FeedText,
                Text::new("activity\n(waiting)"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgb(0.8, 0.85, 0.95)),
            ));
        });

    commands
        .spawn((
            BannerRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-160.0)),
                width: Val::Px(320.0),
                padding: UiRect::all(Val::Px(8.0)),
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.45, 0.25, 0.05, 0.9)),
            Visibility::Hidden,
        ))
        .with_children(|banner| {
            banner.spawn((
                BannerText,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}

fn update_feed_panel(feed: Res<ActivityFeed>, mut text: Query<&mut Text, With<FeedText>>) {
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    let mut lines = vec!["activity".to_string()];
    for entry in feed.0.iter().rev().take(FEED_VISIBLE) {
        lines.push(format!(
            "{:>5.1}s ago  {:<11} {:>6} triples",
            entry.received_at.elapsed().as_secs_f32(),
            entry.kind.label(),
            entry.triple_count
        ));
    }
    if feed.0.is_empty() {
        lines.push("(waiting)".to_string());
    }
    **text = lines.join("\n");
}

pub fn banner_message(state: &ConnState, now: Instant) -> Option<String> {
    match state {
        ConnState::Streaming => None,
        ConnState::Connecting => Some("Connecting to the side-channel…".to_string()),
        ConnState::AwaitSnapshot => Some("Resync: waiting for a fresh snapshot…".to_string()),
        ConnState::Backoff { retry_at } => Some(format!(
            "Disconnected — retrying in {:.0}s",
            retry_at.saturating_duration_since(now).as_secs_f32().ceil()
        )),
    }
}

fn update_banner(
    status: Res<ConnStatus>,
    mut banner: Query<&mut Visibility, With<BannerRoot>>,
    mut text: Query<&mut Text, With<BannerText>>,
) {
    let message = banner_message(&status.0, Instant::now());
    if let Ok(mut visibility) = banner.single_mut() {
        let wanted = if message.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
    if let (Some(message), Ok(mut text)) = (message, text.single_mut()) {
        if **text != message {
            **text = message;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparql_mcp::application::event_sink::Snapshot;
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn feed_is_a_ring_buffer_of_50() {
        let mut feed = ActivityFeed::default();
        for i in 0..60 {
            feed.push(FeedKind::Insert, i);
        }
        assert_eq!(feed.0.len(), FEED_CAPACITY);
        assert_eq!(feed.0.front().unwrap().triple_count, 10);
        assert_eq!(feed.0.back().unwrap().triple_count, 59);
    }

    #[test]
    fn feed_classifies_kinds_and_other_graph() {
        let snapshot = ViewerMsg::Snapshot(Snapshot {
            graph: "g".into(),
            triples: vec![["<a>".into(), "<p>".into(), "<b>".into()]],
            kinds: BTreeMap::new(),
        });
        assert_eq!(
            feed_entry_for(&snapshot, "g"),
            Some((FeedKind::Snapshot, 1))
        );

        let insert = |graph: &str| {
            ViewerMsg::Event(StoreEvent::Insert {
                graph: graph.into(),
                triples: vec![["<a>".into(), "<p>".into(), "<b>".into()]; 3],
                kinds: BTreeMap::new(),
            })
        };
        assert_eq!(
            feed_entry_for(&insert("g"), "g"),
            Some((FeedKind::Insert, 3))
        );
        assert_eq!(
            feed_entry_for(&insert("other"), "g"),
            Some((FeedKind::OtherGraph, 3))
        );

        let changed = ViewerMsg::Event(StoreEvent::Changed {
            graph: "g".into(),
            triples_before: 10,
            triples_after: 4,
        });
        assert_eq!(feed_entry_for(&changed, "g"), Some((FeedKind::Changed, 6)));
        assert_eq!(
            feed_entry_for(&ViewerMsg::State(ConnState::Streaming), "g"),
            None
        );
    }

    #[test]
    fn banner_hidden_only_while_streaming() {
        let now = Instant::now();
        assert!(banner_message(&ConnState::Streaming, now).is_none());
        assert!(banner_message(&ConnState::Connecting, now).is_some());
        let msg = banner_message(
            &ConnState::Backoff {
                retry_at: now + Duration::from_secs(4),
            },
            now,
        )
        .unwrap();
        assert!(msg.contains("4s"), "{msg}");
    }
}
