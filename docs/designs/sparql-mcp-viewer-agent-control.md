# sparql-mcp Viewer: Agent Control over MCP

*   **Author:** dminier
*   **Status:** Draft (revised after EGM review round 1)
*   **Date:** 2026-11-18

--------------------------------------------------------------------------------

## Overview

`sparql-mcp-viewer` draws the active project graph of `sparql-mcp` as a live 3D
node-link graph. Each sparql-mcp project is a named graph in one shared store.
The viewer draws the active project's graph, and `project_switch` changes
which graph that is. Today only the user drives the viewer: the mouse moves the
camera, `F` frames the graph, clicks select nodes, and a panel sets the class
filter. See [`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md).

This document lets the **agent** drive the viewer too. The agent can:

*   move the camera;
*   highlight or isolate a set of nodes, and select one;
*   change the class filter;
*   search the view and the store;
*   take a screenshot to see the result.

The typical use is conversational. The user asks "show me every service that
depends on X". The agent finds the answer with SPARQL, and the viewer displays
it.

The control surface is a set of MCP tools on **`sparql-mcp-core`**, the MCP
server the agent already talks to. The core relays each command to the viewer
over the existing loopback TCP side-channel, which becomes bidirectional. The
viewer does not become an MCP server (see Alternative A). Both processes run
natively on the same Windows machine.

Ownership is split across five documents:

| Concern | Owned by |
|---|---|
| Side-channel listener, event emission, base wire protocol | [`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md) |
| Viewer projection, layout, rendering, user interaction | [`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md) |
| Write gate, limits, per-graph events | [`sparql-mcp-write-path-hardening.md`](./sparql-mcp-write-path-hardening.md) |
| Agent-driven batch ingestion (`ingest_*`) | [`sparql-mcp-batch-ingestion.md`](./sparql-mcp-batch-ingestion.md) |
| `viewer_*` MCP tools, control messages, agent-driven viewer state | this document |

Agent control and batch ingestion meet at commit time. After `ingest_commit`,
the agent calls `viewer_show` on the commit's `new_subjects`. A large commit
emits one `Changed` event. A controllable viewer resyncs on that event over the
same connection (see [Resync without reconnect](#viewer-registry-viewerhub-core)),
so the following `viewer_show` applies to the new snapshot instead of being
lost in a reconnect.

## The Problem

*   **The agent cannot point at anything in the viewer.** When the user asks
    "where is X?", the agent can only answer in text.
*   **Search is missing.** With a real ontology, the user must scan thousands
    of labels to find one node.
*   **Query results are not visual.** The result of `query_sparql` is a list of
    IRIs, so the user cannot see where those nodes sit in the graph.
*   **The channel is one-way.** After the initial `{"cmd":"snapshot"}`, the
    viewer only receives. No command result, search hit or image can flow
    back.
*   **The agent cannot check what it did.** It cannot tell whether a camera
    move framed the right nodes, or whether a highlight is readable.
*   **Two drivers, one view.** Once both the user and the agent can change the
    view, the design needs a rule for who wins, and a way back to the user's
    own view.

## Goals

*   **Agent-driven display.** The agent can:
    *   frame the whole graph or a set of IRIs, and zoom;
    *   highlight or isolate a set of IRIs, optionally selecting one;
    *   change the class filter.

    A command is visible within one frame of its arrival, and camera moves are
    animated.
*   **Search in both the view and the store.** `viewer_search` matches text in
    the drawn graph (`view`), in every named graph of the store through SPARQL
    (`store`), or both (`both`, the default). One shared matching rule is used
    on both sides. Each hit says whether its node is drawn.
*   **Query-to-display.** Any list of IRIs, in particular one the agent builds
    with `query_sparql`, can be displayed with one `viewer_show` call. The
    response lists the IRIs that are not drawn.
*   **Visual feedback.** `viewer_screenshot` returns a PNG of the viewer window
    as MCP image content.
*   **The user can always take back control.**
    *   No agent command locks the UI.
    *   User camera input, wheel included, cancels an agent camera move at
        once.
    *   `Esc` or the "Restore my view" button clears the agent's highlight and
        restores the camera, filter and selection the user had before the
        agent changed them.
    *   Responses report user activity, so the agent can avoid acting on a
        stale view of the user's state.
*   **No viewer means no-op, not error.** With no viewer connected, every
    `viewer_*` tool succeeds, changes nothing, and reports
    `"viewer": "not_connected"`.
*   **Backward compatible.** An old viewer still receives store events as
    today. The core sends it no requests and reports `not_connected` for it.
    A new viewer with an old core behaves as today.
*   **Testable without a window.** Pure functions have unit tests:
    matching, IRI resolution, material choice, pose math. The full path, from
    MCP call to viewer response, has an end-to-end test using the headless
    viewer.

## Non-Goals

*   **Writing to the store from the viewer.** `viewer_*` tools never modify
    triples.
*   **A viewer-side MCP server.** Rejected in Alternative A.
*   **Natural-language search in the viewer.** Matching is substring-based.
    Understanding the user's question and building SPARQL is the agent's job.
*   **Camera `orbit` and `reset`, and agent control of labels, layout and
    style.** Deferred (see Future Improvements).
*   **Controlling several viewers.** Only one viewer instance is controlled
    (see [Target viewer](#viewer-registry-viewerhub-core)).
*   **Control from a session that does not own the side-channel port.** With
    several Claude Code sessions, only the core that bound `[viewer] bind`
    reaches the viewer. The others get `not_connected`.

## Critical User Journeys (CUJs)

### CUJ 1: Ask the agent to show a query result

*   **Given**: The viewer is connected and shows a populated graph.
*   **When**: The user asks "show me the services that depend on the auth
    module". The agent runs `query_sparql`, gets 14 IRIs, and calls
    `viewer_show` with `mode: "isolate"` and `focus: true`.
*   **Then**:
    *   The 14 nodes switch to the emissive cyan agent-highlight material.
        Every other node fades, and so does every edge that does not have both
        ends highlighted. No node moves.
    *   The camera flies to frame the 14 nodes in 600 ms, the default
        `duration_ms`.
    *   The activity feed shows `agent · show 14 (isolate)`.
    *   The result reports `shown: 14`, `missing: []` and the new camera pose.

### CUJ 2: Search across view and store

*   **Given**: The viewer shows project graph `P`.
*   **When**: The user asks "where is `OrderService`?". The agent calls
    `viewer_search` with `text: "OrderService"`, `scope: "both"` and
    `show: "highlight"`.
*   **Then**:
    *   Two hits come back. One is in `P` (`in_view: true`), and it is
        highlighted and framed. The other is in graph `Q` (`in_view: false`).
    *   The agent tells the user about the second hit, and can offer
        `project_switch` to `Q`.

### CUJ 3: The user takes back control

*   **Given**: The agent has called `viewer_camera` with `action: "frame"`, and
    the camera is animating.
*   **When**: The user scrolls the wheel in the middle of the animation.
*   **Then**: The animation stops on that frame, and the user's zoom applies
    from there with no jump.
*   **And when**: The user presses `Esc` with no node selected.
*   **Then**: The agent's highlight is cleared. The camera, filter and selection
    the user had before the agent's first change come back, with an animated
    move.

### CUJ 4: The agent checks its own display

*   **Given**: The agent has isolated 300 nodes.
*   **When**: The agent calls `viewer_screenshot`.
*   **Then**: The tool returns a PNG of at most 1568 px on its longest side
    (the longest side Claude accepts without downscaling), plus the camera
    pose and the visible node count. The agent sees that labels overlap, and
    calls `viewer_camera` with `action: "zoom"` until the labels separate.

### CUJ 5: Viewer closed

*   **Given**: No viewer is running.
*   **When**: The agent calls `viewer_show`.
*   **Then**: The call succeeds with `{"viewer": "not_connected", "applied":
    false}`. Nothing changes.

### CUJ 6: Ask the agent to hide a class

*   **When**: The user asks "hide the test classes". The agent calls
    `viewer_filter` with `hide_classes: ["…#TestCase"]`.
*   **Then**: The filter panel shows `TestCase` unchecked, and its instances
    fade. The user can re-check it in the panel, or restore their view with
    `Esc`.

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

### Control path

```text
Claude Code ──stdio MCP──▶ sparql-mcp-core (native Windows)
                              │  viewer_* tool
                              │    ├─ store-side work (SPARQL search, spawn_blocking)
                              │    └─ ViewerControl::request(op) ──┐
                              │                                    ▼
                              │                  ViewerHub (instances + pending ids)
                              │                                    │
                     viewer_channel: writer task ── {"request"} ──▶│
                                     reader task ◀─ {"response"} ──┤  loopback TCP (existing)
                                                                   ▼
                       sparql-mcp-viewer (native Windows): connection thread
                         reader task ─▶ Inbox ─▶ FrameMessages ─▶ agent systems (Bevy)
                         writer task ◀─ AgentOutbox ◀──────────── responses
```

`Inbox` is the channel the connection thread fills. Its contents are drained
into `FrameMessages` once per frame.

The core already holds the agent's MCP session and an accepted TCP connection
to the viewer, and control reuses both. The connection direction does not
change: the viewer connects to the core on `127.0.0.1`. Nothing here is
Windows-specific, so the same single-binary-per-OS layout carries over to a
future native Linux build.

### Wire protocol additions

All lines stay newline-delimited JSON. Each kind of line is told apart by a
top-level key, checked in this order: `snapshot`, `event`, `request`,
`response`. Event lines also carry other keys beside `event`.

**Handshake.** The viewer's first line gains two optional fields:

```text
{"cmd":"snapshot","control":1,"instance":"3f2c…"}
```

*   `control` is the control protocol version. The core marks a connection as
    controllable only when `control >= 1`.
*   `instance` is a random id, chosen once per viewer process and kept across
    reconnects.

An old core ignores both fields. An old viewer omits them and is never sent a
request.

**Request (core → viewer).**

```text
{"request":{"id":42,"op":"show","args":{"iris":["…"],"mode":"isolate","focus":true}}}
```

**Response (viewer → core).**

```text
{"response":{"id":42,"ok":true,"result":{…},"user":{"active":false,"epoch":7,"overrode_last_agent_command":false}}}
{"response":{"id":42,"ok":false,"error":{"code":"unsupported_headless","message":"…"}}}
```

*   **Ids.** `id` is a `u64`, unique per core process. The viewer echoes it
    unchanged. Responses may arrive out of request order (a screenshot answers
    frames later), so the core matches them by id only.
*   **Ops.** `op` is one of `status`, `camera`, `show`, `clear`, `filter`,
    `search_view`, `screenshot`. An unknown op returns `unknown_op`.
*   **User block.** Every successful response carries the `user` block (see
    [User takeover](#user-takeover-and-restore)).
*   **Line caps and reader.** Request lines are capped at 1 MiB, and the core
    checks the size before sending. Response lines are capped at 8 MiB.
    *   Both sides read through a cancel-safe, length-capped reader:
        `tokio_util::codec::FramedRead` over
        `LinesCodec::new_with_max_length(cap)`, polled with
        `StreamExt::next()`. `read_line` is never polled inside `select!`,
        because it is not cancel-safe.
    *   On `MaxLineLengthExceeded`, the reader discards the line, logs a
        warning, and keeps reading.
    *   The core's reader cap is 8 MiB. The viewer's reader is uncapped,
        because snapshot lines carry a whole graph.
*   **Old viewers.** A viewer that predates `request` lines treats them as
    malformed and skips them, but it never receives one, because it did not
    send `control`.

### Viewer registry (`ViewerHub`, core)

`ViewerHub` lives in `infrastructure::viewer_channel` and implements the
application-layer trait `ViewerControl`:

```rust
pub enum ViewerOutcome {
    Applied { result: serde_json::Value, user: UserActivity, reason: Option<String> },
    NotConnected,
    Timeout,
    Rejected { code: String, message: String },
}

pub trait ViewerControl: Send + Sync {
    fn request(&self, op: ViewerOp, deadline: Instant)
        -> Pin<Box<dyn Future<Output = ViewerOutcome> + Send + '_>>;
}
```

*   **Target viewer.** The hub holds at most one controlled instance: the
    controllable instance first seen most recently.
    *   A reconnect from the same `instance` keeps the slot, so resyncs and
        reconnects never move control to another window.
    *   If the controlled instance has not come back by the tool deadline, the
        hub promotes the most recently seen remaining instance.
    *   `viewer_status` reports `instance`, so the agent can tell when control
        has moved.
*   **Registration.** `handle_connection` registers with the hub right after
    parsing the handshake, before sending the snapshot.
*   **Separate reader and writer tasks.** `handle_connection` splits the
    socket into two tasks, and neither waits on the other. So a large write in
    one direction (a snapshot, a screenshot) never blocks reading in the
    other.
    *   **Writer task.** It owns `write_half` and runs one `select!` over
        resync ticks, store events, and outbound requests from the hub.
        *   Store events and requests therefore never interleave inside one
            line.
        *   The `select!` is `biased;` with the store-event arm first. It
            drains every event already queued (`try_recv`) before writing a
            request. An event emitted before a `viewer_*` call is therefore
            applied by the viewer before that request.
    *   **Reader task.** It reads `read_half`. It routes each `response` to
        its pending `oneshot`, and forwards a mid-stream `{"cmd":"snapshot"}`
        to the writer.
*   **Resync without reconnect.** A controllable viewer does not reconnect on
    `changed` or `Lagged`.
    *   On `changed`, the viewer sends `{"cmd":"snapshot"}` on the open
        connection.
    *   On `Lagged`, the core resubscribes and pushes a fresh snapshot instead
        of closing the socket.
    *   Both reuse the path the core already follows after `project_switch`:
        resubscribe, then snapshot. Requests in flight are unaffected.
    *   A viewer that did not send `control` keeps the base behavior.
*   **Real disconnects.** When the controlled connection closes (EOF, crash,
    window closed), its pending requests resolve as `NotConnected`. `request`
    then waits, within the tool deadline, for the same instance to register
    again, and retries once if the op is idempotent.
*   **Retry rule.**
    *   `pending` is one hub-wide map from id to
        `(connection, oneshot::Sender)`. A drop guard removes the entry if the
        waiting tool future is cancelled.
    *   A response whose id is not in `pending` is dropped and logged at debug
        level. A retry gets a new id.
    *   Only idempotent ops are retried: `status`, `search_view`,
        `screenshot`, `show`, `clear`, `filter`, and `camera` with `frame` or
        `frame_all`. `camera` with `zoom` is relative, so it is never retried.
    *   The viewer discards unsent responses when its connection closes.
    *   A `Timeout` means the outcome is unknown, and the envelope reports
        `"applied": null`.
    *   Concurrent `viewer_*` calls are sent in arrival order, with no other
        ordering guarantee.
*   **Bind failure.** `main.rs` keeps the `JoinHandle` of
    `spawn_viewer_channel`, and logs a bind error instead of dropping it. The
    hub records the error, and `viewer_status` reports
    `listener: "bind_failed"`.

### MCP tools (core)

The tools are listed when `[viewer] enabled = true` and `bind` parses. If
binding fails at runtime (for example, another session owns the port), they
stay listed and return `not_connected`.

The tools are implemented in `application::tools::viewer`, and dispatched
asynchronously from `SparqlMcpServer::call_tool`. All store work runs in
`spawn_blocking`.

**Envelope.** Every tool returns a JSON text result:

```text
{"viewer": "connected" | "not_connected" | "timeout",
 "applied": true | false | null,
 "reason"?: string,
 "result"?: {…},
 "user"?: {"active": bool, "epoch": u64, "overrode_last_agent_command": bool}}
```

*   `result` and `user` are present only when `viewer` is `connected`.
*   With `not_connected`, `applied` is `false`. With `timeout`, it is `null`,
    because the viewer may still apply the command.
*   A `Rejected` outcome sets `applied: false` and adds `reason`. Rejection
    reasons include `unsupported_headless`, `user_changed`, and a well-formed
    argument the viewer cannot apply.
*   A partly applied command sets `applied: true` and adds `reason`. For
    example, `show` with `focus: true` while the user is moving the camera
    applies the highlight. It returns `result.camera_moved: false` and
    `reason: "user_controlling_camera"`. `viewer_camera` itself returns
    `applied: false` in that case.

**Malformed versus inapplicable arguments.**

*   Malformed arguments fail the tool's JSON schema or a core-side limit: a
    missing required field, an unknown enum value, more than 5000 IRIs,
    `factor <= 0`, or `frame` without `iris`. The core returns MCP
    `invalid_params` for them, without contacting the viewer.
*   Well-formed arguments the viewer cannot apply return `applied: false`
    with `reason`. Examples: `frame` when no IRI is drawn, or `select` of an
    IRI that is not drawn.

| Tool | Arguments | Effect / result |
|---|---|---|
| `viewer_status` | none | `instance`, `listener`, connection state, graph IRI, node and edge counts, visible count, camera pose (`position`, `look_at`, `distance`), selection, class filter, agent highlight, whether a restore point exists |
| `viewer_camera` | `action`: `frame_all` \| `frame` \| `zoom`; `iris?`, `factor?`, `duration_ms?` (default 600; 0 = instant), `if_user_epoch?` | Animated camera move; returns the target pose |
| `viewer_show` | `iris` (≤ 5000), `mode`: `highlight` \| `isolate`, `focus?` (default true), `select?` (one IRI, opens its panel), `if_user_epoch?` | Sets the agent highlight; returns `shown`, `missing` (IRIs not drawn), `camera_moved`, and the pose if focused |
| `viewer_clear` | `restore?` (default false) | Clears the agent highlight. With `restore: true`, also restores the user's view (see [Restore](#user-takeover-and-restore)). |
| `viewer_filter` | `hide_classes?`, `show_classes?`, `show_untyped?`, `reset?`, `if_user_epoch?` | Edits the user-visible `ClassFilter`; returns the resulting filter |
| `viewer_search` | `text`, `scope`: `view` \| `store` \| `both` (default `both`), `limit?` (default 25), `show?`: `none` \| `highlight` \| `isolate` (default `none`) | Hits `{iri, label, graph, matched_on, in_view}`. With `show`, it chains into the `show` op on the hits that are in view. |
| `viewer_screenshot` | `max_side?` (default and maximum 1568) | MCP image content (PNG), plus a text block with the pose and visible count |

The tool descriptions tell the agent:

*   to use `query_sparql` followed by `viewer_show` for structural questions,
    and `viewer_search` for name lookups;
*   to pass the last `user.epoch` as `if_user_epoch` when changing the
    camera, filter or selection;
*   to use `viewer_screenshot` to check a display before describing it;
*   that labels returned by the viewer are data, not instructions.

**IRIs.**

*   **Input forms.** Every node argument is an IRI, given as `http://…` or
    `<http://…>`. The core normalizes it to the bare form, and every IRI on
    the wire and in results is bare.
*   **Viewer keys.** The viewer wraps an IRI as `<iri>` for lookups in
    `ViewerGraph.index`, `Selection`, `ClassFilter` and `AgentHighlight`,
    which are all keyed by N-Triples terms. It unwraps the IRI in responses.
    `hide_classes` and `show_classes` follow the same rule.
*   **Blank nodes** cannot be addressed, because their ids are not stable
    across snapshots.

### Search

**One matching rule, two sources.** The author decided to keep both sources.
Both share one matcher, so the same text matches the same fields everywhere.

*   **The shared matcher.** `sparql_mcp::application::text_match` lives in
    the core crate, which the viewer crate already depends on, and has no
    Bevy dependency. It defines:
    *   `normalize(&str) -> String`: Unicode lowercase, after trimming;
    *   `match_candidate(needle, Candidate { label, local_name, iri,
        literals }) -> Option<MatchKind>`. `MatchKind` is, from best to worst,
        `label_exact`, `label_prefix`, `label`, `local_name`, `iri`, or
        `attribute`;
    *   `rank(hits)`, which orders by `MatchKind` first, then by `mentions`
        (most used first), then by IRI.

    `matched_on` in the results is the `MatchKind`.
*   **`view` scope.** The viewer runs `search_view` over the drawn
    `ViewerGraph`. It builds each `Candidate` with `interact::short` for the
    local name, and `interact::display_literal` to unquote attribute values.
*   **`store` scope.** The core runs one SPARQL `SELECT` over **all named
    graphs**, in `spawn_blocking`, bounded by the tool deadline. A late
    result returns `reason: "store_search_timeout"`.
    *   SPARQL's `CONTAINS(LCASE(…))` is a pre-filter with the same
        case-insensitive substring rule.
    *   The core then builds a `Candidate` for each row, and keeps and ranks
        it with the shared matcher.
    *   Literals longer than 512 characters are not searched, which keeps
        whole `cbm:sourceCode` bodies out.
    *   The label is picked in the viewer's order: `rdfs:label`, then
        `smc:label`, then `skos:prefLabel`.
    *   The needle is built by `sparql_string_literal`, never by
        concatenating raw text. That helper is moved to `application`, built
        on `cbm::turtle::escape`.

    ```sparql
    SELECT ?g ?s (SAMPLE(?lbl) AS ?label) (SAMPLE(?lit) AS ?matched) WHERE {
      GRAPH ?g {
        ?s ?p ?o .
        FILTER( CONTAINS(LCASE(STR(?s)), LCASE(?needle))
             || ( isLiteral(?o) && STRLEN(STR(?o)) <= 512
                  && CONTAINS(LCASE(STR(?o)), LCASE(?needle)) ) )
        OPTIONAL { ?s rdfs:label ?l1 } OPTIONAL { ?s smc:label ?l2 }
        OPTIONAL { ?s skos:prefLabel ?l3 }
        BIND(COALESCE(?l1, ?l2, ?l3) AS ?lbl)
        BIND(IF(isLiteral(?o), ?o, ?unbound) AS ?lit)
      }
    } GROUP BY ?g ?s LIMIT ?limit
    ```

    `?needle` and `?limit` are substituted by the helper before the query
    runs. `?limit` is 4× the tool's `limit`, which leaves room for re-ranking.
*   **`both` scope.**
    *   The core runs the store query first. It then sends `search_view`
        with `also_check` set to the store-hit IRIs.
    *   The viewer returns its own hits, plus `drawn`: the subset of
        `also_check` present in `ViewerGraph`.
    *   Results are merged by bare IRI. `in_view` is true for an IRI that the
        view search returned, or that is in `drawn`.
    *   `limit` applies after the merge. View hits come first, in their
        ranking, then store hits (active graph first, then by IRI).
*   **Other cases.**
    *   For `scope: "store"`, the core sends `search_view` with `limit: 0`
        and `also_check` only.
    *   With no viewer connected, every hit has `in_view: false`.
    *   A store hit in the active graph can still be `in_view: false` if the
        viewer has not applied the write yet, or if its subject is a blank
        node.

### Viewer-side agent state (`agent` module)

*   **Message flow.** The connection thread decodes `request` lines (a new
    `Decoded::Request` variant of the viewer's parsed-line enum) into
    `ViewerMsg::Request(AgentRequest)`. These follow the existing `Inbox` →
    `FrameMessages` path. Responses go back through an mpsc sender held in an
    `AgentOutbox` resource, and the connection thread's writer task drains it
    into the socket.
*   **`answer_core_requests`.** `CorePlugin` adds this system from
    `agent.rs`, so it runs in both windowed and headless modes. It answers
    `status` and `search_view` from `ViewerGraph`.
*   **`handle_agent_requests`.** `AgentPlugin` adds this system, in windowed
    mode only. It answers the other ops.
*   **Scheduling.** Both systems run in `CoreSet::Graph`, after
    `apply_to_graph` and before `CoreSet::Sync`. A request therefore sees the
    graph as of the same frame, and its material changes render in that
    frame. The viewer's system sets run Net → Graph → Sync → Ui.
*   **Answer timing.** Every op answers in the frame it arrives, except
    `screenshot` (see below).
*   **`AgentHighlight { terms: HashSet<String>, mode: Highlight | Isolate }`.**
    Terms are stored in `"<iri>"` N-Triples form. Keying by term, not
    `NodeId`, lets the highlight survive snapshot rebuilds, as `Selection`
    does.
*   **Rendering.**
    *   **New material.** `agent_highlight_material`, emissive cyan: base
        colour `srgb(0.2, 0.95, 1.0)`, emissive
        `LinearRgba::rgb(0.3, 2.2, 2.6)`.
    *   **Precedence.** A pure function decides it:
        `node_material_choice(selected, highlighted, filtered_out, isolate,
        pulsing, kind) -> MaterialChoice`. The order is: selected >
        agent-highlighted > faded (filtered out, or not highlighted in
        `Isolate`) > last-batch pulse (the brief glow on nodes touched by the
        latest write) > kind.
    *   **Edges.** `update_edge_meshes` reads `Option<Res<AgentHighlight>>`,
        and also rebuilds when the highlight `is_changed()`. Under
        `Isolate`, an edge is faded when either end is filtered out, or when
        its two ends are not both highlighted.
*   **Filter and selection** write the existing `ClassFilter` and `Selection`
    resources. The agent's changes are therefore what the user would see after
    clicking, and the user can undo them with the same panels.
*   **Activity feed.** A new `FeedKind::Agent(AgentFeedOp)` entry is pushed per
    applied command and rendered as `agent · <op> <count>[ (<mode>)]`. While an
    agent highlight or a restore point exists, the ingestion panel shows a
    small "Agent view · Restore my view" chip. It reuses
    `interact::button_node`, and carries `Hovered` so
    `block_controller_over_ui` sees it.

### Camera animation

Bevy 0.20's `PanOrbitCamera` is anchor-based and reads the camera `Transform`
each frame. Its `LookTo` extension only rotates about the anchor.

*   **`CameraTween`.** A component on the `SceneCamera` holding the start and
    end `Transform`, the start time, the duration and an ease-in-out curve.
    *   **Inserting it** sets `PanOrbitCamera.current_motion =
        CurrentMotion::Stationary`, which stops any momentum, as `LookTo`
        does.
    *   **The tween system.** It runs in `PreUpdate`, after the camera input
        chain (`PanOrbitCameraInputMessage::receive_messages`) and before
        `PanOrbitCamera::update_camera_positions`. It interpolates position
        (lerp) and rotation (slerp), and removes the component when done.
    *   **User input wins.** If `current_motion.is_user_controlled()` is true
        that frame, the system removes the tween without writing `Transform`.
*   **Look-at point.** `PanOrbitCamera` stores no pivot, so the viewer keeps a
    `CameraFocus { look_at: Option<Vec3> }` resource.
    *   `frame`, `frame_all`, `F` and a restore set it to the framed centre.
        Any user camera motion clears it.
    *   When it is empty, `look_at = position + forward * |last_anchor_depth|`.
    *   `distance` is the camera-to-`look_at` distance, and `zoom` scales it.
    *   A finished tween sets `last_anchor_depth = -distance`, so the next
        user orbit pivots on the same point.
*   **Targets.** `frame` and `frame_all` reuse `bounding_sphere` and
    `framing_distance`, and keep the current viewing direction, as `F` does.
    `frame_target` takes positions from `LayoutState` for the resolved node
    ids: a snapshot of where the nodes are on that frame.
*   **The `F` key** is rewritten on `frame_target` with duration 0. It removes
    any running `CameraTween`.

### User takeover and restore

The agent never locks the UI. User camera input always takes precedence over
agent camera moves. The user can undo any agent change. For filter and
selection, the viewer reports user activity, and the agent decides.

*   **Camera.**
    *   While `PanOrbitCamera.current_motion.is_user_controlled()` is true
        (the user is orbiting, panning or zooming with the wheel), no agent
        command moves the camera. This covers `camera`, `show` with
        `focus: true`, and `viewer_search` with `show`.
    *   `is_actively_controlled()` is not used, because it ignores zoom-only
        motion.
    *   The non-camera part of a command still applies (see the envelope).
*   **Restore.** A one-level `UserViewCheckpoint` saves the user's camera pose,
    `ClassFilter` and `Selection` (keyed by IRI).
    *   **When it is taken.** Before the first agent command that changes the
        camera, filter or selection after any user input.
    *   **What restores it.** The chip's "Restore my view" button, the second
        `Esc` (pressed with nothing selected), or `viewer_clear` with
        `restore: true`. Each one clears the agent highlight, then returns to
        the saved state with an animated move. The checkpoint is consumed.
*   **`Esc`.** One system, `escape_chain`, replaces
    `clear_selection_on_escape`, so one press has exactly one effect:
    *   with a node selected, `Esc` clears the selection;
    *   otherwise, it restores the user's view if a checkpoint exists;
    *   otherwise, it clears the agent highlight.
*   **Feedback.** A `UserActivityTracker` resource holds:
    *   `last_input`: the time of the last mouse button, key, wheel or
        pointer-move event over the window;
    *   `epoch`: a counter incremented by every user change to the camera,
        filter or selection. That means a node click, `Esc`, a filter-panel
        button, `F`, or any frame where `current_motion.is_user_controlled()`;
    *   `overrode`: set by those same changes, and reset by each applied
        agent command.

    Agent writes to `ClassFilter` and `Selection` do not touch the tracker.
*   **The `user` block.** Each response carries:
    *   `user.active`: `last_input` in the last 3 s;
    *   `user.epoch`;
    *   `user.overrode_last_agent_command`.

    All three are computed before the current request is applied.
*   **Stale-view guard.** A mutating request may carry `if_user_epoch`. If the
    epoch has moved on, the request is refused with `reason: "user_changed"`,
    and nothing changes.

### Screenshot

*   **Capture.** The viewer spawns `Screenshot::primary_window()` with an
    observer on `ScreenshotCaptured`. Capture is asynchronous, so the observer
    keeps the request id and sends the response itself. If a `CameraTween` is
    running, the capture waits for it to end, within the tool deadline.
*   **Encoding.**
    *   The captured `Image` is converted with `Image::try_into_dynamic()`,
        then `to_rgb8()`. Alpha is dropped: the camera is HDR, and in HDR the
        alpha channel stores brightness, as Bevy's `save_to_disk` notes.
    *   The image is downscaled so its longest side is at most `max_side`,
        then encoded as PNG.
*   **Size budget.**
    *   If the PNG exceeds 3.5 MiB (about 4.7 MiB in base64), the viewer
        re-encodes at 1280 px, then 1024 px.
    *   If it is still too large, it answers
        `ok: false, code: "payload_too_large"` at once.
    *   The response reports `width`, `height` and `bytes`.
*   **Unavailable window.** If the primary window is minimized or has zero
    size, the viewer answers `ok: false, code: "window_not_visible"` at once.
*   **Content.** UI panels and labels are in the image, so the agent can read
    node names from it. The core turns the image into MCP image content
    (`Content::image(base64, "image/png")`).

### Headless mode

`--headless` is the viewer's window-less test mode. `CorePlugin` inserts
`AgentOutbox` and adds `answer_core_requests`.

*   **`status`.** It returns `graph`, `nodes`, `edges`, `connection` and
    `instance`, with `camera: null`, `visible: null` and `selection: null`.
*   **`search_view`** is answered in full.
*   **Every other op** is refused with `unsupported_headless`.

This keeps the end-to-end test free of a window, while still covering the full
request/response path.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: The viewer is its own MCP server

The viewer would run an `rmcp` Streamable-HTTP server on loopback, and Claude
Code would connect to it as a second MCP server.

*   **Pros:** The viewer can be controlled without `sparql-mcp-core`, and the
    side-channel does not change.
*   **Cons:**
    *   It adds a second MCP server per project, and a second port for the
        viewer to bind.
    *   The tools vanish or fail whenever the window is closed, which goes
        against the no-op goal.
    *   The agent combines two servers to go from query to display, and store
        search needs a new path to the store.
*   **Rejected:** It adds a server and a port to configure, and fails when the
    window is closed.

### Alternative B: Core-side tools relayed over the side-channel (chosen)

*   **Pros:** One MCP server. It reuses the existing connection, store access
    and no-op semantics.
*   **Cons:** Protocol changes in two crates. Control needs
    `sparql-mcp-core` running.

### Alternative C: A separate control port opened by the viewer

*   **Pros:** Control and event traffic are isolated on separate sockets.
*   **Cons:** The viewer binds a second port. The core must discover that port,
    and two connections must be kept consistent.
*   **Rejected.**

### Alternative D: Commands as triples in a control graph

*   **Pros:** No protocol change. Commands are auditable as triples.
*   **Cons:**
    *   There is no response path: no search results, no `missing`, no
        screenshot.
    *   It pollutes the store, and every camera move becomes a write.
*   **Rejected.**

### Alternative E: A separate agent layer for filter and selection

*   **Pros:** The user's own settings are never overwritten.
*   **Cons:** Two filters and two selections that the user cannot tell apart.
*   **Partly adopted.** Only the highlight is agent-owned. The user's state is
    protected by the restore checkpoint and `if_user_epoch` instead.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Latency.** A command is visible within one frame of its arrival. Round
    trip on loopback targets under 100 ms, excluding animation and screenshot.
    A timing assertion in the core integration test measures it.
    *   This assumes write tools run off the async workers (write-path
        hardening, F9).
    *   Until then, a `viewer_*` call made during a long write may wait for
        it, still bounded by the tool deadline.
*   **Timeouts.** Each `viewer_*` call has one total deadline: 5 s, or 12 s for
    `viewer_screenshot`.
    *   The deadline covers store work, every viewer request, the
        re-registration wait and the retry.
    *   A retry uses the time left, not a fresh budget.
    *   No tool call takes longer than 12 s.
*   **No impact on the write path.** Control has its own channels and never
    touches `BroadcastEventSink`.
*   **Payload bounds.**
    *   At most 5000 IRIs per `show`.
    *   The core checks the serialized request size before sending. A request
        over 1 MiB returns `applied: false` with
        `reason: "request_too_large"`.
    *   A response is at most 8 MiB, and a screenshot PNG at most 3.5 MiB
        before base64.
*   **Security and privacy.**
    *   The listener stays loopback-only.
    *   A local process that connects with `control: 1` and the newest
        `instance` can answer the agent's `viewer_*` calls, for example with
        forged hits or images. `viewer_status.instance` makes such a switch
        visible. This is the existing local-user trust boundary.
    *   The core treats viewer responses as untrusted data:
        *   it checks that a screenshot decodes as a PNG within `max_side`;
        *   it caps hit counts at `limit`, and strings at 1 KiB;
        *   tool descriptions present labels as data, not instructions.
    *   Search text reaches SPARQL only through the escaping helper.
    *   No op takes a file path.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Partial or oversized lines on a bidirectional socket | Lost snapshots or responses, a wedged connection | Cancel-safe capped `FramedRead`; separate reader and writer tasks |
| Resync during a request (`changed`, `Lagged`) | Lost or duplicated commands | In-band resync, idempotent-only retry, a fresh id per retry |
| Request overtakes a just-emitted write | False `missing` | `biased` writer drains events before requests |
| Store search scans large literals | Slow tool | `spawn_blocking`, 512-character literal cap, tool deadline |
| Screenshot too large, or window minimized | Silent timeout, image rejected downstream | 3.5 MiB budget with downscale; `payload_too_large`; `window_not_visible` |
| Control silently moves to another viewer or process | Agent drives the wrong window | Sticky `instance` id, reported in `viewer_status` |
| Agent overwrites user changes it has not seen | User loses their view | `if_user_epoch`; restore checkpoint; `Esc` |
| Local process answers with forged content | Misleading hits or images | Accepted (local-user trust boundary); core validates PNG and caps strings |

## Future Improvements

*   **Camera `orbit` and `reset`, and a standalone `viewer_select`.** These
    were cut from v1 by the author after the review. `viewer_show` with
    `select` covers selection.
*   **Saved views.** Named camera, filter and highlight presets.
*   **Follow mode.** The camera frames each new ingestion batch.
*   **Agent control of labels and layout.**
*   **Annotated screenshots.** Numbered markers in the image, so the agent can
    refer to nodes without reading labels.
*   **Choosing among several viewers**, by `instance`.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Required changes to the companion designs

1.  **[`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md),
    wire protocol.** Add:
    *   the `control` and `instance` handshake fields;
    *   the `request` and `response` line kinds and their key order;
    *   the line caps and the capped reader;
    *   the rule that only controllable connections receive requests.
2.  **[`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md),
    connection handling.** `handle_connection` gets separate reader and
    writer tasks. The writer is `biased` toward store events.
3.  **[`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md).**
    *   Under Non-Goals, add a pointer to this document for agent-driven
        search.
    *   The material precedence gains the agent highlight.
    *   `Esc` becomes `escape_chain`, which includes restore.
4.  **Both, in-band resync.** A controllable connection resyncs over the
    same connection:
    *   after `{"event":"changed"}`, the viewer sends `{"cmd":"snapshot"}`
        mid-stream, and the core answers it (resubscribe, then snapshot);
    *   on `Lagged`, the core re-snapshots instead of closing.

    This promotes the viewer design's *Resnapshot on the same connection*
    future item. Old viewers keep reconnecting.

### Delivery phases

Each phase starts only when the previous phase's exit criterion passes.

| Phase | Scope | Exit criterion |
|---|---|---|
| 1 | Protocol, handshake, reader and writer tasks, in-band resync, `ViewerHub`, `ViewerControl`, `viewer_status`, envelope, headless `status` | Core integration test with a fake TCP viewer covers: status round trip; `not_connected` with no viewer; timeout giving `applied: null`; drop, then reconnect of the same instance, then retry; `changed` and a forced `Lagged` that do not close a controllable connection; an insert followed at once by `show` of the same IRI giving `missing: []`; a large response read while the core writes a large snapshot, with no deadlock. Old-viewer (core test) and old-core (a `connection.rs` unit test against a fake server that ignores `control`) compatibility tests pass. |
| 2 | `AgentHighlight`, `viewer_show` (with `select`), `viewer_clear`, `viewer_filter`, materials, `escape_chain`, restore checkpoint, feed entries, chip | Manual CUJ 1 with `focus: false`, CUJ 6, and the restore half of CUJ 3. Unit tests for IRI resolution, `node_material_choice`, and `if_user_epoch`. |
| 3 | `CameraTween`, `CameraFocus`, `viewer_camera`, focus on `show`, user takeover, `F` refactor | Manual CUJ 1 with `focus: true`, and CUJ 3 with the wheel. Unit tests for subset framing and zoom pose math. |
| 4 | `text_match`, `viewer_search` (`view`, `store`, `both`), `sparql_string_literal` | Manual CUJ 2. Unit tests for matching and ranking (shared by both sides) and for escaping (quotes, backslashes, newlines). Headless e2e covers `search_view`. |
| 5 | `viewer_screenshot` | Manual CUJ 4. On a label-dense 5000-node scene at 1568 px with panels, the PNG is under 3.5 MiB, or is downscaled to fit. An oversized capture returns `payload_too_large`, not a timeout. |
| 6 | Docs and skill guidance | README tool list; `plugins/kb-workbench/skills/kb-workbench/SKILL.md` and `plugins/kb-workbench/skills/sparql-first-code/SKILL.md` mention `query_sparql` → `viewer_show` and `ingest_commit` → `viewer_show`. |

### Implementation status

**Phase 1 is done** (the core integration test, `crates/sparql-mcp-core/tests/viewer_control.rs`,
and a headless end-to-end test that starts the real viewer binary). Phases 2 to 6 are not started:
the tools are `viewer_status` only, and the viewer answers `status` and refuses every other op
with `unknown_op`.

What phase 1 did, and where it differs from the plan above:

*   **Core.** `application/viewer_control.rs` (types, `ViewerControl`, `NullViewerControl`),
    `ViewerHub` and the reader and writer tasks in `infrastructure/viewer_channel.rs`,
    `application/tools/viewer.rs` (`viewer_status`, the envelope), `with_viewer_control` on the
    server, wiring in `main.rs`. `HubInfo` (listener state, controlled instance, connection count)
    is added to the trait so `viewer_status` can say why no viewer connects.
*   **Resync echo (not in the plan).** The core adds `"control":1` to the snapshot of a controllable
    connection. A new viewer resynchronises on the same connection only if it sees this echo;
    against an older core, which never echoes, it reconnects after `changed` as before. Without the
    echo a new viewer against an old core would ask in-band and never get an answer.
*   **`FramedRead` quirk.** After an error (a line over the cap) `FramedRead` yields one `None` and
    then reads on. The reader must not take that `None` for the end of the connection; a test sends
    a 9 MiB line and checks that control still works afterwards.
*   **Calling a disconnected controlled viewer.** With nothing connected the answer is
    `not_connected` at once; if the controlled instance is gone but another is connected, that one
    takes over. Waiting for a reconnect happens only after a request was lost in flight, and only for
    an idempotent op.
*   **Viewer.** `connection.rs` has `spawn_controllable` (instance id, `control:1`, a response
    sender) next to the old `spawn`, which does not offer control. `agent.rs` holds `AgentOutbox`,
    `answer_core_requests` and the pure `answer`/`status_snapshot`; `CorePlugin` adds them, so the
    headless path answers too. `user` is a constant "no activity" block until the user is tracked
    (phase 3).
*   **Compatibility.** The three existing channel tests, which send the plain handshake, pass
    unchanged. Old core: four tests in `connection.rs` (the plain `spawn` sends the old first line;
    a controllable viewer against a core without the echo still reconnects on `changed`).
*   **Not covered by an automated test:** a real `Lagged` against a *viewer-side* process (the core's
    handling is tested with a fake viewer), and the previous release's viewer binary against the new
    core (the plain handshake test stands in for it).

### Target Files to Create/Modify

#### 1. `crates/sparql-mcp-core/src/application/viewer_control.rs` (Create)

*   `ViewerOp`, with `#[serde(tag = "op", content = "args", rename_all =
    "snake_case")]`.
*   `UserActivity`, `ViewerOutcome`, the `ViewerControl` trait, and
    `NullViewerControl`, which always returns `NotConnected`.

This file does no I/O.

#### 2. `crates/sparql-mcp-core/src/application/text_match.rs` (Create)

`normalize`, `Candidate`, `MatchKind`, `match_candidate` and `rank`. It is pure
and unit-tested, and is used by the core and the viewer.

#### 3. `crates/sparql-mcp-core/src/infrastructure/viewer_channel.rs` and `infrastructure/mod.rs` (Modify)

*   `ViewerHub`: the instance slots, the id counter, `pending` with drop
    guards, the retry rule, the bind status, and `impl ViewerControl`.
    `infrastructure/mod.rs` re-exports it.
*   `handle_connection`:
    *   parses `control` and `instance`, and registers before the snapshot;
    *   splits into a reader task (`FramedRead<_, LinesCodec>` capped at
        8 MiB) and a `biased` writer task;
    *   answers a mid-stream snapshot command, and re-snapshots on
        `Lagged`;
    *   deregisters on exit.
*   The core's `Cargo.toml` declares the tokio `time` feature explicitly.

#### 4. `crates/sparql-mcp-core/src/application/tools/viewer.rs` (Create)

*   Entry points:
    *   `pub fn tool_defs() -> Vec<Tool>`;
    *   `pub async fn dispatch(name: &str, args: &JsonObject, ctl: &dyn
        ViewerControl, store: &Arc<dyn SparqlStore>, active_graph:
        &Arc<RwLock<String>>) -> Option<Result<CallToolResult, McpError>>`.
*   Argument validation (`invalid_params`), IRI normalization, the envelope,
    and the deadline.
*   The store search, ranked with `text_match`, using `sparql_string_literal`.
    That helper is moved to `application`, on top of `cbm::turtle::escape`,
    and `importer::escape_literal` then calls it.
*   Merging for `both`, chaining for `search … show`, and screenshot
    validation.

#### 5. `crates/sparql-mcp-core/src/mcp/server.rs` (Modify)

*   Add a `viewer: Arc<dyn ViewerControl>` field, defaulting to
    `NullViewerControl` as `event_sink` defaults to `NullEventSink`, plus
    `viewer_tools_enabled: bool` and `with_viewer_control`.
*   `list_tools` adds `viewer::tool_defs()` when the flag is set.
*   `call_tool` awaits `viewer::dispatch` before the core dispatch.
*   Extend `every_declared_tool_has_a_dispatch_arm`, and add one sentence to
    the `instructions` text.

#### 6. `crates/sparql-mcp-core/src/main.rs` (Modify)

When `[viewer]` is enabled and `bind` parses:

*   create one `Arc<ViewerHub>`;
*   pass it to `spawn_viewer_channel`, keeping the `JoinHandle` and recording
    a bind error in the hub;
*   call `with_viewer_control(hub)`.

#### 7. `crates/sparql-mcp-core/tests/viewer_control.rs` (Create)

Covers the Phase 1 exit criteria, using a fake viewer over
`serve_viewer_channel`. It reuses `listen_ephemeral`,
`connect_and_request_snapshot` and `recv_json` from
`tests/viewer_channel.rs`.

#### 8. `crates/sparql-mcp-viewer/src/connection.rs` (Modify)

*   Send `"control":1` and `instance` in the handshake.
*   Replace `read_line` with `FramedRead<_, LinesCodec>`, read in a reader
    task.
*   Add `Decoded::Request` and `ViewerMsg::Request(AgentRequest)`.
    `AgentRequest` derives `Debug` and `Clone`.
*   On `changed`, request a snapshot in-band instead of reconnecting.
*   `spawn` also returns a `tokio::sync::mpsc::UnboundedSender<AgentResponse>`.
    A writer task drains it into `write_half`.

`connection.rs` stays Bevy-free.

#### 9. `crates/sparql-mcp-viewer/src/agent.rs` (Create)

*   `AgentPlugin`, `AgentHighlight`, `AgentOutbox`, `UserActivityTracker`,
    `UserViewCheckpoint`.
*   The systems `answer_core_requests` and `handle_agent_requests`, and the
    screenshot observer.
*   Pure functions: `resolve_iris`, `search_graph` (on `text_match`), and
    `status_snapshot`.

#### 10. `crates/sparql-mcp-viewer/src/camera.rs` (Modify)

*   `CameraTween` and its `PreUpdate` system, ordered after the input chain.
*   `CameraFocus` and `frame_target`, with the zoom pose function.
*   Cancel the tween on `current_motion.is_user_controlled()`.
*   Rewrite `F` on `frame_target`.

#### 11. `crates/sparql-mcp-viewer/src/render.rs` (Modify)

*   `agent_highlight_material` in `ViewerStyle`.
*   `node_material_choice`, used by `apply_node_materials`.
*   `update_edge_meshes` reads `AgentHighlight`, and rebuilds when it
    changes.

#### 12. `crates/sparql-mcp-viewer/src/interact.rs` and `src/ingest.rs` (Modify)

*   `escape_chain`, replacing `clear_selection_on_escape`.
*   Tracker updates on user filter and selection changes.
*   `FeedKind::Agent(AgentFeedOp)`, and the chip.

#### 13. `crates/sparql-mcp-viewer/src/plugin.rs`, `src/headless.rs`, `src/lib.rs` (Modify)

*   `CorePlugin` inserts `AgentOutbox` and adds `answer_core_requests`.
*   `ViewerPlugin` adds `AgentPlugin`.
*   Declare `pub mod agent`.

#### 14. `crates/sparql-mcp-viewer/Cargo.toml` (Modify)

*   Add:
    *   `image = { version = "0.25", default-features = false, features =
        ["png"] }`, the same major version as `bevy_image`;
    *   `base64 = "0.23"`;
    *   `tokio-util = { version = "0.7", features = ["codec"] }`;
    *   `futures-util`.
*   Bevy's `png` feature is not needed.
*   The `rmcp` dev-dependency gains the `client` feature.

#### 15. `crates/sparql-mcp-viewer/tests/headless_e2e.rs` (Modify)

1.  Build `SparqlMcpServer` in-process with `with_viewer_control(hub)`, serve
    it over `tokio::io::duplex`, and connect an `rmcp` client to the other
    half.
2.  Run `serve_viewer_channel` on an ephemeral port with the same hub.
3.  Start the headless viewer as a child process with
    `env!("CARGO_BIN_EXE_sparql-mcp-viewer") --headless --connect
    127.0.0.1:<port>`.
4.  Assert the `viewer_status` counts and a `viewer_search` `scope: view`
    hit.
5.  Kill the child.

#### 16. Call sites broken by the signature and enum changes (Modify)

*   Pass an `Arc<ViewerHub>` in:
    *   `crates/sparql-mcp-core/tests/viewer_channel.rs` (lines 57, 124, 164);
    *   `crates/sparql-mcp-core/examples/viewer_load.rs` (line 76);
    *   `crates/sparql-mcp-viewer/tests/headless_e2e.rs` (line 75).
*   Add a `ViewerMsg::Request(_)` arm to the exhaustive matches:
    *   `plugin.rs::apply_to_graph`;
    *   `ingest.rs::feed_entry_for` and `record_feed`;
    *   `headless.rs::print_messages`;
    *   `tests/headless_e2e.rs::drain_until`.

#### 17. Docs (Modify)

`README.md` (MCP tools), `crates/sparql-mcp-viewer/README.md` (agent control,
`Esc`, the chip), `CHANGELOG.md`, and the two SKILL.md files named in Phase 6.

### Verification Plan

*   `cargo build --workspace`, `cargo test --workspace`,
    `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
*   The core integration test (file 7) and the headless e2e (file 15) run in
    CI.
*   Manual runs of CUJs 1–6 on the author's machine, with core and viewer both
    native on Windows and Claude Code driving. CUJ 5 is checked by closing the
    viewer mid-session.
*   Compatibility: run the previous release's viewer against the new core,
    and the new viewer against the previous core. Both stream normally, and
    the new core reports `not_connected` for the old viewer.
