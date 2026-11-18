# sparql-mcp Viewer: Agent Control over MCP

*   **Author:** dminier
*   **Status:** Draft
*   **Date:** 2026-11-18

--------------------------------------------------------------------------------

## Overview

`sparql-mcp-viewer` (see [`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md))
draws the active project graph of `sparql-mcp` as a live 3D node-link graph.
Today, only the user drives it: mouse for the camera, `F` to frame the graph,
clicks for selection, a panel for the class filter.

This document lets the **agent** drive the viewer too. The agent can move the
camera, show or isolate a set of nodes, change the class filter, select a node,
search, and take a screenshot to see the result. The typical use is
conversational: the user asks the agent *"show me every service that depends
on X"*, the agent finds the answer with SPARQL, and the viewer displays it.

The control surface is exposed as MCP tools of **`sparql-mcp-core`**, the MCP
server the agent already talks to. The core relays each command to the viewer
over the existing loopback TCP side-channel, which becomes bidirectional. The
viewer does not become an MCP server itself (see Alternative A).

Ownership is split across three documents:

| Concern | Owned by |
|---|---|
| Side-channel listener, event emission, base wire protocol | `interface-agentic-rust-stylee.md` |
| Viewer projection, layout, rendering, user interaction | `sparql-mcp-viewer.md` |
| `viewer_*` MCP tools, control messages on the side-channel, agent-driven viewer state | this document |
| Agent-driven batch ingestion (`ingest_*` tools, limits, write-path hardening) | [`sparql-mcp-batch-ingestion.md`](./sparql-mcp-batch-ingestion.md) |

The two agent-facing features meet at commit time. After `ingest_commit`, the
agent calls `viewer_show` on the new subjects. Ingestion keeps every
`StoreEvent` bounded (`event_max_triples`), so the viewer's ingestion feed
stays responsive during large jobs.

The protocol additions below are listed under *Required changes to the
companion designs* so they land in `interface-agentic-rust-stylee.md` first,
as that design requires.

## The Problem

*   **The agent is blind to the viewer.** The agent writes triples and the
    viewer shows them, but the agent cannot point at anything. When the user
    asks "where is X?", the agent can only answer in text.
*   **Search is missing.** Neither the viewer nor its panels offer search.
    With a real ontology (thousands of nodes), finding a node by eye is slow.
*   **Query results are not visual.** The agent answers structural questions
    with `query_sparql`, but the result set (a list of IRIs) never reaches the
    3D view, which is where it would be easiest to read.
*   **The channel is one-way.** After the initial `{"cmd":"snapshot"}`, the
    viewer only receives. Nothing can flow back: no command result, no search
    hit, no image.
*   **The agent cannot check what it did.** Without feedback, the agent cannot
    tell whether a camera move framed the right nodes or whether a highlight
    is readable.
*   **Two drivers, one view.** Once both the user and the agent can move the
    camera, a rule is needed for who wins.

## Goals

*   **Agent-driven display.** The agent can frame the whole graph or a set of
    IRIs, highlight or isolate a set of IRIs, select a node, and change the
    class filter. Each command is visible in the viewer within one frame of
    its arrival. Camera moves are animated.
*   **Search in both the view and the store.** `viewer_search` matches labels,
    IRIs and literal attributes in the drawn graph, and/or in every named
    graph of the store through SPARQL. Each hit says whether its node is
    currently drawn.
*   **Query-to-display.** Any IRI list, in particular one built by the agent
    with `query_sparql`, can be displayed with one `viewer_show` call. The
    response lists the IRIs that are not drawn.
*   **Visual feedback.** `viewer_screenshot` returns a PNG of the viewer
    window as MCP image content, so the agent can see what the user sees.
*   **The user can always take back control.** No agent command ever locks the
    UI. Any user camera input cancels an agent camera animation at once. The
    user can clear the agent's highlight with `Esc` or a button. Each
    response tells the agent whether the user has been interacting, so the
    agent can avoid undoing the user's changes.
*   **No viewer means no-op, not error.** When no viewer is connected, every
    `viewer_*` tool succeeds, changes nothing and reports
    `"viewer": "not_connected"`. The agent's flow never breaks because the
    window is closed.
*   **Backward compatible.** An old viewer with a new core, or a new viewer
    with an old core, keeps working as today. Control is negotiated at
    connect.
*   **Testable without a window.** Pure functions (search, IRI resolution,
    framing math) have unit tests. The control path, from MCP call to viewer
    response, has an end-to-end test using the headless viewer.

## Non-Goals

*   **Writing to the store from the viewer.** The viewer stays read-only.
    `viewer_*` tools never modify triples.
*   **A viewer-side MCP server.** Rejected in Alternative A.
*   **Natural-language search in the viewer.** The viewer does substring
    matching. Understanding the user's question and building SPARQL is the
    agent's job.
*   **Driving layout parameters, labels density or rendering style.** These
    stay user settings in v1 (see Future Improvements).
*   **Multi-viewer orchestration.** With several viewers connected to the same
    core, only one is controlled (see *Viewer registry*).
*   **Controlling a viewer from a session that does not own the side-channel
    port.** With several Claude Code sessions, only the session whose core
    bound `[viewer] bind` reaches the viewer. The others get `not_connected`.

## Critical User Journeys (CUJs)

### CUJ 1: Ask the agent to show a query result

*   **Given**: The viewer is connected and shows a populated graph.
*   **When**: The user asks "show me the services that depend on the auth
    module". The agent runs `query_sparql`, gets 14 IRIs, and calls
    `viewer_show` with `mode: "isolate"` and `focus: true`.
*   **Then**:
    *   The 14 nodes keep their material and gain a highlight ring. Every
        other node and edge fades, but nothing moves.
    *   The camera flies to frame the 14 nodes in about 0.6 s.
    *   The activity feed shows `agent · show 14 (isolate)`.
    *   The tool result reports `shown: 14`, `missing: []` and the new camera
        pose.

### CUJ 2: Search across view and store

*   **Given**: The viewer shows project graph `P`.
*   **When**: The user asks "where is `OrderService`?". The agent calls
    `viewer_search` with `text: "OrderService"`, `scope: "both"`,
    `show: "highlight"`.
*   **Then**:
    *   Two hits come back. One is in `P` (`in_view: true`) and is highlighted
        and framed. The other is in graph `Q` (`in_view: false`).
    *   The agent tells the user about the second hit and can offer
        `project_switch` to `Q`.

### CUJ 3: The user takes back control

*   **Given**: The agent has just called `viewer_camera` with
    `action: "frame"`, and the camera is animating.
*   **When**: The user right-drags to orbit in the middle of the animation.
*   **Then**:
    *   The animation stops on that frame, and the user's orbit applies from
        there with no jump.
    *   The next `viewer_*` response carries `user.overrode_last_agent_command:
        true`.
*   **And when**: The user presses `Esc` with no node selected.
*   **Then**: The agent's highlight or isolation is cleared and the normal
    view comes back.

### CUJ 4: The agent checks its own display

*   **Given**: The agent has isolated 300 nodes for the user.
*   **When**: The agent calls `viewer_screenshot`.
*   **Then**: The tool returns a PNG of at most 1568 px on its longest side,
    plus the camera pose and the visible node count. The agent sees that
    labels overlap, and calls `viewer_camera` with `action: "zoom"` to
    tighten the view.

### CUJ 5: Viewer closed

*   **Given**: No viewer is running.
*   **When**: The agent calls `viewer_show`.
*   **Then**: The call succeeds with `{"viewer": "not_connected",
    "applied": false}`. No error is raised, and nothing else changes.

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

### Control path

```text
Claude Code ──stdio MCP──▶ sparql-mcp-core (native Windows)
                              │  viewer_* tool
                              │    ├─ store-side work (SPARQL search) runs here
                              │    └─ ViewerControl::request(op) ──┐
                              │                                    ▼
                              │                  ViewerHub (registry + pending ids)
                              │                                    │ {"request":…}
                              ▼                                    ▼
                       viewer_channel  ◀══ loopback TCP (existing) ══▶ sparql-mcp-viewer (native Windows)
                              ▲                                    │ connection thread
                              │ {"response":…}                     ▼
                              └──────────── AgentInbox ─▶ agent systems (Bevy) ─▶ camera / highlight /
                                                                                  filter / selection /
                                                                                  screenshot
```

The core already holds the agent's MCP session and an accepted TCP
connection to the viewer. Control reuses both. The TCP connection direction
does not change: the viewer still connects to the core on `127.0.0.1`. Both
processes run natively on the same Windows machine, so there is no cross-OS
step. Nothing in this design is Windows-specific, so the same
single-binary-per-OS layout carries over to a future native Linux build.

### Wire protocol additions

All lines stay newline-delimited JSON, told apart by their single top-level
key, as in the base protocol.

**Handshake.** The viewer's first line gains an optional field:

```json
{"cmd":"snapshot","control":1}
```

`control` is the control protocol version. The core marks a connection as
controllable only when `control >= 1`. An old core ignores the field. An old
viewer omits it and is never sent a request.

**Request (core → viewer).**

```json
{"request":{"id":42,"op":"show","args":{"iris":["…"],"mode":"isolate","focus":true}}}
```

**Response (viewer → core).**

```json
{"response":{"id":42,"ok":true,"result":{…},"user":{"active":false,"overrode_last_agent_command":false}}}
{"response":{"id":42,"ok":false,"error":{"code":"unsupported_headless","message":"…"}}}
```

*   `id` is a `u64` that is unique per core process. The viewer echoes it
    and never reorders the meaning of two ids.
*   `op` is one of `status`, `camera`, `show`, `clear`, `filter`, `select`,
    `search_view`, `screenshot`. An unknown op returns
    `error.code = "unknown_op"`.
*   The `user` block is present on every successful response (see *User
    takeover*).
*   Request lines are capped at 1 MiB. Response lines are capped at 8 MiB,
    which leaves room for a base64 screenshot. A line over the cap is
    dropped, and the waiting request times out.

**Viewer side.** A `request` line is a new `Decoded` variant. An old viewer
already treats it as malformed and skips it with a warning, but it never
receives one because it did not send `control`.

### Viewer registry (`ViewerHub`, core)

`ViewerHub` lives in `infrastructure::viewer_channel` and implements the
application-layer trait `ViewerControl`:

```rust
pub enum ViewerOutcome {
    Applied { result: serde_json::Value, user: UserActivity },
    NotConnected,
    Timeout,
    Rejected { code: String, message: String },
}

pub trait ViewerControl: Send + Sync {
    fn request(&self, op: ViewerOp, timeout: Duration)
        -> Pin<Box<dyn Future<Output = ViewerOutcome> + Send + '_>>;
}
```

*   **Target viewer.** The hub holds at most one *controlled* connection: the
    most recently connected controllable viewer. A newer viewer replaces it.
    When the controlled connection closes, the hub promotes the most recent
    remaining controllable connection, if any.
*   **Pending map.** `request` assigns an id, stores a `oneshot::Sender` in
    `pending`, and pushes the request onto the connection's outbound
    `mpsc`. `handle_connection` routes each incoming `response` line to its
    `oneshot`. When the connection closes, all its pending senders are
    dropped.
*   **Reconnects.** The viewer reconnects on every `changed` event and on
    `Lagged` (base design). A request in flight at that moment loses its
    connection. `request` then waits up to 2 s for a controllable viewer to
    register again and retries once. If none comes back, the outcome is
    `NotConnected`.
*   **Single writer.** `handle_connection` already owns the socket writer and
    runs a `select!` over resync ticks and store events. It gains two arms:
    outbound requests from the hub, and response lines from the reader. Store
    events and requests therefore never interleave inside one line.
*   **The write path is untouched.** `BroadcastEventSink` and the broadcast
    channel do not change. Control traffic never blocks a store write.

### MCP tools (core)

The tools are listed only when `[viewer] enabled = true`. They are
implemented in `application::tools::viewer` and dispatched asynchronously
from `SparqlMcpServer::call_tool`, before the synchronous `dispatch_core`.
That is the only change to the dispatcher.

Every tool returns a JSON text result with this envelope:

```json
{"viewer":"connected"|"not_connected"|"timeout","applied":true|false,
 "result":{…},"user":{"active":false,"overrode_last_agent_command":false}}
```

`Rejected` outcomes (bad arguments, `unsupported_headless`,
`user_controlling_camera`) set `applied: false` and add `reason`. Only
malformed tool arguments raise an MCP `invalid_params` error.

| Tool | Arguments | Effect / result |
|---|---|---|
| `viewer_status` | none | Graph IRI, node and edge counts, visible count, camera pose (`position`, `look_at`, `distance`), selection, class filter, current agent highlight, connection state |
| `viewer_camera` | `action`: `frame_all` \| `frame` \| `orbit` \| `zoom` \| `reset`; `iris?`, `yaw_deg?`, `pitch_deg?`, `factor?`, `duration_ms?` (default 600, 0 = instant) | Animated camera move. Returns the target pose. |
| `viewer_show` | `iris` (≤ 5000), `mode`: `highlight` \| `isolate`, `focus?` (default true), `select?` | Sets the agent highlight. Returns `shown`, `missing` (IRIs not drawn), pose if focused. |
| `viewer_clear` | none | Clears the agent highlight. Leaves camera, filter and selection alone. |
| `viewer_filter` | `hide_classes?`, `show_classes?`, `show_untyped?`, `reset?` | Edits the user-visible `ClassFilter`. Returns the resulting filter. |
| `viewer_select` | `iri` or `null` | Selects a node and opens its panel, or clears the selection. |
| `viewer_search` | `text`, `scope`: `view` \| `store` \| `both` (default `both`), `limit?` (default 25), `show?`: `none` \| `highlight` \| `isolate` (default `none`) | Hits `{iri, label, graph, matched_on, in_view}`. With `show`, chains into `viewer_show` on the in-view hits. |
| `viewer_screenshot` | `max_side?` (default and max 1568) | MCP image content (PNG), plus a text block with pose and visible count |

Tool descriptions tell the agent three things. Use `query_sparql` followed by
`viewer_show` for structural questions, and `viewer_search` for name lookups.
Check `user.active` before moving the camera. Use `viewer_screenshot` to
check a display before describing it to the user.

**IRIs only.** Every node argument is an IRI, given as `http://…` or
`<http://…>`. Blank nodes cannot be addressed because their ids are not
stable across snapshots (base design).

### Search

Search covers two sources, as decided for this feature.

*   **`view` scope** runs in the viewer (op `search_view`) over the drawn
    `ViewerGraph`. It does a case-insensitive substring match on the node
    label, the IRI local name, the full IRI, and literal attribute values.
    Hits are ranked: exact label match first, then label prefix, then label
    substring, then IRI, then attribute. Ties are broken by `mentions`, most
    used first. The function `search_graph(&ViewerGraph, &str, limit)` is
    pure and unit-tested.
*   **`store` scope** runs in the core over **all named graphs** with one
    SPARQL `SELECT`. It matches `STR(?s)` and literal objects of `?s` with
    `CONTAINS(LCASE(…), LCASE(?needle))`. It returns `?g ?s` and the best
    `rdfs:label`/`skos:prefLabel`, with `LIMIT`. The needle is passed as an
    escaped SPARQL string literal built by one helper, never by string
    concatenation of raw user text.
*   **`both`** runs the two at once and merges them by IRI. `in_view` is true
    for an IRI found by the view search or present in the viewer graph.
    Store hits in the active graph that the viewer does not draw (literal-only
    subjects) come back with `in_view: false`.

### Viewer-side agent state (`agent` module)

A new `AgentPlugin` is added to `ViewerPlugin`. The connection thread pushes
requests into a new `ViewerMsg::Request(AgentRequest)` variant, which goes
through the existing `Inbox` → `FrameMessages` path. Responses go back
through an `mpsc` sender held in an `AgentOutbox` resource. The connection
thread writes them to the socket.

*   **`handle_agent_requests`** runs in `CoreSet::Ui`, after `apply_to_graph`,
    so a request always sees the graph as of the same frame. It decodes the
    op, applies it to the resources below and answers in the same frame,
    except `screenshot` (see below).
*   **`AgentHighlight { terms: HashSet<String>, mode: Highlight | Isolate }`**
    is keyed by term, not `NodeId`, so it survives snapshot rebuilds the way
    `Selection` does.
*   **Rendering.** `apply_node_materials` gains one material,
    `agent_highlight_material`, which is emissive cyan. The precedence is:
    selected > agent-highlighted > faded (class filter, or not highlighted
    in `Isolate` mode) > last-batch pulse > kind. Under `Isolate`, an edge is
    faded unless both of its ends are highlighted, which reuses the existing
    faded edge meshes.
*   **Filter and selection** write the existing `ClassFilter` and `Selection`
    resources. The agent's changes are therefore exactly what the user would
    see after clicking, and the user can undo them with the same UI.
*   **Activity feed.** A new `FeedKind::Agent` entry is pushed per applied
    command, for example `agent · frame 14`. The ingestion panel gains a
    small "Agent view active · Clear" chip while `AgentHighlight` is not
    empty.

### Camera animation

Bevy 0.20's `PanOrbitCamera` is anchor-based and reads the camera
`Transform` each frame. It has no orbit state to keep in sync, and the
existing `F` key already writes `Transform` directly. Its `LookTo` extension
only rotates about the anchor, so it cannot fly to a target.

*   A **`CameraTween`** component (start and end `Transform`, start time,
    duration, ease-in-out) is inserted on the `SceneCamera`. A system in
    `PreUpdate`, ordered before `PanOrbitCamera::update_camera_positions`,
    interpolates position (lerp) and rotation (slerp), and removes the
    component when it is done.
*   **Targets.** `frame` and `frame_all` reuse `bounding_sphere` and
    `framing_distance` from `camera.rs` on the selected positions. They keep
    the current viewing direction, as `F` does. `orbit` rotates about the
    framed centre (or the current look-at point) by yaw and pitch. `zoom`
    scales the distance to the look-at point. `reset` returns to the start
    pose.
*   **The `F` key** is refactored to the same `frame_target` function, with
    duration 0, so the user and the agent frame identically.

### User takeover

The rule is simple: the agent never locks anything, and the user always wins.

*   **Camera.** While `PanOrbitCamera::is_actively_controlled()` is true
    (the user is dragging or scrolling), `camera`, `show` with
    `focus: true` and `search` with `show` do not move the camera. They
    return `reason: "user_controlling_camera"`, and any highlight still
    applies. User input during a `CameraTween` removes the tween on that
    frame.
*   **Highlight.** `Esc` clears the selection first, as today. A second `Esc`
    with nothing selected clears `AgentHighlight`. The chip's "Clear" button
    does the same.
*   **Feedback.** The viewer records the time of the last user input and
    whether the user changed the camera, filter or selection after the last
    agent command. Each response carries `user.active` (input in the last
    3 s) and `user.overrode_last_agent_command`. The agent then decides; the
    viewer does not.

### Screenshot

*   The viewer spawns `Screenshot::primary_window()` with an observer on
    `ScreenshotCaptured`. Capture is asynchronous, so the request id is kept
    on the observer and the response is sent from it, usually one or two
    frames later.
*   The captured `Image` is converted to RGBA8, downscaled so its longest
    side is at most `max_side` (≤ 1568 px), encoded as PNG and sent as
    base64. The core turns it into MCP image content
    (`Content::image(base64, "image/png")`).
*   UI panels are included, because they show what the user sees. Labels are
    in the image, so the agent can read node names from it.
*   The screenshot timeout is 10 s. Other ops use 3 s.

### Headless mode

`--headless` registers the control handshake and answers `status` and
`search_view` from `ViewerGraph`. Every op that needs a renderer (`camera`,
`show`, `clear`, `filter`, `select`, `screenshot`) returns
`unsupported_headless`. This keeps the end-to-end test free of a window
while still covering the full request/response path.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: The viewer is its own MCP server

The viewer would run an `rmcp` Streamable-HTTP server on Windows loopback,
and Claude Code would connect to it as a second MCP server.

*   **Pros:** The viewer is controllable without `sparql-mcp-core`. No change
    to the side-channel.
*   **Cons:** It adds a second MCP server to configure per project, plus a
    second loopback port for the viewer to bind. That port is lost if the
    viewer crashes or starts twice. The tools would vanish or fail whenever
    the window is closed, which goes against the no-op goal. The agent would combine two servers to do
    `query → show`, and store search would need the viewer to query the
    store through a new path.
*   **Rejected:** It costs more setup and is less robust, for no gain in
    the main query-to-display journey.

### Alternative B: Core-side tools relayed over the side-channel (chosen)

This is the design above.

### Alternative C: A separate control port opened by the viewer

The core would connect to a second TCP port on the viewer for commands.

*   **Cons:** The viewer would have to bind a second port. Two connections
    would have to be kept consistent (which viewer goes with which stream),
    and the core would need to discover the viewer's port.
*   **Rejected:** The existing connection already carries all the needed
    context.

### Alternative D: Commands as triples in a control graph

The agent would write `viewer:Command` triples with `update_sparql`, and the
viewer would react to the matching insert events. There would be no
protocol change.

*   **Cons:** There is no response path, so search results, missing IRIs and
    screenshots are impossible. It pollutes the store, makes every camera
    move a store write, and fires the ingestion pulse on control traffic.
*   **Rejected:** It cannot meet the feedback goals.

### Alternative E: Agent changes go to a separate "agent layer", never to user state

Agent filter and selection would be kept apart from the user's
`ClassFilter` and `Selection`, and combined at render time.

*   **Pros:** The user's own settings are never overwritten.
*   **Cons:** It creates two filters and two selections, which the user
    cannot tell apart on screen, and the panels would need to show both.
*   **Partly adopted:** Only the highlight, which has no user equivalent, is
    agent-owned (`AgentHighlight`). Filter and selection write the user's
    state, so they stay visible and undoable through the existing UI.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Latency.** A command is visible within one frame of the viewer
    receiving it. Round trip from tool call to response is under 100 ms on
    loopback, excluding camera animation and screenshot.
*   **Timeouts.** 3 s per request (10 s for screenshot), plus at most one 2 s
    re-registration wait on reconnect. A tool call therefore never hangs
    longer than about 15 s.
*   **No impact on the write path.** Control uses its own `mpsc` and never
    touches `BroadcastEventSink`. A stalled viewer can delay `viewer_*`
    tools only up to their timeout, never `update_sparql`.
*   **Payload bounds.** At most 5000 IRIs per `show`. Request lines are
    ≤ 1 MiB and response lines ≤ 8 MiB. A 1568 px PNG of a dark scene is
    usually well under 2 MiB.
*   **Security and privacy.** The listener stays loopback-only. Any local
    process can already read the store stream. With this change, a local
    process that connects with `control: 1` can also answer the agent's
    `viewer_*` calls, for example with fake search hits or a fake
    screenshot. This matches the existing trust boundary (local user), and
    is written down here rather than assumed away. Search text reaches
    SPARQL only through the escaping helper. No op takes a file path.
*   **Compatibility.** Mixed versions behave as today (see Wire protocol
    additions). `viewer_*` tools are not listed when `[viewer]` is disabled.

### Future Improvements

*   **Saved views.** Named camera, filter and highlight presets that the
    agent can store, for example as triples, and restore.
*   **Follow mode.** The camera frames each new ingestion batch
    automatically, as an agent-toggled mode.
*   **Agent control of labels and layout.** Label density, spacing, and
    freezing the layout.
*   **Annotated screenshots.** Overlay IRIs or numbered markers so the agent
    can refer to nodes in the image without reading labels.
*   **Choosing among several viewers.** Address a viewer by id in
    `viewer_status`.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Required changes to the companion designs

1.  **`interface-agentic-rust-stylee.md`, wire protocol.** Add the `control`
    handshake field, the `request` and `response` line kinds, the line size
    caps, and the rule that only controllable connections receive requests.
2.  **`interface-agentic-rust-stylee.md`, connection handling.**
    `handle_connection` keeps reading after the first line, and is the single
    writer for events, snapshots and requests.
3.  **`sparql-mcp-viewer.md`, Non-Goals.** "SPARQL querying" stays a
    non-goal for the UI. Add a pointer to this document for agent-driven
    search. The material precedence list gains the agent highlight, and
    `Esc` gains its second meaning.

### Delivery phases

Each phase starts only when the previous phase's exit criterion passes, as
in the viewer design.

| Phase | Scope | Exit criterion |
|---|---|---|
| 1 | Protocol, handshake, `ViewerHub`, `ViewerControl`, `viewer_status`, no-op envelope, headless `status` | Core integration test with a fake TCP viewer: status round trip, `not_connected` with no viewer, timeout, reconnect retry. Old-viewer and old-core compatibility tests pass. |
| 2 | `AgentHighlight`, `viewer_show`, `viewer_clear`, `viewer_select`, `viewer_filter`, materials, `Esc` chain, feed entries | Manual CUJ 1 (with a hand-written IRI list) and the highlight half of CUJ 3 pass. Unit tests for IRI resolution and material precedence pass. |
| 3 | `CameraTween`, `viewer_camera`, focus on `show`, user takeover, `F` refactor | Manual CUJ 3 passes. Unit tests for framing a subset, orbit and zoom pose math pass. |
| 4 | `viewer_search` (view, store, both), SPARQL escaping helper | Manual CUJ 2 passes. Unit tests for ranking and escaping (quotes, backslashes, newlines) pass. Headless e2e covers `search_view`. |
| 5 | `viewer_screenshot` | Manual CUJ 4 passes. The returned image displays in Claude Code. A PNG size check passes on a 5,000-node scene. |
| 6 | Docs, skill guidance, CHANGELOG | README tool list updated. The `sparql-first` / `kb-workbench` skill guidance mentions `query_sparql` → `viewer_show`. |

### Target Files to Create/Modify

#### 1. `crates/sparql-mcp-core/src/application/viewer_control.rs` (Create)

`ViewerOp` (a serde-tagged enum mirroring the ops), `UserActivity`,
`ViewerOutcome`, the `ViewerControl` trait, and `NullViewerControl`, which
always returns `NotConnected`. There is no I/O here.

#### 2. `crates/sparql-mcp-core/src/infrastructure/viewer_channel.rs` (Modify)

*   `ViewerHub`: the controlled connection slot, the list of controllable
    connections, the id counter, the `pending` map, the 2 s re-registration
    wait, and `impl ViewerControl`.
*   `spawn_viewer_channel` / `serve_viewer_channel` take an
    `Arc<ViewerHub>`.
*   `handle_connection` parses `control` from the first line, registers with
    the hub, adds the outbound-request and inbound-response arms to its
    `select!`, enforces line caps, and deregisters on exit.

#### 3. `crates/sparql-mcp-core/src/application/tools/viewer.rs` (Create)

Tool definitions (JSON schemas), argument parsing, IRI normalisation
(accepts `<…>` or a bare IRI), the response envelope, store search
(`store_search(store, needle, limit)` and `sparql_string_literal(&str)`),
merging for `scope: both`, and `search … show` chaining.

#### 4. `crates/sparql-mcp-core/src/application/tools/mod.rs` and `application/mod.rs` (Modify)

Export `viewer` and `viewer_control`.

#### 5. `crates/sparql-mcp-core/src/mcp/server.rs` (Modify)

Add a `viewer: Option<Arc<dyn ViewerControl>>` field and
`with_viewer_control`. `list_tools` adds the viewer tool definitions when the
field is `Some`. `call_tool` awaits `viewer::dispatch(…)` for `viewer_*`
names before `dispatch_core`. The `instructions` text gains one sentence on
the viewer tools.

#### 6. `crates/sparql-mcp-core/src/main.rs` (Modify)

In `Cmd::Serve`, when `[viewer]` is enabled and the bind address parses,
create one `Arc<ViewerHub>`, pass it to `spawn_viewer_channel`, and call
`srv.with_viewer_control(hub)`.

#### 7. `crates/sparql-mcp-core/tests/viewer_control.rs` (Create)

A fake viewer over `serve_viewer_channel` on an ephemeral port. It covers:
status round trip; `not_connected`; timeout; drop mid-request followed by
reconnect and retry; old-style handshake receiving no requests; oversized
response dropped.

#### 8. `crates/sparql-mcp-viewer/src/connection.rs` (Modify)

Send `"control":1` in the handshake. Add `Decoded::Request` and
`ViewerMsg::Request(AgentRequest)`. `spawn` also returns an
`AgentOutbox` sender, and the streaming `select!` gains an arm that writes
responses to `write_half`.

#### 9. `crates/sparql-mcp-viewer/src/agent.rs` (Create)

`AgentPlugin`, `AgentHighlight`, `AgentOutbox`, `UserActivityTracker`,
`handle_agent_requests`, the screenshot observer, and the pure functions
`resolve_iris`, `search_graph` and `status_snapshot`, with unit tests.

#### 10. `crates/sparql-mcp-viewer/src/camera.rs` (Modify)

`CameraTween` and its `PreUpdate` system, `frame_target`, orbit and zoom pose
functions, cancelling on `is_actively_controlled`, and `F` rewritten on top
of `frame_target`.

#### 11. `crates/sparql-mcp-viewer/src/render.rs` (Modify)

`agent_highlight_material` in `ViewerStyle`. The new precedence in
`apply_node_materials`. Edge fading under `Isolate`.

#### 12. `crates/sparql-mcp-viewer/src/interact.rs` and `src/ingest.rs` (Modify)

The `Esc` chain, recording user activity on filter and selection changes,
`FeedKind::Agent`, and the "Agent view active · Clear" chip.

#### 13. `crates/sparql-mcp-viewer/src/plugin.rs`, `src/headless.rs`, `src/lib.rs` (Modify)

Register `AgentPlugin` in `ViewerPlugin`. Headless answers `status` and
`search_view` and rejects the rest. Declare `pub mod agent`.

#### 14. `crates/sparql-mcp-viewer/Cargo.toml` (Modify)

Add PNG encoding for the screenshot: the Bevy `png` feature, or the `image`
crate with the `png` feature, whichever 0.20.1 exposes for
`Image` → `DynamicImage` conversion (verify in Phase 5). Add `base64`.

#### 15. `crates/sparql-mcp-viewer/tests/headless_e2e.rs` (Modify)

Drive the core through an `rmcp` client (already a dev-dependency), with the
headless viewer attached. Assert `viewer_status` counts and a
`viewer_search` `scope: view` hit.

#### 16. Docs (Modify)

`README.md` (MCP tools exposed), `crates/sparql-mcp-viewer/README.md`
(agent control, `Esc`, the chip), `CHANGELOG.md`, and skill guidance under
`plugins/`.

### Verification Plan

*   `cargo build --workspace`, `cargo test --workspace`,
    `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
*   Core integration tests (file 7) and the headless e2e (file 15) run in CI.
*   Manual runs of CUJs 1–5 on the author's machine: core and viewer both
    native on Windows, Claude Code driving. CUJ 5 is checked by closing the viewer
    mid-session.
*   Compatibility: run the previous release's viewer against the new core,
    and the new viewer against the previous core. Both stream normally, and
    the new core reports `not_connected` for the old viewer.
