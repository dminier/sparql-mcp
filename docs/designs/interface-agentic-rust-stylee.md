# Interface Agentic Rust Stylée

*   **Author:** dminier
*   **Status:** Draft
*   **Date:** 2026-11-15

--------------------------------------------------------------------------------

## Overview

`sparql-mcp` is a Rust MCP server that gives coding agents a persistent
semantic store (Oxigraph, SPARQL 1.1) plus a Claude Code skill
(`kb-workbench`) that turns that store into Obsidian notes and a 2D canvas.
That export is useful as a durable, human-readable record, but it is a
batch step: the agent finishes work, the skill regenerates the canvas, the
user opens Obsidian to look at it. There is no way to watch the knowledge
base while an agent is actually populating it.

This project, **Interface Agentic Rust Stylée**, adds a second, live way to
look at the same store: a native, visually polished 3D graph viewer that
shows ontology classes and their instances, and updates node-by-node as the
agent writes new triples — instead of waiting for a full export-and-reload
cycle.

## The Problem

Today, watching an agent build up a knowledge base means either reading raw
SPARQL query results or waiting for `kb-workbench` to regenerate an Obsidian
canvas after the fact. Neither gives a sense of the graph's shape while it
is forming: which classes are in play, how densely instances connect to
each other, where the ontology is thin or contradicted. For a tool whose
whole pitch is "SPARQL is the source of truth," the only way to *see* that
source of truth today is static and 2D.

The proposed solution is a dedicated Rust application that connects to the
same Oxigraph store `sparql-mcp` already maintains, lays the ontology and
its instances out in 3D, and appends to that layout as new triples land —
giving the user a live view of the agent's work instead of an after-the-fact
snapshot.

## Goals

*   Render the ontology (classes, properties) and their instances as a 3D
    node-link graph, visually distinguishing schema-level nodes (classes,
    predicates) from instance-level nodes (actual data).
*   Update the rendered graph incrementally as the underlying store changes
    — new nodes and edges appear as they are written, without a full
    reload or a layout reshuffle of existing nodes.
*   Source ontology and instance data directly from `sparql-mcp`'s existing
    SPARQL store (via `query_sparql` / the Oxigraph store), rather than
    building a second, parallel data layer.
*   Deliver a graphically polished result — deliberate lighting, motion, and
    layout, not a bare wireframe debug view — since the explicit ask is a
    "beau graphisme" interface, not a diagnostic tool.
*   Ship as a Rust codebase, consistent with `sparql-mcp`'s existing stack
    and contributor base.
*   Position the result as an improvement to `github.com/dminier/sparql-mcp`
    — it must be clear how this piece attaches to the existing project —
    see the Technical Plan, which fixes this as a new in-workspace crate —
    not a disconnected rewrite.

## Non-Goals

*   Not a replacement for `kb-workbench`'s Obsidian export. The canvas
    stays the durable, human-editable record; this viewer is a live,
    read-only window onto the same data.
*   Not a general-purpose SPARQL query editor or triple-store admin tool.
*   Not a multi-user or server-hosted viewer — scoped to one operator
    watching their own local store.
*   Not committing, in v1, to supporting every OS/GUI combination. The
    target environment is a native Windows machine, running both
    `sparql-mcp serve` and the viewer; broader platform support (e.g. a
    native Linux build, same architecture) is a future improvement, not a
    v1 requirement.

## Critical User Journeys (CUJs)

### CUJ 1: Watching an agent populate the graph live

*   **Given**: The viewer is running and connected to the project's
    `sparql-mcp` store, showing the graph as it currently stands (possibly
    empty).
*   **When**: A coding agent, in a separate Claude Code session, calls
    `load_ontology` and then runs several `update_sparql` inserts while
    working on a task.
*   **Then**: Within roughly a second of the commit for a single-triple
    insert (exact bound not yet measured; batch loads of many triples are
    not held to this target), the new ontology classes and instances appear
    in the 3D view, connected to their related nodes, without the camera
    resetting or existing nodes jumping to new positions.

### CUJ 2: Exploring the ontology shape after a session

*   **Given**: A project has an existing store with a non-trivial ontology
    and hundreds of instances.
*   **When**: The user opens the viewer against that project.
*   **Then**: The user can visually distinguish the small set of ontology
    classes from the larger mass of instances, and can navigate (rotate,
    zoom, pan) the 3D scene to inspect dense or sparse regions of the graph.

### CUJ 3: Viewer restarts mid-session

*   **Given**: The viewer was connected and showing a populated graph.
*   **When**: The viewer process is closed and reopened (or crashes and is
    relaunched) while the agent keeps writing to the store.
*   **Then**: On reconnect, the viewer requests a fresh snapshot, rebuilds
    its in-memory graph from scratch, and re-settles the layout — accepting
    a one-time visual reset as the cost of a stateless client, rather than
    attempting to resume from where it left off.

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

Three architectural decisions were made by the author (see "Alternatives
Considered" for the rejected options):

1.  The viewer is a **native Windows executable** (`sparql-mcp-viewer.exe`),
    built for `x86_64-pc-windows-msvc` the same way `release.yml` already
    builds `sparql-mcp.exe` (natively on Windows, no WSL-side
    cross-compilation), rendering with a GPU-based 3D engine (Bevy, which
    wraps `wgpu`) — chosen over relying on WSLg (unverified on this machine)
    or a browser-based renderer.
2.  The viewer's source lives as a **new crate inside the existing
    `sparql-mcp` Cargo workspace**: `crates/sparql-mcp-viewer`.
3.  `sparql-mcp-core` gains a **push side-channel**: every committed write
    (`update_sparql`, `load_ontology`, `load_ontology_file`) also emits an
    event describing what changed, instead of the viewer polling.

`sparql-mcp`'s MCP server (the STDIO process Claude Code spawns) and the
viewer both run as **native Windows processes on the same machine.** They
are different processes, but the same OS — there is no cross-OS boundary.
Putting the viewer's code in the same Cargo workspace buys *compile-time*
sharing — the viewer reuses `sparql-mcp-core`'s domain types (`smc:Project`,
triple/ontology representations) and serialization code so the two sides
agree on wire formats by construction — but it does **not** mean the viewer
opens the same RocksDB store, or shares a live Oxigraph handle, or runs in
the same process. All data crosses the process boundary over the TCP
side-channel described below (a same-machine loopback connection); there is
no direct filesystem or in-process access from the viewer binary into the
server's store.

### Data flow

```text
┌───────────────────── Windows (native, one machine) ────────────────┐
│  Claude Code agent                                                  │
│       │ stdio (MCP: query_sparql, update_sparql, load_ontology, …) │
│       ▼                                                             │
│  sparql-mcp-core  (existing STDIO MCP server, extended)             │
│       │                              │                              │
│       ▼                              ▼                              │
│  Oxigraph / RocksDB store      viewer side-channel (new)            │
│  (unchanged)                   TCP listener, 127.0.0.1:<port>       │
│                                 - on connect: full-graph snapshot    │
│                                 - thereafter: one JSON event per    │
│                                   committed write                   │
│                                         │                            │
│                                         │ TCP over 127.0.0.1          │
│                                         │ (same machine, same OS)    │
│                                         ▼                            │
│  sparql-mcp-viewer.exe  (new crate: crates/sparql-mcp-viewer)       │
│       - connects to 127.0.0.1:<port>, requests snapshot             │
│       - applies incoming insert/delete events to its in-memory graph│
│       - runs a continuous force-directed layout so existing nodes   │
│         hold position while new ones settle in                     │
│       - renders via Bevy (wgpu): classes vs instances styled        │
│         distinctly, bloom/lighting, orbit camera                    │
└─────────────────────────────────────────────────────────────────────┘
```

This is a same-machine, same-OS TCP connection over the loopback interface
— there is no port-forwarding or cross-OS networking layer involved, and
no environment-dependent fallback to decide. A different local bind
address remains configurable (`[viewer] bind`) for a user's own preference,
not as a forwarding-failure fallback.

### `sparql-mcp-core` changes (side-channel)

*   New config section in `sparql-mcp.toml`:
    `[viewer] enabled = false` (default off — a user who never installs the
    viewer pays no extra cost; today's "nothing extra running" posture is
    preserved) and `bind = "127.0.0.1:7878"` when enabled.
*   When enabled, the server binds a loopback-only TCP listener alongside its
    existing STDIO loop (same process, one additional accept loop).
*   Wire protocol: newline-delimited JSON.
    *   Client → server, on connect: `{"cmd":"snapshot"}`.
    *   Server → client, reply: one JSON object containing every triple
        currently in the server's single active project graph (`active_graph`,
        as set by `project_switch`), with each subject pre-classified as
        `"class"` (its `rdf:type` is `owl:Class` / `rdfs:Class` /
        `rdf:Property`) or `"instance"` (anything else) so the viewer does not
        have to re-derive that distinction.
    *   Server → client, ongoing: `{"event":"insert"|"delete"|"changed", "graph":"<urn:project:…>", …}`
        emitted once per committed `update_sparql` / `load_ontology` /
        `load_ontology_file` call, in commit order. `"changed"` is the coarse
        fallback used for pattern-based `update_sparql` calls (see the
        Detailed Implementation Plan).
    *   To avoid losing a write that commits during snapshot generation, the
        server subscribes to the event broadcast channel *before* running
        the snapshot query, buffers any events that arrive while the query
        executes, and replays that buffer to the client immediately after
        sending the snapshot — so the write's commit order, not accept-loop
        sequencing, is the ordering authority.
    *   Event emission never blocks the write path: the application layer
        pushes to a bounded `tokio::sync::broadcast` channel and returns
        immediately regardless of whether a viewer is connected or keeping
        up. A receiver that falls behind (`RecvError::Lagged`) is expected,
        not an error condition — the viewer side detects it and requests a
        fresh snapshot to resynchronize rather than continuing from a gap.
    *   If the agent calls `project_switch` while a viewer is connected, the
        server emits a fresh snapshot for the new active graph and the
        viewer discards its prior graph state — switching projects is
        treated as a new session from the viewer's perspective, not a diff.
*   This is additive to the existing MCP tool surface — no existing tool's
    request/response shape changes.
*   If the configured bind address is already in use, the server logs a
    warning and continues without the side-channel rather than failing to
    start — the viewer is additive, so a port conflict must never block the
    core MCP server.

### `crates/sparql-mcp-viewer` (new crate)

*   Binary crate, workspace member, built for `x86_64-pc-windows-msvc`
    natively on Windows (or on the `windows-latest` CI runner) — see the
    Detailed Implementation Plan for why this is preferred over WSL-side
    cross-compilation.
*   Startup: reads `sparql-mcp.toml` for the viewer's target
    `bind` address (shared config file, read-only from the viewer's side),
    connects over TCP, requests the snapshot, then stays connected.
*   In-memory model: a graph of `NodeId -> {kind: Class|Instance, label,
    position}` plus edges, kept in sync by applying `insert`/`delete` events
    to the same structure the snapshot populated.
*   Layout: an incremental force-directed simulation (repulsion between all
    nodes, attraction along edges, run at a fixed tick rate — exact rate to
    be tuned during implementation) so
    existing nodes keep their position when new nodes arrive — new nodes
    spawn near their first connected neighbor and settle via the simulation,
    rather than the whole graph re-laying out on every change.
*   Rendering: Bevy (chosen over hand-rolled `wgpu`/`winit` — see
    Alternative F in "Alternatives Considered" for the trade-off). Classes
    render as larger,
    emissive-material nodes; instances as smaller standard-shaded nodes;
    edges as translucent lines/tubes; new-node arrival is a brief scale/fade
    animation rather than an instant pop-in, so incremental growth is visible
    rather than just "correct."

## Alternatives Considered

### Alternative A: Run the viewer inside WSL via WSLg

*   **Pros**: single Linux build target, no cross-compilation, viewer and
    MCP server could even share a filesystem path trivially.
*   **Cons**: depends on WSLg being enabled and functional on this specific
    machine, which was unconfirmed at decision time; a native Windows
    executable has no such dependency.
*   **Reason for Rejection**: Rejected by author — avoids making the whole
    project's "can I even see a window" status depend on an unverified
    environment feature.

### Alternative B: Browser-based 3D view (Rust backend + three.js/Babylon.js frontend)

*   **Pros**: most mature 3D web tooling, no WSLg dependency, easiest to
    make visually polished quickly.
*   **Cons**: the rendering layer is JS/WASM, not native Rust GUI — conflicts
    with the ask for a Rust-styled interface.
*   **Reason for Rejection**: Rejected by author — the project's purpose is
    explicitly a Rust-native interface.

### Alternative C: Separate repository instead of a workspace crate

*   **Pros**: cleaner branding/release separation; no coupling to
    `sparql-mcp`'s release cadence.
*   **Cons**: would duplicate domain type and serialization definitions, or
    require publishing `sparql-mcp-core` as a versioned dependency up front.
*   **Reason for Rejection**: Rejected by author — in-workspace crate chosen
    for shared types and a single build/release pipeline, even though (per
    the Technical Plan note above) it does not grant in-process data access
    across the server/viewer process boundary.

### Alternative D: Polling a read-only secondary RocksDB handle instead of a push side-channel

*   **Pros**: no changes required to `sparql-mcp-core`'s write path at all.
*   **Cons**: adds polling latency; depends on the pinned Oxigraph/RocksDB
    version actually supporting secondary-instance (read-replica) opens,
    which was not verified; and a secondary handle would still need to live
    in the server process and relay over the same TCP channel anyway, since
    the viewer process cannot open the server's RocksDB directory directly.
*   **Reason for Rejection**: Rejected by author — the cross-process
    constraint means a channel of some kind is needed regardless, so a push
    channel was preferred over polling for lower latency, at the cost of
    touching `sparql-mcp-core`'s write path.

### Alternative E: Merge viewer and MCP server into one long-running process

*   **Pros**: simplest possible data path — in-memory push, no serialization
    across a socket.
*   **Cons**: now that both run on the same OS this is technically
    possible, but it would couple the STDIO MCP server's lifetime (spawned
    per Claude Code session) to the Bevy windowed app's lifetime — whichever
    outlives the other would have to be killed or restarted alongside it —
    and it would drop `sparql-mcp`'s current "spawned per session, no
    daemon" model, which the project's own README treats as a selling
    point.
*   **Reason for Rejection**: Rejected by author — would regress an
    existing, stated design goal of `sparql-mcp` (no daemon), independent
    of which OS either side runs on.

### Alternative F: Hand-rolled `wgpu`/`winit` renderer (no engine)

*   **Pros**: no ECS/asset-pipeline overhead; smaller dependency tree and
    faster compiles than Bevy.
*   **Cons**: camera controls, lighting, bloom, and mesh/material management
    all have to be built from scratch instead of reused.
*   **Reason for Rejection**: Rejected by author — the time spent
    reimplementing Bevy's built-in scene/camera/lighting plumbing is better
    spent on graph logic and visual polish, and Bevy's dependency weight is
    an acceptable cost for a desktop-only, infrequently-rebuilt binary.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Security & Privacy**: the side-channel listens on `127.0.0.1` only
    (never `0.0.0.0`), so it is reachable only from processes on the same
    machine — including other local users on a shared machine.
    It carries the full contents of the active project graph with no
    authentication. This is acceptable for a single-operator local tool but
    must be documented as a known limitation in the new crate's README.
*   **Performance**: the force-directed layout in `layout.rs` is an
    all-pairs repulsion (O(n²) per tick), acceptable for the hundreds-to-low-
    thousands of nodes a single project's ontology + instances are expected
    to reach. Scaling beyond that is listed under Future Improvements, not
    solved here.
*   **Rollout**: the `[viewer]` section defaults to `enabled = false`, so
    existing `sparql-mcp` installs are unaffected until a user opts in; no
    migration or backward-compatibility concern for existing configs.

--------------------------------------------------------------------------------

## Future Improvements

*   **Spatial partitioning for layout**: replace the O(n²) repulsion pass
    with a Barnes-Hut/octree approximation if real projects exceed a few
    thousand instances and the simulation can no longer complete within a
    single frame's time budget (exact threshold to be determined during
    implementation).
*   **Authenticated side-channel**: add a shared-secret or token check on
    the TCP listener if the viewer is ever used on a machine with untrusted
    local users.
*   **Precise triples for pattern-based updates**: `update_sparql` calls
    using `INSERT/DELETE … WHERE` currently only produce a coarse
    "something changed" event (see implementation notes below); a future
    pass could have the domain layer report the actual bound triples instead
    of only before/after counts.
*   **Cross-platform display**: ship a second native binary built for
    Linux (same crate, same architecture, a different target triple) if
    other users want this on Linux desktops — not a WSLg or browser-based
    detour. Both the server and the viewer would still need to run on the
    same OS as each other, exactly as the Windows-only setup requires today.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

Grounded in the actual current code: `sparql-mcp-core` is a hexagonal
Rust crate (`domain` = traits/types, `infrastructure` = adapters,
`application::tools` = one module per MCP tool, `mcp::server` = the
`rmcp`-based dispatcher). All writes going through `update_sparql`,
`load_ontology`, and `load_ontology_file` already funnel through
`application::tools::sparql` and `application::tools::ontology` — those two
files are the natural hook point for emitting viewer events, so the change
touches the application layer, not the `SparqlStore` domain trait or the
`OxigraphAdapter` infrastructure code. The existing `release.yml` already
proves native Windows builds work (`x86_64-pc-windows-msvc` on
`windows-latest`), so the implementation plan uses that path rather than
introducing WSL-side cross-compilation tooling.

### Target Files to Create/Modify

#### 1. `Cargo.toml` (root workspace) (Modify)

*   **Type / Scope**: Cargo workspace manifest.
*   **Proposed Changes**: Add `crates/sparql-mcp-viewer` to
    `[workspace] members`.

#### 2. `sparql-mcp.toml` (Modify)

*   **Type / Scope**: Sample/runtime TOML config.
*   **Proposed Changes**: Add a `[viewer]` section documented like the
    existing `[gdrive]`/`[plugins]` sections: `enabled = false`,
    `bind = "127.0.0.1:7878"`.

#### 3. `crates/sparql-mcp-core/Cargo.toml` (Modify)

*   **Type / Scope**: Rust crate manifest.
*   **Proposed Changes**: Add `"net"` and `"sync"` to the existing `tokio`
    feature list (needed for the TCP listener and the broadcast channel —
    `"macros"`, `"rt-multi-thread"`, `"process"`, `"io-util"` are already
    present).

#### 4. `crates/sparql-mcp-core/src/config.rs` (Modify)

*   **Type / Scope**: Config model + TOML deserialization.
*   **Proposed Changes**: Add `ViewerConfig { enabled: bool, bind: String }`
    (`Default` → `enabled = false`, `bind = "127.0.0.1:7878"`), and a new
    `pub viewer: Option<ViewerConfig>` field on `Config`, alongside the
    existing `pub gdrive: Option<GDriveConfig>` field. Add a unit test
    mirroring the existing `[gdrive]` parsing test (around line 208) for a
    `[viewer]\nenabled = true\n` fragment.

#### 5. `crates/sparql-mcp-core/src/application/event_sink.rs` (Create)

*   **Type / Scope**: New application-layer module.
*   **Proposed Changes**: Define:
    *   `SubjectKind { Class, Instance }` and a `classify_subjects(triples)
        -> HashMap<String, SubjectKind>` helper (a subject is `Class` if any
        triple in the batch types it as `owl:Class` / `rdfs:Class` /
        `rdf:Property` / `owl:ObjectProperty` / `owl:DatatypeProperty`,
        otherwise `Instance`) — one shared implementation so the snapshot
        path and the event path never classify differently.
    *   `StoreEvent { Insert { graph, triples, kinds }, Delete { graph,
        triples, kinds }, Changed { graph, triples_before, triples_after } }`
        (serde-serializable) — `Changed` is the coarse fallback used when
        the exact triples touched by an update aren't cheaply knowable (see
        file 7 below).
    *   `trait EventSink: Send + Sync { fn emit(&self, event: StoreEvent); }`
    *   `BroadcastEventSink(tokio::sync::broadcast::Sender<StoreEvent>)` and
        `NullEventSink` (no-op), both implementing `EventSink`, so tool
        functions always have a sink to call without an `Option` check.

#### 6. `crates/sparql-mcp-core/src/application/mod.rs` (Modify)

*   **Type / Scope**: Module declarations.
*   **Proposed Changes**: Add `pub mod event_sink;` and re-export
    `EventSink`, `StoreEvent`, `SubjectKind`, `NullEventSink`,
    `BroadcastEventSink`.

#### 7. `crates/sparql-mcp-core/src/application/tools/sparql.rs` (Modify)

*   **Type / Scope**: `update_sparql` tool implementation.
*   **Proposed Changes**: `update_sparql` takes an added
    `event_sink: &dyn EventSink` parameter. After `store.update(sparql)`
    succeeds (and `dry_run` is false): attempt to parse `sparql` as a ground
    `INSERT DATA` / `DELETE DATA` request (reusing Oxigraph's own update
    parser, already a transitive dependency) — if it parses as ground data,
    emit a precise `StoreEvent::Insert`/`Delete` with the literal triples and
    their classification; otherwise (pattern-based `INSERT/DELETE … WHERE`,
    `LOAD`, `CLEAR`) emit `StoreEvent::Changed` using the before/after counts
    `with_delta` already computes today. This is an explicit, documented
    limitation, not a silent gap: pattern-based updates do not get
    node-level detail in the viewer in v1.

#### 8. `crates/sparql-mcp-core/src/application/tools/ontology.rs` (Modify)

*   **Type / Scope**: `load_ontology` / `load_ontology_file` tool
    implementations.
*   **Proposed Changes**: Both take the same `event_sink: &dyn EventSink`
    parameter. Both already parse the full RDF payload before loading it
    (`load_rdf` / `load_rdf_file`), and the payload is always ground data, so
    both emit a precise `StoreEvent::Insert` built from exactly the triples
    that were parsed, classified via `classify_subjects`. This is the
    primary path by which *ontology* classes reach the viewer (instances
    typically arrive via `update_sparql`).

#### 9. `crates/sparql-mcp-core/src/infrastructure/viewer_channel.rs` (Create)

*   **Type / Scope**: New infrastructure adapter.
*   **Proposed Changes**: `spawn_viewer_channel(bind: SocketAddr, store:
    Arc<dyn SparqlStore>, active_graph: String, events: broadcast::Receiver<
    StoreEvent>)` — binds a `tokio::net::TcpListener` on `bind` and, per
    accepted connection: reads a `{"cmd":"snapshot"}` line, runs a
    `CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }` query against
    `active_graph` through the existing `SparqlStore::query` path, classifies
    the result with `classify_subjects`, writes it back as one JSON object,
    then forwards every subsequent `StoreEvent` from a cloned broadcast
    receiver as one newline-delimited JSON object per event until the
    client disconnects.

#### 10. `crates/sparql-mcp-core/src/infrastructure/mod.rs` (Modify)

*   **Type / Scope**: Module declarations.
*   **Proposed Changes**: Add `pub mod viewer_channel;` and
    `pub use viewer_channel::spawn_viewer_channel;`, alongside the existing
    `fs_doc_store` / `oxigraph` exports.

#### 11. `crates/sparql-mcp-core/src/mcp/server.rs` (Modify)

*   **Type / Scope**: `SparqlMcpServer` struct + constructor + tool dispatch.
*   **Proposed Changes**: Add an `event_sink: Arc<dyn EventSink>` field,
    threaded through `SparqlMcpServer::new(...)`'s parameter list (alongside
    `store`, `doc_store`, …), and passed into the existing call sites that
    invoke `sparql::update_sparql(...)`, `ontology::load_ontology(...)`, and
    `ontology::load_ontology_file(...)`.

#### 12. `crates/sparql-mcp-core/src/main.rs` (Modify)

*   **Type / Scope**: CLI entrypoint / server bootstrap.
*   **Proposed Changes**: In the command path that starts the STDIO server:
    read `config.viewer`. If `enabled`, create a
    `tokio::sync::broadcast::channel`, wrap its sender in a
    `BroadcastEventSink`, pass it to `SparqlMcpServer::new(...)`, and
    `tokio::spawn(spawn_viewer_channel(bind, store.clone(), active_graph.clone(), receiver))`
    before entering the existing stdio `serve` loop. If disabled or absent,
    pass a `NullEventSink` — today's behavior, unchanged.

#### 13. `crates/sparql-mcp-core/tests/viewer_channel.rs` (Create)

*   **Type / Scope**: Integration test, same style as the existing
    `tests/tools_sparql.rs` / `tests/mcp_handshake.rs`.
*   **Proposed Changes**: Start an in-memory store, call
    `spawn_viewer_channel` on an ephemeral `127.0.0.1:0` port, call
    `ontology::load_ontology` and `sparql::update_sparql` directly against
    the same store/sink, connect a plain `TcpStream` test client, and assert
    it receives the snapshot line followed by the expected `Insert` event
    lines in order.

#### 14. `crates/sparql-mcp-viewer/Cargo.toml` (Create)

*   **Type / Scope**: New binary crate manifest.
*   **Proposed Changes**: `[package] name = "sparql-mcp-viewer"`; `[[bin]]`
    `path = "src/main.rs"`; dependencies: `sparql-mcp` (path dependency on
    `../sparql-mcp-core`, for `StoreEvent`/`SubjectKind`/`Config` reuse —
    note the core crate's package name is `sparql-mcp`, not
    `sparql-mcp-core`), `bevy` (pinned version), `tokio` (`net`,
    `rt-multi-thread`, `macros`), `serde`, `serde_json`.

#### 15. `crates/sparql-mcp-viewer/src/main.rs` (Create)

*   **Type / Scope**: Bevy application entrypoint.
*   **Proposed Changes**: Reads `sparql-mcp.toml` (via `sparql_mcp::config::
    Config`, the same type the server uses, so the viewer can never drift
    from the server's idea of the bind address), builds a Bevy `App` with
    `DefaultPlugins` (window title/size, clear color), registers the
    resources/systems defined in `connection.rs`, `graph.rs`, `layout.rs`,
    and `render.rs`, and runs the app.

#### 16. `crates/sparql-mcp-viewer/src/connection.rs` (Create)

*   **Type / Scope**: Networking module.
*   **Proposed Changes**: Connects to the configured `bind` address over
    TCP on a background `tokio` runtime, sends `{"cmd":"snapshot"}`, decodes
    the newline-delimited JSON stream (snapshot, then events) into
    `StoreEvent`s, and pushes them through an `std::sync::mpsc` channel that
    a Bevy system drains once per frame — keeping all networking off the
    render thread.

#### 17. `crates/sparql-mcp-viewer/src/graph.rs` (Create)

*   **Type / Scope**: In-memory graph model.
*   **Proposed Changes**: `ViewerGraph` keyed by subject/object IRI, storing
    `kind: Class | Instance`, `label`, and a `Vec3` position per node, plus
    `(from, to, predicate)` edges; `apply_event(&mut self, StoreEvent)`
    inserts or removes nodes/edges; new nodes are recorded with no position
    yet, so `layout.rs` is solely responsible for placing them.

#### 18. `crates/sparql-mcp-viewer/src/layout.rs` (Create)

*   **Type / Scope**: Force-directed layout simulation.
*   **Proposed Changes**: A fixed-timestep Bevy system running an
    all-pairs repulsion + spring-attraction-along-edges simulation with
    damping; existing nodes' positions are never reset, only nudged each
    tick; a node with no position yet spawns at a jittered offset from its
    first connected neighbor's current position (or the graph centroid if it
    has none), so new arrivals visibly settle into place instead of starting
    stacked at the origin.

#### 19. `crates/sparql-mcp-viewer/src/render.rs` (Create)

*   **Type / Scope**: Bevy rendering systems.
*   **Proposed Changes**: Reconciles `ViewerGraph` against spawned Bevy
    entities each frame (spawn/despawn meshes to match current nodes/edges,
    update transforms from `layout.rs`'s positions); class nodes get a
    larger, emissive-material mesh, instance nodes a smaller
    standard-shaded mesh, edges render as translucent lines; a newly spawned
    node's scale animates 0→1 over its first ~0.3s instead of appearing
    instantly; sets up bloom, basic lighting, and an orbit-camera controller
    (drag to rotate, scroll to zoom).

#### 20. `crates/sparql-mcp-viewer/README.md` (Create)

*   **Type / Scope**: New crate documentation.
*   **Proposed Changes**: Explains what the viewer is, how to set
    `[viewer] enabled = true` in `sparql-mcp.toml`, how to build the Windows
    binary (native build on Windows via a standard `rustup` toolchain
    against this repository, the same target (`x86_64-pc-windows-msvc`)
    `release.yml` already builds), and how to run the resulting
    `.exe` against a running `sparql-mcp serve` session. States the
    loopback-only, unauthenticated nature of the side-channel as a known
    limitation.

#### 21. `README.md` (root) (Modify)

*   **Type / Scope**: Project overview doc.
*   **Proposed Changes**: Add `crates/sparql-mcp-viewer/` to the "What's in
    this repo" tree; add a short "Live 3D viewer (optional)" section
    describing the feature and linking to `crates/sparql-mcp-viewer/README.md`.

#### 22. `CHANGELOG.md` (Modify)

*   **Type / Scope**: Project changelog.
*   **Proposed Changes**: Add an entry under the current "Unreleased"
    heading's "Added" list: the `[viewer]` config section, the
    `EventSink`/`StoreEvent` side-channel, and the new
    `sparql-mcp-viewer` crate — following the existing entry style.

#### 23. `.github/workflows/ci.yml` (Modify)

*   **Type / Scope**: CI workflow.
*   **Proposed Changes**: Add a step, before `cargo clippy`/`cargo test`,
    installing the Linux packages Bevy needs to compile on `ubuntu-latest`
    (ALSA, udev, and X11/Wayland/Vulkan loader dev headers) — without this,
    adding `sparql-mcp-viewer` to the workspace breaks the existing
    `cargo clippy --all-targets --all-features` / `cargo test --all
    --all-features` steps, which build every workspace member regardless of
    which OS they're meant to ship on.

#### 24. `.github/workflows/release.yml` (Modify)

*   **Type / Scope**: Release workflow.
*   **Proposed Changes**: In the `x86_64-pc-windows-msvc` matrix entry only,
    extend the "Build" step to also build `sparql-mcp-viewer`, and the
    "Package" step to include `sparql-mcp-viewer.exe` in that platform's
    archive. Linux and (commented-out) macOS matrix entries are left
    untouched — the viewer is Windows-only per the chosen architecture.

### Verification Plan

*   **Compilation/Build**: `cargo build --workspace` (now includes
    `sparql-mcp-viewer`); the Windows-only GUI binary itself can only be
    fully exercised on a real Windows machine or the `windows-latest` CI
    runner — this sandbox can type-check and lint it, not display a window.
*   **Unit Tests**: `cargo test --all` — new coverage for the `[viewer]`
    config parsing (file 4), `classify_subjects` (file 5), and the
    ground-vs-pattern-update branch in `update_sparql` (file 7).
*   **Integration Test**: `crates/sparql-mcp-core/tests/viewer_channel.rs`
    (file 13) — snapshot + event delivery over a real TCP connection.
*   **Static Analysis & Linting**: existing `cargo clippy --all-targets
    --all-features -- -D warnings` and `cargo fmt --all -- --check` gates,
    now covering the new crate too.
*   **Manual verification (cannot be scripted from this environment)**: run
    `sparql-mcp serve` natively on Windows with `[viewer] enabled = true`,
    run the built `sparql-mcp-viewer.exe` on the same machine, and confirm
    CUJ 1 — issuing
    `load_ontology` / `update_sparql` calls from an agent session makes new
    nodes appear in the running viewer without a restart or a full reload.

--------------------------------------------------------------------------------

*Drafted with EGM*
