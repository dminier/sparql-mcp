# sparql-mcp Viewer: 3D Knowledge Graph Client

*   **Author:** dminier
*   **Status:** Draft
*   **Date:** 2026-11-17

--------------------------------------------------------------------------------

## Overview

`sparql-mcp` stores an agent's working memory as RDF triples in an Oxigraph
SPARQL store. The companion design
[`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md)
adds a loopback TCP side-channel to `sparql-mcp-core`. On connect, the channel
sends a snapshot of the active project graph. After that, it sends one event
per committed write.

This document designs the **client** of that channel. The client is
`crates/sparql-mcp-viewer`, a native Rust application built on Bevy (wgpu). It
turns the triple stream into a 3D node-link graph that grows while an agent
writes. It also shows what each ingestion batch changed, and lets the user
inspect any node.

Ownership is split between the two documents:

| Concern | Owned by |
|---|---|
| Server event emission, TCP listener, wire protocol, `[viewer]` config | `interface-agentic-rust-stylee.md` |
| RDF-to-graph projection, layout, rendering, interaction, ingestion view, delivery phases | this document |

This document consumes the wire protocol and does not redefine it. Any
protocol change is made in the companion design first.

## The Problem

The companion design fixes how data leaves the store. It leaves the following
viewer behavior undecided:

*   **Mapping RDF to a drawable graph.** A naive "every RDF term is a node"
    mapping turns each literal (labels, dates, comments) into its own node.
    In the `smc:` ontology, most triples have a literal object, so the result
    is mostly leaf noise around each subject.
*   **Classifying classes and instances.** The server classifies subjects per
    write batch. A later batch that mentions a class without its `rdf:type`
    triple would mark that class as an instance. The viewer needs a rule that
    stops a known class from being downgraded.
*   **Recovering from incomplete events.** Some events are coarse. A
    pattern-based `update_sparql` produces a `changed` event that carries only
    triple counts. A slow viewer can also fall behind the broadcast buffer
    (`Lagged`). The viewer needs a defined way to recover in both cases.
*   **Showing ingestion.** The user wants to see what the agent is ingesting,
    not only the result. A graph that silently grows does not show which
    write added which nodes.
*   **Testing without a window.** The viewer is a Windows GUI. If graph
    logic lives inside Bevy systems, no automated test can run it.
*   **Size of the work.** Rendering, layout, networking and UI together are
    too large for one implementation pass. The work needs an order and an exit
    criterion for each step.

## Goals

*   **Readable projection.** IRIs and blank nodes become nodes. Literals
    become attributes of their subject node, never separate nodes. `rdf:type`
    and `rdfs:subClassOf` edges are drawn in a style distinct from other
    predicates.
*   **Stable classification.** Once a node is classified as a class, it stays
    a class for the rest of the session, whatever later events say.
*   **Incremental growth.** An `insert` event becomes visible in under one
    second after the viewer receives it. Its new nodes appear next to a
    connected neighbor. The camera does not reset. Existing nodes move only
    through small per-tick layout adjustments, never by a full re-layout.
*   **Defined recovery.** On a `changed` event, on `Lagged`, on reconnect, and
    on a project switch, the viewer requests a fresh snapshot and rebuilds
    its graph. Nodes whose IRI was already known keep their positions.
*   **Visible ingestion.** An activity feed lists recent events (kind, triple
    count, time). Nodes touched by the most recent batch are highlighted.
*   **Inspection.** Selecting a node shows its label, IRI, literal attributes,
    and incoming and outgoing edges. Nodes can be filtered by class.
*   **Testability without a window.** Graph model and layout are plain Rust
    with no Bevy dependency and have their own unit tests. A `--headless` mode
    connects, applies events and prints node and edge counts, so the pipeline
    can be smoke-tested without a window.
*   **Deliberate visual quality.** The scene uses HDR rendering, bloom and
    tonemapping, and holds 60 fps with 5,000 nodes on the author's machine.
*   **Phased delivery.** Implementation is split into phases. Each phase has a
    pass/fail exit criterion, and work on a phase starts only when the
    previous phase's criterion is met.

## Non-Goals

*   **Writing to the store.** The viewer is read-only. Ingestion is done by
    agents through MCP tools (`kb-workbench`). The ingestion view only shows
    ingestion; it does not perform it.
*   **Server-side changes and the wire protocol.** These are covered by the
    companion design.
*   **SPARQL querying.** No query editor and no triple-store administration.
*   **Inference.** No RDFS/OWL reasoning. A node is a class only if a
    triple states it.
*   **Layout persistence.** Layout state does not survive a viewer restart in
    v1. A restart re-settles the layout, as in the companion design's CUJ 3.
*   **Custom render passes.** v1 uses Bevy's built-in renderer and materials.
    Hand-written wgpu passes (GPU instancing, compute-shader layout) are out
    of scope unless measurements show the built-in path misses the
    performance goal.
*   **Platforms other than Windows.** Not targeted in v1, same as the
    companion design.

## Critical User Journeys (CUJs)

### CUJ 1: Watching an ingestion batch arrive

*   **Given**: The viewer is connected and shows the current graph. The graph
    may be empty.
*   **When**: An agent calls `load_ontology` with three classes, then one
    `INSERT DATA` with twenty instances typed by those classes.
*   **Then**:
    *   Classes appear as large emissive nodes, and instances as small nodes
        placed next to their class through drawn `rdf:type` edges.
    *   The activity feed shows two entries with their triple counts.
    *   The twenty instances pulse briefly as "last batch".
    *   No existing node jumps, and the camera does not move.

### CUJ 2: Inspecting a node

*   **Given**: A populated graph is on screen.
*   **When**: The user clicks an instance node.
*   **Then**:
    *   A panel shows the node's label, full IRI (copyable), literal
        attributes grouped by predicate, and incoming and outgoing edges.
    *   Clicking an edge target in the panel selects that node.
*   **And when**: The user applies a class filter.
*   **Then**: Nodes outside the filter fade out but keep their positions.

### CUJ 3: Recovering from a coarse update

*   **Given**: The viewer is connected and shows a populated graph.
*   **When**: An agent runs a pattern-based `DELETE … WHERE` that removes ten
    instances. The server can only send a `changed` event for it.
*   **Then**:
    *   A "resync" banner appears, and the viewer obtains a fresh snapshot.
    *   Surviving nodes keep their positions, and the removed nodes fade out.
    *   The banner clears when the rebuild is done.

### CUJ 4: Smoke test without a window

*   **Given**: `sparql-mcp serve` is running natively on Windows with the
    side-channel enabled.
*   **When**: The developer runs `sparql-mcp-viewer --headless` on the same
    machine, and an agent writes triples.
*   **Then**: Each received event prints one line with the event kind and the
    graph's node and edge counts after applying it. The process exits with
    code 2 if it does not reach `Streaming` within `--connect-timeout`.

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

### Process and thread layout

```text
sparql-mcp-viewer.exe
┌──────────────────────────────────────────────────────────────────────┐
│ net thread (std::thread + tokio current_thread runtime)              │
│   connect → send {"cmd":"snapshot"} → read NDJSON lines              │
│   decode → ViewerMsg { Snapshot | Event | State(ConnState) }         │
│        │ crossbeam-channel (unbounded; Bevy drains it every frame)   │
│        ▼                                                             │
│ Bevy main thread                                                     │
│   Update:      Net → Graph → Sync → Ui   (see file 4)                │
│   FixedUpdate: layout_step                                           │
│   PostUpdate:  write_transforms → animations → edge meshes           │
│                                                                      │
│   ViewerGraph (Resource, plain Rust)   LayoutState (Resource)        │
│        ▲ source of truth                   ▲ positions, velocities   │
│        └── Bevy entities are a projection of these two resources     │
└──────────────────────────────────────────────────────────────────────┘
```

The network thread owns the socket and never touches Bevy state. Bevy
systems never block on I/O. The only coupling is the channel and the
`ViewerMsg` type. Rendering does not depend on network timing: a slow or
dead server leaves the last known graph on screen.

### Module boundaries

| Module | Depends on Bevy | Responsibility |
|---|---|---|
| `connection` | no (exposes a channel) | socket, reconnect with backoff, NDJSON decoding |
| `graph` | no | `ViewerGraph`, RDF-to-graph projection, `apply` for snapshot and events |
| `layout` | no | force simulation, placement of new nodes, sleep/wake |
| `plugin` | yes | `CorePlugin` (inbox, graph application; used headless and windowed) and `ViewerPlugin` (everything else); system sets and their order |
| `render` | yes | entity reconciliation, materials, edges, spawn and fade animations |
| `camera` | yes | Bevy's `PanOrbitCamera`, plus frame-all and UI input blocking |
| `interact` | yes | picking, selection panel, class filter |
| `ingest` | yes | activity feed, last-batch highlight, resync banner |
| `headless` | minimal (`MinimalPlugins` + `CorePlugin`) | prints counts, no window |

`graph` and `layout` compile and run their tests without Bevy. This is what
makes the "testability without a window" goal hold.

### Connection state machine

```text
Connecting ──ok──▶ AwaitSnapshot ──snapshot──▶ Streaming
    ▲                                           │  │
    │                    snapshot (project_switch) │  │ changed event
    │                    → rebuild, stay Streaming ◀┘  │ → reconnect now
    ├──────────────────────────────────────────────────┘   (no backoff)
    └── Backoff (1s, 2s, 4s … max 30s) ◀── socket closed / error /
                                           AwaitSnapshot timeout (30 s)
```

*   **Resync mechanism.** The companion protocol only sends a snapshot on
    connect (and after `project_switch`). So the viewer resyncs by closing
    and reopening the connection.
    *   A `changed` event causes an immediate reconnect, with no backoff.
    *   A socket error, or no snapshot within 30 s, goes through `Backoff`.
    *   The backoff delay resets to 1 s once the connection reaches
        `Streaming`.

    None of this needs a protocol change.
*   **Project switch.** The server pushes a new snapshot when the active
    graph changes (companion Required change 5). A `snapshot` message whose
    `graph` differs from the current graph replaces the graph, and positions
    are not kept.
*   **Server lag.** The server closes the connection on
    `RecvError::Lagged` (companion Required change 3). The viewer handles
    this like any socket error.

### RDF-to-graph projection (`graph`)

*   **Node.** Any IRI or blank node seen as subject or object. Each node
    stores the following:
    *   `kind: Class | Instance`, sticky (see below);
    *   `label`, resolved in this order: `rdfs:label`, then `smc:label`, then
        the IRI's local name, then the blank-node id;
    *   `attributes`, a map from predicate to a set of literals (N-Triples
        strings);
    *   `types`, the set of `rdf:type` objects;
    *   `last_touched`, the id of the last event that changed it.
*   **Edge.** A triple `(s, p, o)` whose object is an IRI or a blank node.
    Each edge has one of three styles: `Type` (`rdf:type`), `SubClass`
    (`rdfs:subClassOf`), or `Relation` (anything else).
*   **Literal triple.** Stored in the subject's `attributes`. It never creates
    a node or an edge.
*   **Classification.** A node becomes `Class` in two cases:
    *   the server's `kinds` map says `class`;
    *   the graph holds `(node, rdf:type, C)` where `C` is one of the
        companion's `CLASS_TYPE_IRIS`: `owl:Class`, `rdfs:Class`,
        `rdf:Property`, `owl:ObjectProperty`, or `owl:DatatypeProperty`.

    Once a node is `Class`, it is never downgraded during the session. The
    next snapshot recomputes classification from scratch.
*   **Set semantics.** The graph keeps the exact triple set. Inserting a
    triple that is already present, or deleting one that is absent, does
    nothing.
*   **Deletion.** A `delete` event removes the listed triples, matched by
    their exact N-Triples strings. Each node counts the triples that mention
    it, and it is removed when that count reaches 0.
*   **Graph filter.** Events whose `graph` is not the graph of the last
    snapshot are not applied. The activity feed records them as
    "other graph".
*   **Rebuild from snapshot.** Every snapshot builds a fresh graph with new
    node ids.
    *   If the snapshot is for the same graph, the layout positions of IRI
        nodes that exist in both graphs carry over.
    *   If it is for a different graph, nothing carries over.
    *   Blank nodes never carry over, because their ids are not stable
        between snapshots.
*   **Change counter.** `ViewerGraph` keeps a `generation` counter that
    increases on each change. Rendering reconciles only when the counter has
    moved.

### Layout (`layout`)

*   **Model.** Force-directed layout:
    *   all-pairs repulsion;
    *   spring attraction along edges, with `Type` edges stiffer so that
        instances cluster around their class;
    *   damping and a speed cap per tick.
*   **Timing.** Runs in `FixedUpdate` at a fixed rate. This keeps it
    independent of frame rate.
*   **New node placement.** A new node starts at a small, seeded random offset
    from its first connected neighbor that already has a position. If it has
    no such neighbor, it starts near the graph centroid. Existing positions
    are never reset.
*   **Sleep and wake.** When total kinetic energy drops below a threshold, the
    simulation stops. Any graph change wakes it up again.
*   **Scaling.** Repulsion uses a Barnes-Hut approximation over an octree,
    which brings the cost to about O(n log n) per step. This is v1 work,
    because the target is 5,000 nodes. The step budget is 4 ms at 5,000
    nodes in a release build. If a measurement exceeds it, the step moves to
    a worker thread with double-buffered positions. The `LayoutState`
    interface does not change, and this is a measured fallback, not v1
    work.

### Rendering (`render`, `camera`)

*   **Reconciliation.** One entity per node, tagged with
    `NodeRef(node_id)`. Reconciliation compares the graph's node set with
    the spawned entities, and spawns or despawns the difference.
*   **Shared handles.** There is one mesh handle per shape and one material
    handle per style: class, instance, selected, faded and last-batch. Bevy
    batches draws automatically. The viewer never creates a material per
    node.
*   **Arrival, departure and pulse.** Per-node animation acts only on
    `Transform.scale`, never on materials.
    *   **Arrival**: a new node scales from 0 to 1 over about 0.3 s, through
        a `Spawning` component.
    *   **Departure**: a removed node scales from 1 to 0 through a
        `Despawning` component, then the entity is removed.
    *   **Last-batch pulse**: a sine scale between 1.0 and 1.3, with the
        shared last-batch material.
*   **Edges.** All edges are drawn as one `LineList` mesh per edge style,
    with vertex positions rewritten only while the layout is awake. With a
    5,000-node target, per-frame immediate-mode `Gizmos` is not used.
    *   Edge meshes use an unlit, alpha-blended shared material.
    *   They are excluded from picking.
    *   An edge uses the faded style if either end is filtered out.
*   **Scene.** The scene is drawn as follows:
    *   the camera renders in HDR, with bloom and a filmic tonemapper;
    *   class materials use emissive values above 1, so that they bloom;
    *   instances use a matte `StandardMaterial`;
    *   distance fog, MSAA, and one directional light plus ambient light.
*   **Labels.** Labels are drawn only for classes and for hovered or selected
    nodes, as `bevy_ui` text placed each frame at the node's
    `world_to_viewport` position.
*   **Camera.** Bevy 0.20's `PanOrbitCamera` (`bevy_camera_controller`)
    provides rotate, zoom and pan. The viewer adds only two things: an `F`
    key that frames the whole graph, and disabling the controller while the
    pointer is over a UI panel. Graph changes never move the camera.

### Interaction and ingestion view (`interact`, `ingest`)

*   **Picking.** Uses Bevy's built-in picking on node meshes. Clicks over a
    UI panel do not reach the camera or the picking system.
*   **Selection panel.** Shows the content described in CUJ 2. All panels
    use `bevy_ui`, with no third-party UI crate.
*   **Activity feed.** A ring buffer of the last 50 messages, each with
    receive time, kind (`snapshot`, `insert`, `delete`, `changed`, or
    `resync`), and triple count.
*   **Last-batch highlight.** Nodes whose `last_touched` matches the latest
    event pulse for `ViewerStyle.pulse_secs`, which defaults to 2 s.
*   **Resync banner.** Visible while the connection is not `Streaming`, with
    the current state and the time until the next retry.

### Headless mode

`--headless` builds the App with `MinimalPlugins` and `CorePlugin`, with no
window and no renderer. It prints one line per applied message. It exits
with code 2 if it does not reach `Streaming` within `--connect-timeout`
(10 s by default).
This is the automated end-to-end check that runs in CI.

--------------------------------------------------------------------------------

## Alternatives Considered

The companion design already covers the alternatives that concern both
documents: WSLg, a browser-based renderer, a hand-rolled wgpu renderer, a
separate repository, polling, and merging the viewer with the server. They
are not repeated here.

That leaves one open challenge from the companion design's Stage 5 review:
auto-refreshing the existing Obsidian canvas instead of building a viewer.
It is a question about whether the viewer should exist at all, so it belongs
in that document too.

### Alternative A: Blocking `std::thread` for the network client

*   **Mechanism**: A plain thread reads a `std::net::TcpStream` line by line
    and sends decoded messages through `std::sync::mpsc`. Reconnection is a
    sleep loop.
*   **Pros**:
    *   no tokio in the Windows binary, so a smaller dependency tree;
    *   the simplest code.
*   **Cons**:
    *   a blocked read cannot be cancelled cleanly, so the thread is
        abandoned at shutdown;
    *   connect, read, and backoff timeouts each need their own setup, which
        does not scale as the connection logic grows.
*   **Reason for Rejection**: Rejected by author. With a tokio runtime, the
    connection state machine (connect timeout, backoff, clean shutdown) is
    written with standard async timers instead of hand-rolled blocking
    timeouts.

### Alternative B: Bevy `IoTaskPool` with `async-net` / `async-io`

*   **Mechanism**: An async task runs on Bevy's own I/O pool and writes into
    a channel.
*   **Pros**:
    *   no extra thread or runtime to manage;
    *   the task's lifetime follows the Bevy App.
*   **Cons**:
    *   uses a less common async stack (futures-lite);
    *   one accidental blocking call starves the pool that Bevy also uses
        for asset loading.
*   **Reason for Rejection**: Rejected by author. The network code should stay
    isolated from Bevy's executors, so that a networking bug cannot stall
    rendering or asset loading.

### Alternative C: ECS-native graph (nodes and edges as entities)

*   **Mechanism**: Each RDF node is a Bevy entity. Edges are relationship
    components. Projection and layout are Bevy systems that query
    components.
*   **Pros**:
    *   there is no separate model to keep in sync with entities;
    *   it is the idiomatic Bevy approach.
*   **Cons**:
    *   graph logic and layout can only be tested inside a Bevy `World`;
    *   the snapshot rebuild becomes a mass despawn and respawn;
    *   it is harder to move the layout to a worker thread later.
*   **Reason for Rejection**: Ruled out by the approved Goal "Testability
    without a window", which requires graph and layout code with no Bevy
    dependency. Entities stay a projection of `ViewerGraph`.

### Alternative D: Literals as graph nodes

*   **Mechanism**: Every RDF term, literals included, becomes a node, so the
    drawing mirrors the triple set one to one.
*   **Pros**:
    *   the drawing is complete and literal;
    *   no special case is needed in the projection.
*   **Cons**:
    *   in the `smc:` ontology and in typical KB content, most triples have
        a literal object. This multiplies the node count and fills the
        layout with leaf nodes;
    *   equal literals (for example the same date) would either merge
        unrelated subjects or need one node per occurrence.
*   **Reason for Rejection**: Ruled out by the approved Goal "Readable
    projection". Literals become attributes that the selection panel shows.

### Alternative E: Diff-based resync instead of reconnect

*   **Mechanism**: On `changed`, the viewer asks the server for a fresh
    snapshot on the same connection. Or the server sends the exact triples
    that changed, and the viewer applies the difference.
*   **Pros**:
    *   no reconnect cost;
    *   with exact triples, no full snapshot is needed.
*   **Cons**:
    *   both options need a protocol change, which belongs to the companion
        design;
    *   exact triples for pattern-based updates are listed there as a
        future improvement, not as v1 work.
*   **Reason for Rejection**: Proposed in this draft and approved with the
    Technical Plan. Reconnecting works with the protocol as it is defined
    today. If repeated snapshots turn out to be a measured cost, the
    companion design can add a same-connection snapshot command later
    without changing this design's state machine.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Security & Privacy**:
    *   The viewer connects only to the address it is given. It never
        listens on a port.
    *   It inherits the companion design's limitation: the side-channel is
        loopback-only and has no authentication. The viewer README must
        state this.
    *   The viewer writes no data to disk. The clipboard receives an IRI only
        when the user clicks "copy".
*   **Performance**:
    *   60 fps with 5,000 nodes on the author's machine.
    *   A layout step takes 4 ms or less at 5,000 nodes in a release build.
    *   An event becomes visible in under 1 s after the viewer receives it.
    *   A snapshot of 5,000 nodes is applied in under 500 ms.
*   **Build cost**: Depending on the `sparql-mcp` crate compiles Oxigraph and
    RocksDB into the viewer. This makes Windows builds noticeably longer and
    the binary larger. The author accepted this cost when choosing the shared
    types location. For local iteration, run `cargo run -p sparql-mcp-viewer
    --features bevy/dynamic_linking`. That flag is given on the command line
    only, so `--all-features` in CI never turns it on.
*   **Rollout & Rollback**:
    *   The viewer is a separate, opt-in binary, and the server channel is
        off by default.
    *   Rolling back means not shipping `sparql-mcp-viewer.exe`. The server
        behavior is unchanged unless `[viewer] enabled = true`.

--------------------------------------------------------------------------------

### Additional Resources

*   [`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md)
    is the companion design: server side-channel and wire protocol.
*   The companion design's review reports are `zoom_out_report.md`,
    `comprehension_report.md`, `critic_report.md`, and
    `goldfish_review_report.md`.
*   Bevy 0.20 release notes and the 0.19 → 0.20 migration guide:
    https://bevy.org/news/bevy-0-20/
*   Bevy Linux build dependencies: `docs/linux_dependencies.md` in the Bevy
    repository at the pinned tag.

### Future Improvements

*   **Layout persistence.** Save node positions per project, keyed by IRI,
    so that a restart does not re-settle the layout.
*   **Resnapshot on the same connection.** If reconnect-based resync shows a
    measured cost, add a `{"cmd":"snapshot"}`-at-any-time rule to the
    companion protocol.
*   **Custom render passes.** GPU instancing for nodes and compute-shader
    layout. These belong in a separate design, and only if the built-in
    renderer misses the performance target.
*   **Other platforms.** Native Linux and macOS builds. Today, removing the
    Windows-only packaging restriction is enough to allow them, but they are
    not tested.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Required changes to the companion design

Apply these to `docs/designs/interface-agentic-rust-stylee.md` before
Phase 1 starts. Phase 1 then implements them together with companion files
2–13.

1.  **Hand over ownership.** Companion file 1 (the `members` edit) and
    companion files 14 to 24 are now specified by this document. Replace them
    there with a pointer to this document. This covers the viewer crate, its
    README, the root README, the CHANGELOG, CI and release.
2.  **Term encoding.** Every term in a triple is a string in N-Triples
    syntax:
    *   `<iri>` for an IRI;
    *   `_:id` for a blank node;
    *   `"lex"`, `"lex"@lang` or `"lex"^^<dt>` for a literal.
3.  **Lag behavior.** When a connection's broadcast receiver returns
    `RecvError::Lagged`, the server closes that connection. No new message
    type is added. This replaces the companion's sentence "the viewer side
    detects it".
4.  **Wire types.** In `sparql_mcp::application::event_sink`:
    *   `StoreEvent` and `SubjectKind` derive `Serialize` and `Deserialize`.
    *   `StoreEvent` has `#[serde(tag = "event", rename_all = "lowercase")]`,
        and `SubjectKind` has `#[serde(rename_all = "lowercase")]`.
    *   A triple is `[String; 3]`: subject, predicate, object.
    *   A new type, `Snapshot { graph: String, triples: Vec<[String; 3]>, kinds: BTreeMap<String, SubjectKind> }`,
        is sent as `{"snapshot": { … }}`.

    A decoder tells line types apart by the top-level key: `"snapshot"` or
    `"event"`.
5.  **Active graph as server state.** The server keeps the active graph in
    shared mutable state (`Arc<RwLock<String>>`), set at startup from
    `--active-graph`.
    *   `project_switch` writes the new graph IRI to it.
    *   `spawn_viewer_channel` takes that shared state instead of a fixed
        `String`. It reads the state for every snapshot, and pushes a new
        snapshot to every connection when the value changes.
6.  **Shared class list.** `event_sink` exports
    `pub const CLASS_TYPE_IRIS: [&str; 5]`. `classify_subjects` and the
    viewer's `graph.rs` both use this constant.

### Delivery phases

"Companion file N" means a file in the companion design. "File N" means a
file in this document. Each phase ends when its exit criterion is met and
`cargo fmt`, `cargo clippy` and `cargo test` pass. The next phase starts only
after that.

| Phase | Files | Exit criterion |
|---|---|---|
| 0 | Spike only; nothing is committed | An empty Bevy 0.20 window opens on Windows, built from this repository. A line sent from one native Windows process to `127.0.0.1:7878` is received by another native Windows process listening on that port. If it does not, stop and return to drafting. |
| 1 | Companion files 2–13, plus Required changes 2–6 | The companion's `tests/viewer_channel.rs` passes: snapshot first, events in order, the connection closed on lag, and a new snapshot after `project_switch`. |
| 2 | Files 1, 2, 3, 4 (headless path), 5 (`CorePlugin`), 6, 7, 13, 14, 19, 20 | CUJ 4 passes against a real `sparql-mcp serve`. The unit tests of `graph` and `connection` pass, and so does `tests/headless_e2e.rs`. CI passes with the Bevy Linux packages. The release build is scoped with `-p`. |
| 3 | Files 4 (windowed path), 5 (`ViewerPlugin` with render, camera and layout), 8 (all-pairs layout and placement), 9 (reconciliation, materials, edges, base lights), 10 | The `smc:` ontology and twenty instances render at layout positions. Classes and instances are distinct. Rotate, zoom, pan and `F` work. |
| 4 | Files 8 (`octree.rs`, budget test), 9 (`Spawning` and `Despawning`) | CUJ 1 passes. `layout_budget_5000` passes in a release build. |
| 5 | Files 11, 12 | CUJ 2 and CUJ 3 pass. |
| 6 | Files 4 (`--diagnostics`), 9 (HDR, bloom, tonemapping, MSAA, fog), 15, 16, 17, 18 | 60 fps with 5,000 nodes, in a release build on the author's machine, driven by file 15. The Windows release archive contains `sparql-mcp-viewer.exe`. |

Before Phase 2, the CI workflow (file 19) installs the Bevy Linux
packages on the `ubuntu-latest` runner. Without them, the viewer crate does
not compile there. No local installation step is needed on the Windows
development machine.

### Target Files to Create/Modify

#### 1. `Cargo.toml` (root workspace) (Modify)

*   **Type / Scope**: Workspace manifest.
*   **Proposed Changes**:
    *   Add `crates/sparql-mcp-viewer` to `members`.
    *   Add `[profile.dev.package."*"] opt-level = 3`. This optimizes
        dependencies in dev builds, so Bevy runs fast enough to judge the
        frame rate.
    *   Cost: the first dev build of Oxigraph, RocksDB and rusqlite also
        takes longer, in CI as well. This is accepted because `rust-cache`
        caches the compiled dependencies.

#### 2. `crates/sparql-mcp-viewer/Cargo.toml` (Create)

*   **Type / Scope**: Crate manifest (library + binary).
*   **Proposed Changes**:
    *   **Package.** `name = "sparql-mcp-viewer"`, `version = "0.1.0"`, with
        `edition`, `license` and `repository` inherited from the workspace
        (`.workspace = true`). `[lib] path = "src/lib.rs"`, and
        `[[bin]] name = "sparql-mcp-viewer" path = "src/main.rs"`.
    *   **`rust-version`.** Copied from Bevy's `Cargo.toml` at the pinned
        version. The workspace value (1.78) is too old for Bevy.
    *   **Dependency specs reused from the core crate.** `sparql-mcp = { path = "../sparql-mcp-core" }`,
        `clap = { version = "4", features = ["derive"] }`, `serde = "1"`,
        `serde_json = "1"`, `anyhow = "1"`, `tracing = "0.1"`, and
        `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`.
    *   **Viewer-specific `tokio` features**, not a straight reuse of the
        core crate's list: `tokio = { version = "1", features = ["rt", "net", "io-util", "time", "macros", "sync"] }`
        (single-threaded `rt` instead of core's `rt-multi-thread`; adds
        `time` for the reconnect backoff and `sync` for the inbox channel;
        drops `process`, which the viewer does not need).
    *   **New dependencies.**
        *   `crossbeam-channel = "0.5"`.
        *   `arboard = "3"`.
        *   `rand = { version = "0.9", features = ["small_rng"] }`.
        *   `glam`: pinned to the version `cargo tree -i glam` reports after
            Bevy is added.
    *   **Bevy.** `bevy = { version = "=0.20.N", default-features = false, features = [...] }`.
        *   `N` is the latest 0.20 patch on crates.io on the day Phase 2
            starts. It is recorded in `Cargo.lock`.
        *   Features enable windowing (`bevy_winit` with `x11` and
            `wayland`), 3D rendering, PBR, the core pipeline with
            tonemapping LUTs, `bevy_ui`, text with the default font, picking
            with the mesh backend, `bevy_camera_controller`, and
            multithreading.
        *   Audio, glTF, animation and gamepad are left out.
        *   Feature names are copied from Bevy's 0.20 manifest.
    *   **No `dev` feature.** See the NFR "Build cost".

#### 3. `crates/sparql-mcp-viewer/src/lib.rs` (Create)

*   **Type / Scope**: Library root.
*   **Proposed Changes**:
    *   Declares `pub mod connection; pub mod graph; pub mod layout; pub mod plugin; pub mod headless;`
        and the Bevy modules `render`, `camera`, `interact` and `ingest`.
    *   Each Bevy module is added in its phase.
    *   Integration tests and `main.rs` use this library.

#### 4. `crates/sparql-mcp-viewer/src/main.rs` (Create)

*   **Type / Scope**: Thin binary.
*   **Proposed Changes**:
    *   **Flags** (clap):
        *   `--connect <ADDR>`;
        *   `--config <PATH>`;
        *   `--connect-timeout <SECS>`, default 10;
        *   `--headless`;
        *   `--diagnostics`, added in Phase 6.
    *   **Address resolution**, in this order:
        1.  `--connect`, if given;
        2.  `viewer.bind` from the config. If `--config` is not given, the
            viewer loads `sparql_mcp::config::DEFAULT_CONFIG_FILE` if that
            file exists. If `--config` names a file that does not exist,
            the viewer exits with an error;
        3.  `127.0.0.1:7878`.

        If the config has `viewer.enabled = false`, the viewer logs a
        warning and still connects.
    *   **`--headless`** calls `headless::run`. Phase 2 implements only this
        path.
    *   **Windowed path** (Phase 3):
        *   `DefaultPlugins`, with the window title "sparql-mcp viewer" and
            a clear color from a constant in `main.rs`;
        *   then `ViewerPlugin`.
    *   **`--diagnostics`** adds `FrameTimeDiagnosticsPlugin` and
        `LogDiagnosticsPlugin`.

#### 5. `crates/sparql-mcp-viewer/src/plugin.rs` (Create)

*   **Type / Scope**: Bevy wiring and the Bevy-side resources.
*   **Proposed Changes**:
    *   **`CorePlugin { addr, connect_timeout }`** (Phase 2).
        *   At build time, it calls `connection::spawn` and inserts:
            *   `Inbox(crossbeam_channel::Receiver<ViewerMsg>)`;
            *   `FrameMessages(Vec<ViewerMsg>)`;
            *   `ConnStatus(ConnState)`;
            *   `ConnectionHandle(ShutdownHandle)`, kept for the whole App
                lifetime;
            *   `ViewerGraph`.
        *   Systems:
            *   `drain_inbox` (set `Net`) moves every pending message into
                `FrameMessages`.
            *   `apply_to_graph` (set `Graph`) calls `ViewerGraph::apply_*`
                for each message and updates `ConnStatus`. If `LayoutState`
                exists, it also calls `place_new` and `remap`.
    *   **`ViewerPlugin`** (Phase 3) adds `CorePlugin`. Each later phase then
        adds its resources and sub-plugins here:
        *   Phase 3: `LayoutState`, `LayoutParams`, `ViewerStyle`, and the
            render and camera plugins;
        *   Phase 5: `Selection(Option<NodeId>)`, `ClassFilter`,
            `ActivityFeed`, and the interact and ingest plugins.
    *   **Schedule.**
        *   **`Update`**, with the sets chained in this order:
            *   `Net`: `drain_inbox`;
            *   `Graph`: `apply_to_graph`, then `record_feed`;
            *   `Sync`: `sync_entities`, then `apply_node_materials`;
            *   `Ui`: `update_panels`, then `update_banner`, then
                `position_labels`.

            Systems inside each set are chained in the listed order. Bevy
            never infers order from data access, so every required order is
            written out here.
        *   **`FixedUpdate`**: `layout_step_system`, with
            `Time::<Fixed>::from_hz(60.0)`.
        *   **`PostUpdate`**: `write_transforms`, then
            `animate_spawn_despawn`, then `update_edge_meshes`. These are
            chained and ordered with `.before(...)` the system set that
            `bevy_transform` registers for `GlobalTransform` propagation;
            look it up in the pinned Bevy version's `bevy_transform::systems`
            docs (`cargo doc -p bevy_transform --open`) rather than
            hardcoding a name that may not match the pinned patch version.

#### 6. `crates/sparql-mcp-viewer/src/connection.rs` (Create)

*   **Type / Scope**: Network client with no Bevy dependency.
*   **Proposed Changes**:
    *   **Types.**
        *   `enum ViewerMsg { Snapshot(Snapshot), Event(StoreEvent), State(ConnState) }`.
        *   `enum ConnState { Connecting, AwaitSnapshot, Streaming, Backoff { retry_at: Instant } }`.
        *   `struct ShutdownHandle`, which wraps a
            `tokio::sync::oneshot::Sender`. Dropping it stops the runtime.
    *   **`spawn(addr, connect_timeout) -> (Receiver<ViewerMsg>, ShutdownHandle)`.**
        It starts a `std::thread`, which builds a tokio `current_thread`
        runtime and runs the state machine from the Technical Plan.
    *   **Decoding.** A buffered line reader feeds `serde_json`, using the
        companion's wire types (Required change 4).
        *   A malformed line is logged at `warn` and skipped.
        *   An unknown `event` value is logged and skipped.
    *   **Timers and reconnects.**
        *   `AwaitSnapshot` times out after 30 s and goes to `Backoff`.
        *   A `changed` event is forwarded, then the connection is closed
            and reopened at once, without backoff.
        *   A socket error goes to `Backoff`, with delays of 1, 2, 4 … up to
            30 s.
        *   The delay resets to 1 s on reaching `Streaming`.
    *   **Unit tests** (against a `TcpListener` inside the test):
        *   decoding of a snapshot and of each event kind;
        *   a malformed line is skipped;
        *   the backoff sequence is 1, 2, 4, 8, 16, 30, 30;
        *   a `changed` event reconnects without backoff.

#### 7. `crates/sparql-mcp-viewer/src/graph.rs` (Create)

*   **Type / Scope**: RDF-to-graph projection, plain Rust.
*   **Proposed Changes**:
    *   **Types.**
        *   `NodeId(u32)`.
        *   Reuse `sparql_mcp::domain::RdfTerm` (already `Iri`, `BlankNode`,
            and `Literal { value, datatype, lang }`) instead of a
            hand-written `Term` enum and parser; the viewer already depends
            on the core crate for `StoreEvent`, `SubjectKind` and
            `CLASS_TYPE_IRIS`.
        *   `Node { term, kind, label, attributes: BTreeMap<String, BTreeSet<String>>, types, mentions: u32, last_touched }`.
        *   `enum EdgeStyle { Type, SubClass, Relation }`.
        *   `Edge { from, to, predicate, style }`.
        *   `ViewerGraph`, with these fields:
            *   `graph: String`;
            *   `triples: HashSet<[String; 3]>`;
            *   `nodes: Vec<Option<Node>>`, where removed slots stay `None`
                and an id is never reused within one `ViewerGraph`;
            *   `index: HashMap<String, NodeId>`;
            *   `edges`;
            *   `generation: u64`;
            *   `event_seq: u64`.
    *   **`from_snapshot(&Snapshot, previous: Option<&ViewerGraph>) -> (ViewerGraph, Option<Remap>)`.**
        Ids are always fresh. `Remap` (old id → new id, for IRI nodes only)
        is returned only when `previous.graph == snapshot.graph`.
    *   **`apply_event(&mut self, &StoreEvent) -> Applied { added, removed, touched, resync, ignored }`.**
        *   An event for another graph sets `ignored` and changes nothing.
        *   Duplicate inserts and absent deletes are no-ops.
        *   `mentions` is updated, and a node is removed at 0.
        *   `changed` sets `resync`.
        *   `generation` is incremented only if something changed.
    *   **Rules.**
        *   Class detection uses `sparql_mcp::application::event_sink::CLASS_TYPE_IRIS`.
        *   Labels and sticky classification follow the Technical Plan.
        *   The `smc:` namespace is `https://sparql-mcp.dev/ns#`
            (`ontology/1-smc.ttl`).
    *   **Unit tests.** Each of these behaviors has one test:
        *   literals become attributes;
        *   edge style per predicate;
        *   classification by each of the 5 IRIs, and from `kinds`;
        *   a class stays a class across events;
        *   a duplicate insert is a no-op;
        *   delete removes orphan nodes;
        *   an event for another graph is ignored;
        *   label fallback order;
        *   `generation` changes only on change;
        *   a same-graph snapshot keeps the positions of IRI nodes through
            `Remap`, and drops blank nodes;
        *   a different-graph snapshot returns no `Remap`;
        *   `changed` sets `resync`.

#### 8. `crates/sparql-mcp-viewer/src/layout/mod.rs` and `src/layout/octree.rs` (Create)

*   **Type / Scope**: Force simulation, plain Rust plus `glam`.
*   **Proposed Changes**:
    *   **`LayoutParams`.** Fields: `repulsion`, `spring`,
        `type_edge_stiffness`, `rest_length`, `damping`, `max_speed`,
        `sleep_energy`, and `theta` (default 0.8). The other defaults are
        tuned in Phase 4.
    *   **`LayoutState { pos: Vec<Vec3>, vel: Vec<Vec3>, awake: bool, rng: SmallRng }`.**
        It has the same length as `ViewerGraph.nodes`, and dead slots are
        skipped.
    *   **Functions.**
        *   `place_new(&mut self, graph, ids)` applies the placement rule
            from the Technical Plan and sets `awake = true`.
        *   `remap(&mut self, &Remap)` carries positions over to the new
            ids.
        *   `step(&mut self, graph, dt)`:
            1.  builds the octree (`octree.rs`, Phase 4; all-pairs in
                Phase 3);
            2.  applies repulsion, springs, damping and the speed cap;
            3.  sets `awake = false` when total kinetic energy is below
                `sleep_energy`.
    *   **No Bevy here.** The Bevy wrapper `layout_step_system` lives in
        file 9.
    *   **Unit tests.**
        *   Two linked nodes converge toward `rest_length`.
        *   A new node is placed near its neighbor.
        *   A node with no neighbor is placed near the centroid.
        *   `place_new` does not move existing positions.
        *   The layout falls asleep, then wakes on `place_new`.
        *   Positions are deterministic with a fixed seed.
        *   Barnes-Hut forces stay within 5% of all-pairs forces on a
            200-node graph.
    *   **Budget test.** `#[ignore] layout_budget_5000`:
        *   a seeded generator (seed 42) builds 50 classes, 4,950
            instances and 10,000 edges;
        *   the test runs 100 steps and asserts a mean step time of 4 ms or
            less;
        *   it is run with `--release`.

#### 9. `crates/sparql-mcp-viewer/src/render.rs` (Create)

*   **Type / Scope**: Bevy rendering.
*   **Proposed Changes**:
    *   **`ViewerStyle`.** Colors, radii, emissive strength, `spawn_secs`
        (0.3), `pulse_secs` (2.0), and pulse scale (1.3).
    *   **`StyleHandles`.**
        *   One sphere mesh per node kind.
        *   One material each for class, instance, selected, faded and
            last-batch.
        *   One `LineList` mesh per edge style, each with an unlit
            alpha-blended material and `Pickable::IGNORE`.
    *   **`setup_scene`.**
        *   Phase 3: the camera with `PanOrbitCamera`, one directional
            light and ambient light.
        *   Phase 6 adds HDR, `Bloom`, `Tonemapping::TonyMcMapface`, MSAA 4×
            and `DistanceFog`.
    *   **Systems.**
        *   `layout_step_system` calls `LayoutState::step`.
        *   `sync_entities` runs only when `generation` changed. It spawns
            `NodeRef(NodeId)` entities with `Spawning`, and adds
            `Despawning` to removed ones.
        *   `apply_node_materials` picks one of the five shared materials
            from the selection, the filter and the last batch.
        *   `write_transforms`.
        *   `animate_spawn_despawn` scales nodes for arrival and departure,
            and applies the sine pulse.
        *   `update_edge_meshes` runs only while `awake`.
    *   **Not here.** `apply_to_graph` is not defined here; it lives in
        file 5.

#### 10. `crates/sparql-mcp-viewer/src/camera.rs` (Create)

*   **Type / Scope**: Camera setup.
*   **Proposed Changes**:
    *   Adds `DefaultPanOrbitCameraPlugins` and configures `PanOrbitCamera` on
        the scene camera.
    *   The `F` key sets the focus and radius from the bounding sphere of
        `LayoutState`.
    *   The controller is disabled while any UI node reports a hovered
        `Interaction`.
    *   No system here reads `ViewerGraph`.

#### 11. `crates/sparql-mcp-viewer/src/interact.rs` (Create)

*   **Type / Scope**: Picking, selection, filters and labels (`bevy_ui`).
*   **Proposed Changes**:
    *   **Selection.** Adds `MeshPickingPlugin`. A `Pointer<Click>` observer
        on `NodeRef` entities sets `Selection(Option<NodeId>)`.
    *   **Selection panel.** It shows:
        *   the label;
        *   the IRI, with a "copy" button (`arboard`);
        *   the attributes grouped by predicate;
        *   incoming and outgoing edges. Clicking an edge target selects
            it.
    *   **`ClassFilter`.**
        *   The visible set is the union of the selected classes, and every
            class is selected at start.
        *   Class nodes are never filtered out.
        *   An instance is visible if at least one of its `types` is
            selected.
        *   Untyped instances follow an "untyped" toggle, which is on at
            start.
        *   Filtered-out nodes use the faded material and keep their
            positions.
    *   **Labels.** All labels are owned here: class labels, the
        selected-node label and the hover label. They are placed by
        `position_labels` with `world_to_viewport`.

#### 12. `crates/sparql-mcp-viewer/src/ingest.rs` (Create)

*   **Type / Scope**: Ingestion view (`bevy_ui`).
*   **Proposed Changes**:
    *   **`ActivityFeed`.** A ring buffer of 50 entries
        `{ received_at: Instant, kind, triple_count }`. `kind` is one of
        `snapshot`, `insert`, `delete`, `changed`, `resync` or
        `other graph`.
    *   **`record_feed`.** It reads `FrameMessages` after `apply_to_graph`.
    *   **Last-batch pulse.** It applies for `pulse_secs` to nodes whose
        `last_touched == event_seq`. Snapshots do not advance `event_seq`,
        so a rebuild does not pulse the whole graph.
    *   **Resync banner.** Shown while `ConnStatus` is not `Streaming`, with
        the state and the seconds until `retry_at`.

#### 13. `crates/sparql-mcp-viewer/src/headless.rs` (Create)

*   **Type / Scope**: Windowless mode.
*   **Proposed Changes**:
    *   **`run(addr, connect_timeout) -> ExitCode`** builds an App with
        `MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 30.0)))`
        and `CorePlugin`.
    *   A system prints `<kind> nodes=<n> edges=<m>` after each applied
        message.
    *   If `ConnStatus` has not reached `Streaming` within
        `connect_timeout`, the App exits with `AppExit::from_code(2)`.

#### 14. `crates/sparql-mcp-viewer/tests/headless_e2e.rs` (Create)

*   **Type / Scope**: Integration test (Phase 2; needs Phase 1).
*   **Proposed Changes**:
    *   **Setup.**
        *   The store is `OxigraphAdapter::open_in_memory()`.
        *   A `BroadcastEventSink` is shared with `spawn_viewer_channel`,
            which listens on `127.0.0.1:0`.
        *   `ontology::load_ontology` and `sparql::update_sparql` are called
            directly, with `(&Arc<dyn SparqlStore>, &JsonObject, &dyn EventSink)`.
            Every call targets the active graph through `graph_iri` or a
            `GRAPH` clause.
        *   The client side is `connection::spawn` plus a `ViewerGraph`,
            with no Bevy.
    *   **Steps.**
        1.  Load `ontology/1-smc.ttl`. Expect `smc:Project` as a class node,
            and its five properties as class-kind nodes.
        2.  `INSERT DATA` three `smc:Project` instances. Expect 3 more
            instance nodes and 3 `Type` edges.
        3.  `DELETE WHERE` one instance. Expect `ViewerMsg::State` to go
            through `Connecting`, then `AwaitSnapshot`, then `Streaming`,
            and expect 2 instance nodes after the rebuild.

#### 15. `crates/sparql-mcp-core/examples/viewer_load.rs` (Create)

*   **Type / Scope**: Load generator for the Phase 6 check.
*   **Proposed Changes**:
    *   `cargo run --release -p sparql-mcp --example viewer_load -- --bind <ADDR>`.
    *   It opens `OxigraphAdapter::open_in_memory()`, starts
        `spawn_viewer_channel`, and loads `ontology/1-smc.ttl` into the
        active graph.
    *   It then inserts 5,000 typed instances with `INSERT DATA`, in batches
        of 100, one batch every 50 ms, and keeps serving until it receives
        Ctrl-C.
    *   It needs no agent and does not touch the RocksDB store.

#### 16. `crates/sparql-mcp-viewer/README.md` (Create)

*   **Type / Scope**: Crate documentation.
*   **Proposed Changes**: The README covers:
    *   what the viewer shows;
    *   how to enable `[viewer]`;
    *   that the Bevy Linux packages (file 19) are needed only by the CI
        Linux job, not for a local Windows build;
    *   how to build on Windows with a Windows `rustup` toolchain;
    *   the run flags and `--headless`;
    *   the controls;
    *   the loopback-only, unauthenticated limitation;
    *   the GPU model used for the 60 fps measurement.

#### 17. `README.md` (root) (Modify)

*   **Type / Scope**: Project overview.
*   **Proposed Changes**:
    *   Add `crates/sparql-mcp-viewer/` to the repository tree.
    *   Add a short "Live 3D viewer (optional)" section that links to the
        crate README.

#### 18. `CHANGELOG.md` (Modify)

*   **Type / Scope**: Changelog.
*   **Proposed Changes**: Under "Unreleased → Added", add the
    `sparql-mcp-viewer` crate, following the existing entry style.

#### 19. `.github/workflows/ci.yml` (Modify)

*   **Type / Scope**: CI (Phase 2).
*   **Proposed Changes**:
    *   Before `cargo clippy`, add a step that runs `sudo apt-get update &&
        sudo apt-get install -y pkg-config libudev-dev libwayland-dev
        libxkbcommon-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev
        libvulkan-dev`.
    *   The list is checked against Bevy's `linux_dependencies.md` at the
        pinned tag. ALSA is left out because audio is disabled.
    *   The fmt, clippy and test steps stay unchanged.

#### 20. `.github/workflows/release.yml` (Modify)

*   **Type / Scope**: Release workflow (Phase 2).
*   **Proposed Changes**:
    *   Change the Build step to
        `cargo build --release --target ${{ matrix.target }} -p sparql-mcp`.
        Today the step builds the whole workspace. With this change, the
        Linux targets (and the macOS targets, if re-enabled) never build
        Bevy.
    *   Add a step `Build viewer` with `if: matrix.os == 'windows'` that
        runs `cargo build --release --target ${{ matrix.target }} -p
        sparql-mcp-viewer`.
    *   In the Package step's Windows branch, also copy
        `sparql-mcp-viewer.exe` into the archive.

### Verification Plan

*   **Compilation/Build**:
    *   Local dev: `cargo build --workspace` on Windows. No WSL and no Bevy
        Linux apt packages are needed for this step.
    *   CI only: the hosted `ubuntu-latest` runner installs the Bevy Linux
        packages from file 19 before `cargo build --workspace` (needed
        because that runner is Linux, not because of any local WSL step).
    *   Release: `cargo build --release -p sparql-mcp-viewer --target
        x86_64-pc-windows-msvc`.
*   **Unit Tests**: `cargo test --all --all-features`. This covers the
    `graph`, `layout` and `connection` unit tests, which need no window.
*   **E2E/Integration**:
    *   `tests/headless_e2e.rs` runs in CI.
    *   CUJ 4 is run by hand on Windows against a locally running
    `sparql-mcp serve`.
    *   CUJs 1 to 3 are checked by hand on Windows at the end of their phase.
        They require visual inspection and cannot be scripted.
*   **Performance**:
    *   Layout budget: `cargo test --release -p sparql-mcp-viewer --
        --ignored layout_budget_5000`.
    *   Frame rate: run `viewer_load` (file 15), then
        `sparql-mcp-viewer --diagnostics` in a release build on the author's
        machine. The frame time must stay at or below 16.7 ms once all
        5,000 nodes are placed.
*   **Static Analysis & Linting**: `cargo fmt --all -- --check` and
    `cargo clippy --all-targets --all-features -- -D warnings`.
*   **Test Coverage Requirement**: Every projection, layout and connection
    rule in the Technical Plan has at least one unit test (files 6, 7 and
    8). No percentage target is set.

--------------------------------------------------------------------------------

*Drafted with EGM*
