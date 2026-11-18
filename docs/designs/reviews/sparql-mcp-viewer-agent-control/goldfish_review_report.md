# Goldfish Annotated Review Report

Status: resolved (2026-11-18). All PROPOSE FIX applied in the rewritten target; ESCALATE items decided by the author (see the target's Alternatives and Future Improvements); DECLINE/CONTEST items left unchanged.

**Proposed outcome:** 46 PROPOSE FIX · 0 CONTEST · 3 ESCALATE · 1 DECLINE.

**Target:** `docs/designs/sparql-mcp-viewer-agent-control.md` (full file, lines 1–674)

**Source:** `consolidated_findings_report.md` (same folder), Round 1. The IDs B1–B4, M1–M22 and N1–N24 refer to the consolidated findings in that report. It holds the full discrepancy text, the raw reviewer IDs and the traceability matrix.

## Method

Each finding was checked against the ground truth below before a suggestion was given:

*   the `GoldfishContext` (`context/ground-truth.md`);
*   the companion designs and the batch-ingestion sibling;
*   the core and viewer source;
*   the Bevy 0.20.1, tokio 1.52.1 and rmcp 1.5.0 registry sources.

The synthesizer re-checked these load-bearing claims directly:

*   `read_line` inside `select!`;
*   `is_actively_controlled` and zoom-only motion;
*   literal-only subjects are drawn (graph.rs:215);
*   HDR alpha (camera.rs:46, screenshot.rs:198);
*   headless mode uses `CorePlugin` only;
*   set ordering (plugin.rs:64-67);
*   `Changed` followed by `viewer_show` in the batch-ingestion design;
*   the viewer CLI flag `--connect` (viewer main.rs:22-23).

## Ground-truth guardrails

None of the proposed edits re-opens the five author decisions:

1.  Alternative B is kept. The viewer is not an MCP server.
2.  The user can always take back control.
3.  Search covers both the view and the store.
4.  Screenshot is in v1.
5.  With no viewer connected, every tool is a no-op success.

Findings that would narrow scope, or that change *how* a decision is realised in a way the author may not want, are marked **ESCALATE** (M20, M21, M22) rather than proposed as fixes.

## Editing note

Line numbers refer to the current target. Several fixes touch the same paragraphs, and the overlaps are flagged where they occur:

*   B3 / M1 / M3 / M18 all touch lines 261–274;
*   M4 / M5 touch lines 289–298.

Apply them together.

## Prioritized Findings & AI Suggestions

### 🔴 B1 [`egm-design`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 234-236](../../sparql-mcp-viewer-agent-control.md#L234-L236), [575-577](../../sparql-mcp-viewer-agent-control.md#L575-L577), [613-616](../../sparql-mcp-viewer-agent-control.md#L613-L616), [645-649](../../sparql-mcp-viewer-agent-control.md#L645-L649).
*   **Discrepancy Description:** The new `select!` arms would poll `read_line`, which is not cancel-safe, so partial lines are lost. `read_line` also has no length cap, so the stated line caps cannot be built on it.
*   **Rationale:**
    *   tokio-1.52.1 `async_buf_read_ext.rs:130-138` documents the data loss.
    *   Both loops use `read_line` today (viewer_channel.rs:77, connection.rs:140).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The synthesizer verified it. The fix uses a dependency the core already has (`tokio-util` with `codec`, core Cargo.toml:22).
*   ***Proposed Edit:***
    *   **Location:** Lines 234–236. Replace the line-cap bullet with:
        ```
        *   Request lines are capped at 1 MiB and response lines at 8 MiB.
            The core enforces the request cap before sending (see *Payload
            bounds*). Both sides read lines with a cancel-safe, length-capped
            reader: `tokio_util::codec::FramedRead` over
            `LinesCodec::new_with_max_length(cap)`, polled with
            `StreamExt::next()`. `read_line` is never polled inside `select!`.
            On `MaxLineLengthExceeded` the reader discards the line, logs a
            warning and keeps reading. The waiting request then times out.
            The core's cap is 8 MiB. The viewer's reader is uncapped, because
            snapshot lines carry a whole graph.
        ```
    *   **Location:** Line 577 (file 2). Old: `enforces line caps, and deregisters on exit.` New: `reads through a FramedRead<_, LinesCodec> capped at 8 MiB in its reader task (see the single-writer rule), and deregisters on exit.`
    *   **Location:** Lines 613–616 (file 8). Replace with:
        ```
        Send `"control":1` (and `instance`, see *Target viewer*) in the
        handshake. Replace `read_line` with `FramedRead<_, LinesCodec>` read
        in a reader task. Add `Decoded::Request` and
        `ViewerMsg::Request(AgentRequest)`. `spawn` also returns a plain
        `tokio::sync::mpsc::UnboundedSender<AgentResponse>`; a writer task
        drains it into `write_half`. `connection.rs` stays Bevy-free.
        ```
    *   **Location:** File 14. Covered by M13.

### 🔴 B2 [`egm-design`] - Severity: Blocker
*   **Affected Scope:** [Lines 275-278](../../sparql-mcp-viewer-agent-control.md#L275-L278), [346-348](../../sparql-mcp-viewer-agent-control.md#L346-L348), [604-609](../../sparql-mcp-viewer-agent-control.md#L604-L609).
*   **Discrepancy Description:** Each side reads and writes in one task. A large write in each direction at the same time fills both TCP buffers, and the connection deadlocks.
*   **Rationale:**
    *   `send_snapshot` writes a whole graph in one line (viewer_channel.rs:168).
    *   The viewer would write an up-to-8 MiB screenshot inside a branch body of its read loop.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. This is a standard full-duplex hazard, and nothing in the design prevents it. Only one reviewer raised it, but the mechanism is concrete, so it stays a Blocker.
*   ***Proposed Edit:***
    *   **Location:** Lines 275–278. Replace the "Single writer" bullet with:
        ```
        *   **Separate reader and writer tasks.** `handle_connection` splits
            the socket. A writer task owns `write_half` and runs a `select!`
            over resync ticks, store events and outbound requests from the
            hub, so store events and requests never interleave inside one
            line. A reader task reads `read_half` and routes each `response`
            to its `oneshot` in `pending`, and forwards a mid-stream
            `{"cmd":"snapshot"}` to the writer (see *Resync without
            reconnect*). Neither task waits on the other, so a large write in
            one direction never blocks reading in the other.
        ```
    *   **Location:** Lines 346–348. Old: `Responses go back through an mpsc sender held in an AgentOutbox resource. The connection thread writes them to the socket.` New: `Responses go back through an mpsc sender held in an AgentOutbox resource. On the connection thread, a reader task feeds Inbox and a separate writer task drains the response channel into the socket.`
    *   **Location:** Line 609 (file 7 test list). Append: `; a large response read while the core is writing a large snapshot (no deadlock).`

### 🔴 B3 [`egm-zoom-out`, `egm-inventive-catalyst`, `egm-design`, `egm-codebase-details`, `egm-comprehension`] - Severity: Blocker
*   **Affected Scope:** [Lines 36-39](../../sparql-mcp-viewer-agent-control.md#L36-L39), [270-274](../../sparql-mcp-viewer-agent-control.md#L270-L274), [533-544](../../sparql-mcp-viewer-agent-control.md#L533-L544), [553](../../sparql-mcp-viewer-agent-control.md#L553).
*   **Discrepancy Description:** The viewer reconnects on every `changed` (connection.rs:147-155). The sibling design's flagship flow, `ingest_commit` above `event_changed_threshold` followed by `viewer_show`, therefore always lands in the reconnect window. The 2 s wait plus one retry only patches the symptom.
*   **Rationale:**
    *   batch-ingestion.md:297-305 and 455 describe that flow.
    *   The core already re-snapshots in-band on an active-graph change (viewer_channel.rs:100-108).
    *   `sparql-mcp-viewer.md:500` already lists "Resnapshot on the same connection" as a future option.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid and verified.
    *   It does not touch the author decisions: Alternative B's side-channel is kept and only its resync rule changes.
    *   It goes through *Required changes to the companion designs*, as the target already requires for protocol edits (lines 41–43).
    *   It removes most of the hub's reconnect machinery, which also shrinks M1 and M3.
*   ***Proposed Edit:***
    *   **Location:** Lines 36–39. Replace with:
        ```
        Agent control and agent-driven batch ingestion meet at commit time.
        After `ingest_commit`, the agent calls `viewer_show` on the new
        subjects. A commit above `event_changed_threshold` will emit one
        `Changed` event (per the batch-ingestion design). A controllable
        viewer resyncs on that event in-band, on the same connection (see
        *Resync without reconnect*), so the following `viewer_show` is
        applied to the new snapshot instead of being lost to a reconnect.
        ```
    *   **Location:** Lines 270–274. Replace the "Reconnects" bullet with two bullets:
        ```
        *   **Resync without reconnect.** A controllable viewer
            (`control >= 1`) does not reconnect on `changed` or `Lagged`. On
            `changed` it sends `{"cmd":"snapshot"}` on the open connection;
            on `Lagged` the core resubscribes and pushes a fresh snapshot
            instead of closing the socket. Both reuse the path the core
            already follows after `project_switch`: resubscribe, then
            snapshot. Requests in flight, and their ids, are unaffected. A
            viewer that did not send `control` keeps the base behaviour.
        *   **Real disconnects.** `handle_connection` registers with the hub
            right after parsing the handshake, before sending the snapshot.
            When the controlled connection closes (EOF, crash, window
            closed), its pending requests resolve as `NotConnected`.
            `request` then waits, within the tool deadline (see *Timeouts*),
            for the same viewer instance to register again, and retries once
            if the op is idempotent (see *Retry rule*).
        ```
    *   **Location:** After line 544, add item 4:
        ```
        4.  **`interface-agentic-rust-stylee.md` and `sparql-mcp-viewer.md`,
            in-band resync.** A controllable connection resyncs in-band.
            After `{"event":"changed"}` the viewer sends `{"cmd":"snapshot"}`
            on the same connection, and the core answers it mid-stream
            (resubscribe, then snapshot), as it already does on an
            active-graph change. On `Lagged` the core resubscribes and
            re-snapshots instead of closing. This promotes the viewer
            design's *Resnapshot on the same connection* future item. Old
            viewers keep reconnecting.
        ```
    *   **Location:** Line 553 (Phase 1 exit). Append: `A changed event and a forced Lagged do not close a controllable connection, and a request sent across them succeeds.`

### 🔴 B4 [`egm-codebase-details`, `egm-design`, `egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 378-382](../../sparql-mcp-viewer-agent-control.md#L378-L382), [396-401](../../sparql-mcp-viewer-agent-control.md#L396-L401), [626-628](../../sparql-mcp-viewer-agent-control.md#L626-L628).
*   **Discrepancy Description:** `is_actively_controlled()` excludes zoom-only motion, so wheel input never cancels a tween. The tween is not ordered after the input chain, and momentum is not stopped.
*   **Rationale:**
    *   bevy_camera_controller-0.20.1 component.rs:195-203 shows the zoom exclusion.
    *   camera_input.rs:56-68 is the input chain, which runs in `PreUpdate`.
    *   component.rs:375-385 applies momentum after release.
    *   look_to.rs:139-140 shows Bevy's own tween setting `Stationary`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid and verified. As written, the design breaks author decision 2 for the wheel.
*   ***Proposed Edit:***
    *   **Location:** Lines 378–382. Replace with:
        ```
        *   A **`CameraTween`** component (start and end `Transform`, start
            time, duration, ease-in-out) is inserted on the `SceneCamera`.
            Inserting it sets `PanOrbitCamera.current_motion =
            CurrentMotion::Stationary`, which stops any momentum, as Bevy's
            `LookTo` does. A system in `PreUpdate`, ordered after the camera
            input chain (`PanOrbitCameraInputMessage::receive_messages`) and
            before `PanOrbitCamera::update_camera_positions`, interpolates
            position (lerp) and rotation (slerp), and removes the component
            when done. If `current_motion.is_user_controlled()` is true that
            frame, it removes the tween without writing `Transform`.
        ```
    *   **Location:** Lines 396–401. Replace with:
        ```
        *   **Camera.** While `PanOrbitCamera.current_motion
            .is_user_controlled()` is true (the user is orbiting, panning or
            zooming with the wheel), `camera`, `show` with `focus: true` and
            `viewer_search` with `show` do not move the camera.
            `is_actively_controlled()` is not used, because it ignores
            zoom-only motion. The non-camera part of a command still applies
            (see the response envelope). User input during a `CameraTween`
            removes the tween on that frame, before
            `update_camera_positions` runs.
        ```
    *   **Location:** Line 627. Old: `cancelling on is_actively_controlled` New: `cancelling on current_motion.is_user_controlled()`.

### 🟡 M1 [`egm-design`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-inventive-catalyst`] - Severity: Major
*   **Affected Scope:** [Lines 265-274](../../sparql-mcp-viewer-agent-control.md#L265-L274).
*   **Discrepancy Description:** A retry re-applies `orbit` and `zoom`. Stale and unknown response ids are unhandled. A `Timeout` claims `applied:false` even though the command may still be applied.
*   **Rationale:** connection.rs:148-155 forwards the event and then drops the connection, so a request may already be queued or applied when the retry happens.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. B3 removes the most frequent trigger, but real disconnects remain.
*   ***Proposed Edit:***
    *   **Location:** After the bullets from B3 (lines 270–274). Insert:
        ```
        *   **Retry rule.** `pending` is one hub-wide map from id to
            `(connection, oneshot::Sender)`. A response whose id is not in
            `pending` is dropped and logged at debug level. A retry gets a new
            id. Only idempotent ops are retried: `status`, `search_view`,
            `screenshot`, `show`, `clear`, `filter`, `select`, and `camera`
            with `frame`, `frame_all` or `reset`. `camera` with `orbit` or
            `zoom` is relative and is never retried. The viewer discards
            unsent responses when its connection closes. A `Timeout` means
            the outcome is unknown, and the envelope reports
            `"applied": null`.
        ```

### 🟡 M2 [`egm-design`, `egm-inventive-catalyst`] - Severity: Major
*   **Affected Scope:** [Lines 275-278](../../sparql-mcp-viewer-agent-control.md#L275-L278), [604-609](../../sparql-mcp-viewer-agent-control.md#L604-L609).
*   **Discrepancy Description:** An unbiased `select!` can write a request before already-queued store events. The viewer then reports freshly written IRIs as `missing`.
*   **Rationale:** `tokio::select!` picks randomly among ready branches. A broadcast `send` completes before the tool returns.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The `biased` drain is cheaper than the `seq` / `after_seq` token that ic proposed, and it is enough on one ordered TCP stream.
*   ***Proposed Edit:***
    *   **Location:** Append to the writer bullet from B2:
        ```
        The writer's `select!` is `biased;` with the store-event arm first,
        and drains every event already queued (`try_recv`) before writing a
        request. An event emitted before a `viewer_*` call is therefore
        applied by the viewer before that request.
        ```
    *   **Location:** Line 609. Append: `; an insert followed at once by show of the same IRI returns missing: [].`

### 🟡 M3 [`egm-structural`, `egm-style`, `egm-comprehension`, `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-inventive-catalyst`] - Severity: Major
*   **Affected Scope:** [Lines 270-274](../../sparql-mcp-viewer-agent-control.md#L270-L274), [423](../../sparql-mcp-viewer-agent-control.md#L423), [497-499](../../sparql-mcp-viewer-agent-control.md#L497-L499).
*   **Discrepancy Description:** The worst case is 8 s for an ordinary op and 22 s for a screenshot, not "about 15 s". The doc never says whether the timeout is per attempt or per call, and store work is not counted.
*   **Rationale:** The figures follow from the doc's own rules (3/10 s per attempt, plus 2 s, plus one retry).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Seven reviewers raised it independently. A single total deadline is simplest, and it also bounds the store search (M11).
*   ***Proposed Edit:***
    *   **Location:** Lines 497–499. Replace with:
        ```
        *   **Timeouts.** Each `viewer_*` call has one total deadline: 5 s,
            or 12 s for `viewer_screenshot`. It covers store-side work,
            every viewer request the tool makes, the re-registration wait
            and the retry. A retry uses the time left, not a fresh budget.
            A tool call therefore never takes longer than 12 s.
        ```
    *   **Location:** Line 423. Old: `The screenshot timeout is 10 s. Other ops use 3 s.` New: `viewer_screenshot has a 12 s deadline; other tools 5 s (see Timeouts).`
    *   **Location:** Line 272. Old: `waits up to 2 s` New: `waits, within the tool deadline,` (already part of the B3 text).

### 🟡 M4 [`egm-style`, `egm-comprehension`, `egm-design`, `egm-implementation-readiness`, `egm-inventive-catalyst`, `egm-structural`] - Severity: Major
*   **Affected Scope:** [Lines 248-253](../../sparql-mcp-viewer-agent-control.md#L248-L253), [289-298](../../sparql-mcp-viewer-agent-control.md#L289-L298).
*   **Discrepancy Description:** `user_controlling_camera` is listed under `Rejected` / `applied:false`, but the highlight still applies. The envelope lacks `reason`. CUJ 5 omits fields that the envelope says every tool returns.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. This is an internal contradiction, and it would mislead the agent.
*   ***Proposed Edit:***
    *   **Location:** Line 249. Old: `Applied { result: serde_json::Value, user: UserActivity },` New: `Applied { result: serde_json::Value, user: UserActivity, reason: Option<String> },`
    *   **Location:** Lines 289–298. Replace from "Every tool returns" through "raise an MCP `invalid_params` error." with:
        ````
        Every tool returns a JSON text result with this envelope:

        ```text
        {"viewer": "connected" | "not_connected" | "timeout",
         "applied": true | false | null,
         "reason"?: string,
         "result"?: {…},
         "user"?: {"active": bool, "epoch": u64,
                   "overrode_last_agent_command": bool}}
        ```

        `result` and `user` are present only when `viewer` is `connected`.
        With `not_connected`, `applied` is `false`. With `timeout`, it is
        `null`, because the viewer may still apply the command. A
        `Rejected` outcome (`unsupported_headless`, `user_changed`, or a
        well-formed argument the viewer cannot apply) sets
        `applied: false` and adds `reason`. A partly applied command sets
        `applied: true` and adds `reason`. For example, `show` with
        `focus: true` while the user is moving the camera applies the
        highlight, returns `result.camera_moved: false` and
        `reason: "user_controlling_camera"`. `viewer_camera` itself
        returns `applied: false` in that case.
        ````
        The M5 paragraph follows this text.

### 🟡 M5 [`egm-style`, `egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Lines 296-298](../../sparql-mcp-viewer-agent-control.md#L296-L298).
*   **Discrepancy Description:** Nothing defines the boundary between "bad arguments" (soft reject) and "malformed arguments" (`invalid_params`).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The agent's error handling depends on this boundary.
*   ***Proposed Edit:***
    *   **Location:** Immediately after the M4 text. Insert:
        ```
        Malformed arguments fail the tool's JSON schema or a core-side
        limit: a missing required field, an unknown enum value, more than
        5000 IRIs, `factor <= 0`, or `frame` without `iris`. The core
        returns MCP `invalid_params` for these without contacting the
        viewer. Well-formed arguments the viewer cannot apply (for example
        `frame` where no IRI is drawn, or `select` of an IRI that is not
        drawn) return `applied: false` with `reason`.
        ```

### 🟡 M6 [`egm-style`, `egm-comprehension`, `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 120-121](../../sparql-mcp-viewer-agent-control.md#L120-L121).
*   **Discrepancy Description:** CUJ 1's "highlight ring" contradicts the material swap described in Rendering.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The material swap matches the code (render.rs:380-400, one material per node). A ring would need a new entity type for no stated benefit.
*   ***Proposed Edit:***
    *   **Location:** Lines 120–121. Replace with:
        ```
            *   The 14 nodes switch to the emissive cyan agent-highlight
                material. Every other node fades, and so does every edge
                that does not have both ends highlighted. No node moves.
        ```

### 🟡 M7 [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 357-362](../../sparql-mcp-viewer-agent-control.md#L357-L362), [630-633](../../sparql-mcp-viewer-agent-control.md#L630-L633).
*   **Discrepancy Description:** The `update_edge_meshes` dirty check ignores `AgentHighlight`, so Isolate edge fading never refreshes on a settled layout. Material precedence has no pure function for the Phase 2 unit test to call.
*   **Rationale:** render.rs:299-338 contains the dirty check.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   **Location:** Lines 360–362. Replace from "Under `Isolate`" to the end of the bullet with:
        ```
            Precedence is decided by a pure function,
            `node_material_choice(selected, highlighted, filtered_out,
            isolate, pulsing, kind) -> MaterialChoice`, which
            `apply_node_materials` calls and Phase 2 unit-tests. Edge fading
            lives in `update_edge_meshes`. It gains
            `Option<Res<AgentHighlight>>` and also rebuilds when the
            highlight `is_changed()`. Under `Isolate`, an edge is faded when
            either end is filtered out or its two ends are not both
            highlighted, which reuses the existing faded edge meshes.
        ```
    *   **Location:** Lines 632–633 (file 11). Replace with: `` `agent_highlight_material` in `ViewerStyle`. `node_material_choice` and its use in `apply_node_materials`. `update_edge_meshes` reads `AgentHighlight` and rebuilds on its change. ``

### 🟡 M8 [`egm-design`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Line 302](../../sparql-mcp-viewer-agent-control.md#L302), [383-388](../../sparql-mcp-viewer-agent-control.md#L383-L388).
*   **Discrepancy Description:** `look_at`, `distance`, the orbit pivot and the `reset` pose have no source in the code.
*   **Rationale:**
    *   An anchor exists only while the motion is `UserControlled` (component.rs:164-170).
    *   The start pose is a literal in `spawn_camera` (camera.rs:42).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid and verified.
*   ***Proposed Edit:***
    *   **Location:** After line 388. Insert a bullet:
        ```
        *   **Look-at point.** `PanOrbitCamera` stores no pivot, so the
            viewer keeps a `CameraFocus { look_at: Option<Vec3> }` resource.
            `frame`, `frame_all`, `F` and `reset` set it to the framed
            centre. Any user camera motion clears it. When empty,
            `look_at = position + forward * |last_anchor_depth|`. `distance`
            is the camera-to-`look_at` distance. `orbit` and `zoom` use this
            point. A finished tween sets `last_anchor_depth = -distance` so
            the next user orbit pivots on the same point. `reset` returns to
            `START_POSE` (eye `(0, 8, 30)` looking at the origin), which is
            extracted from `spawn_camera` as a constant.
        ```

### 🟡 M9 [`egm-comprehension`, `egm-design`, `egm-inventive-catalyst`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 229-231](../../sparql-mcp-viewer-agent-control.md#L229-L231), [308](../../sparql-mcp-viewer-agent-control.md#L308), [337-340](../../sparql-mcp-viewer-agent-control.md#L337-L340).
*   **Discrepancy Description:**
    *   The doc gives no mechanism for `in_view` on store hits. The `limit`-capped view search misreports drawn hits.
    *   "Literal-only subjects" are drawn, contrary to lines 339–340.
*   **Rationale:** graph.rs:215 creates a node for every subject, and graph.rs:242-248 stores literals as attributes. The synthesizer verified both.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Folding the membership check into `search_view` adds no op and no round trip. If the author adopts M22, this fix becomes a `resolve` step in the core-only search.
*   ***Proposed Edit:***
    *   **Location:** Line 230. Old: `` `search_view`, `screenshot` `` New: `` `search_view` (with optional `also_check`), `screenshot` ``
    *   **Location:** Lines 337–340. Replace with:
        ```
        *   **`both`** runs the store query first, then sends `search_view`
            with `also_check` set to the store-hit IRIs. The viewer returns
            its own hits plus `drawn`, the subset of `also_check` present in
            `ViewerGraph`. Results are merged by bare IRI. `in_view` is true
            for an IRI that the view search returned or that is in `drawn`.
            `limit` applies after the merge: view hits first, in their
            ranking, then store hits (active graph first, then by IRI). For
            `scope: "store"`, the core sends `search_view` with `limit: 0`
            and `also_check` only. With no viewer connected, every hit has
            `in_view: false`. A store hit in the active graph can still have
            `in_view: false` if the viewer has not applied the write yet, or
            if the subject is a blank node.
        ```

### 🟡 M10 [`egm-implementation-readiness`, `egm-codebase-details`, `egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Lines 316-318](../../sparql-mcp-viewer-agent-control.md#L316-L318), [354-356](../../sparql-mcp-viewer-agent-control.md#L354-L356).
*   **Discrepancy Description:** The doc does not say whether IRIs travel bare or as `<…>`. The merge key and the values of `matched_on` are also undefined.
*   **Rationale:** The viewer keys everything by N-Triples `<iri>` (graph.rs:27-34, interact.rs:33, 43). The store returns bare IRIs (viewer_channel.rs:185).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   **Location:** Lines 316–318. Replace with:
        ```
        **IRIs only.** Every node argument is an IRI, given as `http://…`
        or `<http://…>`. The core normalises it to the bare form, and every
        IRI on the wire and in results is bare. The viewer wraps it as
        `<iri>` for lookups in `ViewerGraph.index`, `Selection`,
        `ClassFilter` and `AgentHighlight`, which are all keyed by
        N-Triples terms, and unwraps it in responses. `hide_classes` and
        `show_classes` follow the same rule. The merge key for
        `scope: both` is the bare IRI. `matched_on` is one of `label`,
        `local_name`, `iri`, `attribute` or `store_literal`. Blank nodes
        cannot be addressed, because their ids are not stable across
        snapshots (`sparql-mcp-viewer.md`).
        ```
    *   **Location:** Line 354. Old: `` `AgentHighlight { terms: HashSet<String>, `` New: `` `AgentHighlight { terms: HashSet<String> /* "<iri>" N-Triples form */, ``

### 🟡 M11 [`egm-design`, `egm-codebase-details`, `egm-implementation-readiness`, `egm-structural`] - Severity: Major
*   **Affected Scope:** [Lines 331-336](../../sparql-mcp-viewer-agent-control.md#L331-L336), [583](../../sparql-mcp-viewer-agent-control.md#L583).
*   **Discrepancy Description:**
    *   The store search is a synchronous full scan on an async worker, with no deadline.
    *   It scans `cbm:sourceCode` bodies.
    *   Its label predicates do not match the viewer's, and the query text is not given.
*   **Rationale:** sparql_store.rs:159 (`query` is synchronous), main.rs:76-81 (`--with-source`), graph.rs:300-313 (`rdfs:label`, then `smc:label`).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The 512-character literal cap is a proposed default. The author may tune it.
*   ***Proposed Edit:***
    *   **Location:** Lines 331–336. Replace with:
        ````
        *   **`store` scope** runs in the core over **all named graphs**
            with one SPARQL `SELECT`, inside `tokio::task::spawn_blocking`
            because `SparqlStore::query` is synchronous. It is bounded by the
            tool deadline; a late result returns
            `reason: "store_search_timeout"`. It matches `STR(?s)` and
            literal objects of at most 512 characters, which keeps whole
            `cbm:sourceCode` bodies out. It picks one label in the viewer's
            order (`rdfs:label`, then `smc:label`, then `skos:prefLabel`).
            The needle is an escaped SPARQL string literal built by
            `sparql_string_literal`, never by concatenating raw user text:

            ```sparql
            SELECT ?g ?s (SAMPLE(?lbl) AS ?label) WHERE {
              GRAPH ?g {
                ?s ?p ?o .
                FILTER( CONTAINS(LCASE(STR(?s)), LCASE(?needle))
                     || ( isLiteral(?o) && STRLEN(STR(?o)) <= 512
                          && CONTAINS(LCASE(STR(?o)), LCASE(?needle)) ) )
                OPTIONAL { ?s rdfs:label ?l1 }
                OPTIONAL { ?s smc:label ?l2 }
                OPTIONAL { ?s skos:prefLabel ?l3 }
                BIND(COALESCE(?l1, ?l2, ?l3) AS ?lbl)
              }
            } GROUP BY ?g ?s LIMIT ?limit
            ```

            Here `?needle` and `?limit` are substituted by the helper before
            the query runs.
        ````

### 🟡 M12 [`egm-style`, `egm-inventive-catalyst`, `egm-codebase-details`, `egm-design`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 417-423](../../sparql-mcp-viewer-agent-control.md#L417-L423), [503-505](../../sparql-mcp-viewer-agent-control.md#L503-L505), [557](../../sparql-mcp-viewer-agent-control.md#L557).
*   **Discrepancy Description:**
    *   The byte budget is uncalibrated.
    *   An oversized response fails as a silent 10 s timeout.
    *   Converting to RGBA8 under HDR can give a semi-transparent PNG.
    *   A minimized window is not handled.
    *   The Phase 5 size check has no threshold.
*   **Rationale:** camera.rs:46 (`Hdr`). bevy_render-0.20.1 screenshot.rs:198 (alpha stores brightness under HDR).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** The alpha and silent-timeout parts are verified. The minimized-window behaviour is unverified on Bevy 0.20.1, so the fix is a cheap guard rather than a claim. The 3.5 MiB budget keeps the base64 form under common ~5 MB image limits; the author should confirm it against Claude Code's limit.
*   ***Proposed Edit:***
    *   **Location:** Lines 417–420. Replace with:
        ```
        *   The captured `Image` is converted with `Image::try_into_dynamic()`
            then `to_rgb8()`. Alpha is dropped because the camera is HDR, and
            in HDR the alpha channel stores brightness, as Bevy's own
            `save_to_disk` notes. The image is downscaled so its longest side
            is at most `max_side` (≤ 1568 px) and encoded as PNG. If the PNG
            exceeds 3.5 MiB (about 4.7 MiB in base64), the viewer re-encodes
            at 1280 px, then 1024 px. If it is still too large, it answers
            `ok: false, code: "payload_too_large"` at once. The response
            reports `width`, `height` and `bytes`. The core turns the image
            into MCP image content (`Content::image(base64, "image/png")`).
        *   If the primary window is minimized or has zero size, the viewer
            answers `ok: false, code: "window_not_visible"` at once instead
            of waiting for a capture that may never come.
        ```
    *   **Location:** Lines 504–505. Old: `A 1568 px PNG of a dark scene is usually well under 2 MiB.` New: `A screenshot PNG is at most 3.5 MiB before base64 (see Screenshot).`
    *   **Location:** Line 557 (Phase 5 exit). Old: `A PNG size check passes on a 5,000-node scene.` New: `On a label-dense 5000-node scene at 1568 px with panels, the PNG is under 3.5 MiB or is downscaled to fit. An oversized capture returns payload_too_large, not a timeout.`

### 🟡 M13 [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 645-649](../../sparql-mcp-viewer-agent-control.md#L645-L649).
*   **Discrepancy Description:** File 14 defers the choice of PNG dependency to Phase 5.
*   **Rationale:**
    *   `Image::try_into_dynamic()` is unconditional in bevy_image 0.20.1.
    *   `image` 0.25.10 and `base64` 0.23.1 are already in Cargo.lock.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Two reviewers independently cited the same versions.
*   ***Proposed Edit:***
    *   **Location:** Lines 647–649. Replace the body of file 14 with:
        ```
        Add `image = { version = "0.25", default-features = false,
        features = ["png"] }` (the same 0.25 major as `bevy_image`, so
        `DynamicImage` is one type), `base64 = "0.23"`,
        `tokio-util = { version = "0.7", features = ["codec"] }`, and
        `futures-util` for `StreamExt`. The conversion is
        `Image::try_into_dynamic()` (unconditional in bevy_image 0.20.1),
        then `to_rgb8()`, `resize`, and `write_to(…, ImageFormat::Png)`.
        Bevy's `png` feature is not enabled. In dev-dependencies, `rmcp`
        declares `features = ["macros", "client"]` (see file 15).
        ```

### 🟡 M14 [`egm-implementation-readiness`, `egm-codebase-details`, `egm-style`, `egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Line 558](../../sparql-mcp-viewer-agent-control.md#L558), [560-661](../../sparql-mcp-viewer-agent-control.md#L560-L661).
*   **Discrepancy Description:** The file list omits:
    *   the re-export in `infrastructure/mod.rs`;
    *   the callers broken by the new signature;
    *   the exhaustive `ViewerMsg` matches;
    *   the real skill paths.
*   **Rationale:** Verified paths: infrastructure/mod.rs:12; core tests/viewer_channel.rs:57, 124, 164; examples/viewer_load.rs:76; plugin.rs:133-160; ingest.rs:78-105, 137-156; headless.rs:50-56; headless_e2e.rs:34-46, 75; plugins/kb-workbench/skills/.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   **Location:** After file 16 (line 661). Add:
        ```
        #### 17. Call sites broken by the signature and enum changes (Modify)

        *   `crates/sparql-mcp-core/src/infrastructure/mod.rs`: also
            re-export `ViewerHub`.
        *   `crates/sparql-mcp-core/tests/viewer_channel.rs` (lines 57, 124,
            164), `crates/sparql-mcp-core/examples/viewer_load.rs` (line 76)
            and `crates/sparql-mcp-viewer/tests/headless_e2e.rs` (line 75):
            pass an `Arc<ViewerHub>`. Reuse the test helpers
            `listen_ephemeral`, `connect_and_request_snapshot` and
            `recv_json` (tests/viewer_channel.rs:22-47) in file 7.
        *   Exhaustive matches on `ViewerMsg` gain a
            `ViewerMsg::Request(_)` arm: `plugin.rs::apply_to_graph`,
            `ingest.rs::feed_entry_for` and `record_feed`,
            `headless.rs::print_messages`, and
            `tests/headless_e2e.rs::drain_until`. `AgentRequest` derives
            `Debug` and `Clone`, as `ViewerMsg` does.
        ```
    *   **Location:** Line 558. Old: `` The `sparql-first` / `kb-workbench` skill guidance `` New: `` `plugins/kb-workbench/skills/kb-workbench/SKILL.md` and `plugins/kb-workbench/skills/sparql-first-code/SKILL.md` ``
    *   **Location:** Lines 660–661. Old: `and skill guidance under plugins/.` New: `and the two SKILL.md files named in Phase 6.`

### 🟡 M15 [`egm-design`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 342-348](../../sparql-mcp-viewer-agent-control.md#L342-L348), [425-431](../../sparql-mcp-viewer-agent-control.md#L425-L431), [640-643](../../sparql-mcp-viewer-agent-control.md#L640-L643).
*   **Discrepancy Description:** Headless mode builds `CorePlugin` only, and `AgentPlugin` is registered only in `ViewerPlugin`. Headless requests therefore go unanswered and time out.
*   **Rationale:** headless.rs:34-37; plugin.rs:55-98. The synthesizer verified both.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   **Location:** Lines 425–431. Replace with:
        ```
        `CorePlugin` inserts `AgentOutbox`, wrapping the response sender
        that `connection::spawn` returns, and adds `answer_core_requests`
        from `agent.rs`, so it runs in both modes. That system answers
        `status` and `search_view` from `ViewerGraph`. In headless mode it
        rejects every other op with `unsupported_headless`. In windowed mode
        it leaves them to `handle_agent_requests`, which `AgentPlugin`
        (registered by `ViewerPlugin` only) adds. Headless `status` returns
        `graph`, `nodes`, `edges` and `connection`, with `camera: null`,
        `visible: null` and `selection: null`. This keeps the end-to-end test
        free of a window while still covering the full request/response
        path.
        ```
    *   **Location:** Lines 642–643 (file 13). Old: `Headless answers status and search_view and rejects the rest.` New: `` `CorePlugin` inserts `AgentOutbox` and adds `answer_core_requests` (see Headless mode). ``

### 🟡 M16 [`egm-codebase-details`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 651-655](../../sparql-mcp-viewer-agent-control.md#L651-L655).
*   **Discrepancy Description:**
    *   The rmcp dev-dependency has no `client` feature.
    *   The existing test runs no Bevy app.
    *   `headless::run` blocks.
    *   The doc does not say where the core runs.
*   **Rationale:** viewer Cargo.toml:45; headless_e2e.rs:1-4, 94; headless.rs:27-44; the viewer CLI flag is `--connect` (main.rs:22-23).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   **Location:** Lines 651–655. Replace the heading and body with:
        ```
        #### 15. `crates/sparql-mcp-viewer/tests/headless_e2e.rs` (Modify)

        Build `SparqlMcpServer` in-process with `with_viewer_control(hub)`,
        serve it over `tokio::io::duplex`, and connect an `rmcp` client to
        the other half. Run `serve_viewer_channel` on an ephemeral port
        with the same hub. Start the headless viewer as a child process,
        `env!("CARGO_BIN_EXE_sparql-mcp-viewer") --headless --connect
        127.0.0.1:<port>`, and kill it at the end. Assert `viewer_status`
        counts and a `viewer_search` `scope: view` hit. The `rmcp`
        dev-dependency gains the `client` feature (file 14).
        ```

### 🟡 M17 [`egm-implementation-readiness`, `egm-inventive-catalyst`, `egm-style`] - Severity: Major
*   **Affected Scope:** [Lines 311-314](../../sparql-mcp-viewer-agent-control.md#L311-L314), [394](../../sparql-mcp-viewer-agent-control.md#L394), [405-409](../../sparql-mcp-viewer-agent-control.md#L405-L409).
*   **Discrepancy Description:**
    *   The tracker's semantics are undefined.
    *   Agent and user writes are indistinguishable to change detection.
    *   `user.active` is stale by the time the agent acts on it.
    *   "The user always wins" overclaims.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The `epoch` precondition strengthens author decision 2 and keeps the doc's own rule that the agent decides. It is optional for the agent, so it adds no lock.
*   ***Proposed Edit:***
    *   **Location:** Line 394. Old: `The rule is simple: the agent never locks anything, and the user always wins.` New: `The agent never locks the UI. User camera input always takes precedence over agent camera moves. The user can undo any agent change with Esc, the chip or the existing panels. For filter and selection, the viewer reports user activity and the agent decides.`
    *   **Location:** Lines 405–409. Replace with:
        ```
        *   **Feedback.** A `UserActivityTracker` resource holds:
            *   `last_input`: the time of the last mouse button, key, wheel
                or pointer-move event over the window;
            *   `epoch`: a counter incremented by every user change to the
                camera, filter or selection (node click, `Esc`, a
                filter-panel button, `F`, or any frame where
                `current_motion.is_user_controlled()`);
            *   `overrode`: set by those same changes, reset by each applied
                agent command.
            Agent writes to `ClassFilter` and `Selection` do not touch the
            tracker. Each response carries `user.active` (`last_input` in
            the last 3 s), `user.epoch` and
            `user.overrode_last_agent_command`, all computed before the
            current request is applied. A mutating request may carry
            `if_user_epoch`. If the epoch has moved on, the request is
            rejected with `reason: "user_changed"` and changes nothing. The
            agent decides; the viewer only refuses to act on a stale view of
            the user's state.
        ```
    *   **Location:** Line 313. Old: `Check user.active before moving the camera.` New: `Pass the last user.epoch as if_user_epoch when changing the camera, filter or selection.`

### 🟡 M18 [`egm-inventive-catalyst`, `egm-design`, `egm-structural`] - Severity: Major
*   **Affected Scope:** [Lines 204-212](../../sparql-mcp-viewer-agent-control.md#L204-L212), [261-264](../../sparql-mcp-viewer-agent-control.md#L261-L264), [506-512](../../sparql-mcp-viewer-agent-control.md#L506-L512).
*   **Discrepancy Description:** Control flips between viewers on every reconnect. Any local process that connects becomes the target, and nothing shows that control moved.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. One handshake field fixes it, and it prepares the "Choosing among several viewers" Future Improvement.
*   ***Proposed Edit:***
    *   **Location:** Line 207. Old: `{"cmd":"snapshot","control":1}` New: `{"cmd":"snapshot","control":1,"instance":"3f2c…"}`, and add after line 212: `` `instance` is a random id chosen once per viewer process and kept across reconnects. ``
    *   **Location:** Lines 261–264. Replace with:
        ```
        *   **Target viewer.** The hub holds at most one controlled
            instance: the controllable instance it first saw most recently.
            A reconnect from the same instance keeps the slot, so resyncs and
            reconnects never move control to another window. If the
            controlled instance has not come back within the tool deadline,
            the hub promotes the most recently seen remaining instance.
            `viewer_status` reports `instance`, so the agent can tell when
            control has moved.
        ```
    *   **Location:** Line 511. After `fake screenshot.`, insert: `` Such a process becomes the controlled viewer if it is the newest instance; `viewer_status.instance` makes the switch visible. ``

### 🟡 M19 [`egm-structural`] - Severity: Major
*   **Affected Scope:** Between [line 488](../../sparql-mcp-viewer-agent-control.md#L488) and the separator at line 490.
*   **Discrepancy Description:** The feature-spec standard requires a Risks & Mitigations section, and the target has none.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid against the supplied standard. The table collects the mitigations proposed above, so it adds no new design.
*   ***Proposed Edit:***
    *   **Location:** Insert after line 488:
        ```
        --------------------------------------------------------------------------------

        ## Risks & Mitigations

        | Risk | Impact | Mitigation |
        |---|---|---|
        | Partial or oversized lines on a bidirectional socket | Lost snapshots or responses, a wedged connection | Cancel-safe capped `FramedRead`, separate reader and writer tasks (*Viewer registry*) |
        | Resync during a request (`changed`, `Lagged`) | Lost or duplicated commands | In-band resync, idempotent-only retry, a fresh id per retry |
        | Request overtakes a just-emitted write | False `missing` | `biased` writer drains events before requests |
        | Store search scans large literals | Slow tool, blocked runtime worker | `spawn_blocking`, 512-character literal cap, tool deadline |
        | Screenshot too large, or window minimized | Silent timeout, image rejected downstream | 3.5 MiB budget with downscale, `payload_too_large`, `window_not_visible` |
        | Control silently moves to another viewer or a local process | Agent drives the wrong window | Sticky `instance` id, reported in `viewer_status` |
        | Agent overwrites user changes it has not seen | User loses their view | `user.epoch` and `if_user_epoch`; `Esc` and the chip |
        | Local process answers with forged content | Misleading hits or images in the agent context | Accepted (local-user trust boundary); core validates PNG and caps strings |
        ```
    *   **Location:** Lines 506–512. Append: `See Risks & Mitigations.`

### 🟡 M20 [`egm-inventive-catalyst`] - Severity: Major
*   **Affected Scope:** [Lines 402-404](../../sparql-mcp-viewer-agent-control.md#L402-L404), [480-488](../../sparql-mcp-viewer-agent-control.md#L480-L488).
*   **Discrepancy Description:** The user can clear the agent's highlight but cannot restore the camera, filter and selection that the agent overwrote.
*   ***AI Suggestion:*** ESCALATE
*   ***Justification:*** The finding is plausible, and it relates to author decision 2 ("the user can always take back control"). Two readings are possible:
    *   Take-back means stopping the agent and undoing its highlight. The doc already meets this.
    *   Take-back means getting the user's own view back. That needs a new feature (a checkpoint) and changes `Esc` semantics in the companion viewer design.

    Only the author can choose. If M21 is adopted, the agent no longer changes filter or selection, and the finding shrinks to camera-only.
*   ***Proposed Edit (if the author accepts):***
    *   **Location:** After line 404. Insert:
        ```
        *   **Restore.** Before the first agent command that changes the
            camera, filter or selection after any user input, the viewer
            saves the user's camera pose, `ClassFilter` and `Selection`
            (keyed by IRI) in a one-level `UserViewCheckpoint`. The chip's
            "Restore my view" button, and the second `Esc`, clear the agent
            highlight and return to that saved state with an animated move.
        ```
    *   Update Required change 3 (line 543) to mention the restore behaviour of `Esc`.

### 🟡 M21 [`egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 66-69](../../sparql-mcp-viewer-agent-control.md#L66-L69), [300-309](../../sparql-mcp-viewer-agent-control.md#L300-L309), [554-555](../../sparql-mcp-viewer-agent-control.md#L554-L555).
*   **Discrepancy Description:** `viewer_filter`, the standalone `viewer_select`, and camera `orbit`/`reset` serve no CUJ. `viewer_select` also duplicates `viewer_show`'s `select?` argument.
*   ***AI Suggestion:*** ESCALATE
*   ***Justification:*** This is a scope reduction, and it is not settled by the ground truth.
    *   The Overview (lines 16–18) explicitly lists "change the class filter, select a node" as capabilities the author intends.
    *   Cutting them is a product call. It would also simplify M17, M20 and Alternative E.
    *   The alternative is to keep them and add a CUJ that exercises them, which would satisfy the zoom-out concern without cutting.
*   ***Proposed Edit (option A, cut):*** Remove the `viewer_filter` and `viewer_select` rows from the table at lines 300–309. Restrict `viewer_camera` `action` to `frame_all | frame | zoom`. Add a Future Improvements bullet: `**Agent filter, standalone select, orbit and reset.** Deferred from v1; viewer_show's select? covers selection.`
*   ***Proposed Edit (option B, keep):*** Add a CUJ 6, "Ask the agent to hide a class": the agent calls `viewer_filter` with `hide_classes`, and the filter panel shows the change, which the user can undo by clicking.

### 🟡 M22 [`egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 320-340](../../sparql-mcp-viewer-agent-control.md#L320-L340), [621-622](../../sparql-mcp-viewer-agent-control.md#L621-L622).
*   **Discrepancy Description:** Search runs through two engines with different matching rules: viewer-side ranking and store-side SPARQL.
*   ***AI Suggestion:*** ESCALATE
*   ***Justification:*** Author decision 3 fixes the *coverage* (view and store), not the number of engines, so this does not re-open the decision. It does change how the decision is realised, and the trade-off is real:
    *   The viewer matcher sees viewer-computed labels (`fallback_label`, local names) and ranks by `mentions`, which the store query cannot do cheaply.
    *   A single core engine removes the `search_view` op and the merge code.

    The author should pick one. M9's fix works with either choice.
*   ***Proposed Edit (option A, one engine):*** Replace lines 324–340 with: "Search runs once in the core over all named graphs (see the store query). `in_view` comes from `search_view`'s `also_check` membership check, reduced to that one role (or a `resolve` op). `scope: view` restricts results to the active graph." Drop `search_graph` and the ranking from file 9.
*   ***Proposed Edit (option B, keep two):*** After line 322, add: "Two matchers are kept on purpose. The viewer ranks by drawn label and `mentions`, which the store cannot compute cheaply. The store covers graphs that are not drawn. Both use the same case-insensitive substring rule, so the same text matches the same fields."

### 🟢 N1 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 201-202](../../sparql-mcp-viewer-agent-control.md#L201-L202).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid (connection.rs:219-231, 250). Downgraded from Blocker, because this is a one-phrase fix.
*   ***Proposed Edit:*** Old: `All lines stay newline-delimited JSON, told apart by their single top-level key, as in the base protocol.` New: `All lines stay newline-delimited JSON. Each kind is told apart by a discriminating top-level key (snapshot, event, request, response), checked in that order. Event lines also carry other keys beside event.`

### 🟢 N2 [`egm-design`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 350-353](../../sparql-mcp-viewer-agent-control.md#L350-L353).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid (plugin.rs:64-67, 99-104).
*   ***Proposed Edit:*** Old: `` runs in `CoreSet::Ui`, after `apply_to_graph`, so a request always sees the graph as of the same frame. `` New: `` runs in `CoreSet::Graph`, after `apply_to_graph` and before `CoreSet::Sync`, so a request sees the graph as of the same frame and its material changes render in that frame. ``

### 🟢 N3 [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 402-404](../../sparql-mcp-viewer-agent-control.md#L402-L404), [637](../../sparql-mcp-viewer-agent-control.md#L637).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid (interact.rs:209-213). This is a Nit rather than ir's Major: the fix is one sentence.
*   ***Proposed Edit:*** Append to line 404: `` Both steps live in one system, `escape_chain`, which replaces `clear_selection_on_escape`, so one press has exactly one effect. `` At line 637, Old: `` The `Esc` chain `` New: `` `escape_chain` (replacing `clear_selection_on_escape`) ``.

### 🟢 N4 [`egm-design`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 503-504](../../sparql-mcp-viewer-agent-control.md#L503-L504).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. 5000 IRIs at about 210 bytes each exceed 1 MiB.
*   ***Proposed Edit:*** After `At most 5000 IRIs per show.`, insert: `The core checks the serialised size of each request before sending; a request over 1 MiB returns applied: false, reason: "request_too_large".`

### 🟢 N5 [`egm-design`] - Severity: Nit
*   **Affected Scope:** [Lines 506-512](../../sparql-mcp-viewer-agent-control.md#L506-L512).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid, and consistent with the stated trust boundary. It adds defence in depth, not a new boundary.
*   ***Proposed Edit:*** Append: `The core treats viewer responses as untrusted data. It checks that a screenshot decodes as PNG with a longest side of at most max_side, caps hit counts at limit and strings at 1 KiB, and tool descriptions present viewer-returned labels as data, not instructions.`

### 🟢 N6 [`egm-design`, `egm-codebase-details`, `egm-comprehension`] - Severity: Nit
*   **Affected Scope:** [Line 284](../../sparql-mcp-viewer-agent-control.md#L284), [598-602](../../sparql-mcp-viewer-agent-control.md#L598-L602).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid (main.rs:372 drops the handle).
*   ***Proposed Edit:***
    *   Line 284. Old: `` The tools are listed only when `[viewer] enabled = true`. `` New: `` The tools are listed when `[viewer] enabled = true` and `bind` parses. If binding fails at runtime (for example, another session owns the port), they stay listed and return `not_connected`. ``
    *   File 6, append: `` Log a bind error from `spawn_viewer_channel` instead of dropping its `JoinHandle`, and record it in the hub so `viewer_status` reports `listener: "bind_failed"`. ``

### 🟢 N7 [`egm-design`] - Severity: Nit
*   **Affected Scope:** [Lines 265-269](../../sparql-mcp-viewer-agent-control.md#L265-L269), [411-416](../../sparql-mcp-viewer-agent-control.md#L411-L416).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid edge cases, each fixed in one sentence.
*   ***Proposed Edit:***
    *   Append to the Pending map bullet: `A drop guard removes the pending entry if the waiting tool future is cancelled. Concurrent viewer_* calls are sent in arrival order and have no other ordering guarantee.`
    *   Append to the first Screenshot bullet: `If a CameraTween is running, the capture waits until it ends, within the tool deadline.`

### 🟢 N8 [`egm-implementation-readiness`, `egm-zoom-out`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 564-566](../../sparql-mcp-viewer-agent-control.md#L564-L566), [579-596](../../sparql-mcp-viewer-agent-control.md#L579-L596).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Following the existing `EventSink` pattern (server.rs:42, 62, 66-69) removes the Option/Null duplication.
*   ***Proposed Edit:***
    *   File 1, append: `` `ViewerOp` uses `#[serde(tag = "op", content = "args", rename_all = "snake_case")]`. ``
    *   File 3, append: `` Entry points: `pub fn tool_defs() -> Vec<Tool>` and `pub async fn dispatch(name: &str, args: &JsonObject, ctl: &dyn ViewerControl, store: &Arc<dyn SparqlStore>, active_graph: &Arc<RwLock<String>>) -> Option<Result<CallToolResult, McpError>>`. ``
    *   File 5. Old: `` Add a `viewer: Option<Arc<dyn ViewerControl>>` field and `with_viewer_control`. `list_tools` adds the viewer tool definitions when the field is `Some`. `` New: `` Add a `viewer: Arc<dyn ViewerControl>` field defaulting to `NullViewerControl` (as `event_sink` defaults to `NullEventSink`), a `viewer_tools_enabled: bool`, and `with_viewer_control`, which sets both. `list_tools` adds `viewer::tool_defs()` when the flag is set. Extend the tool-parity test (`every_declared_tool_has_a_dispatch_arm`) to the viewer tools. ``

### 🟢 N9 [`egm-zoom-out`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Line 583](../../sparql-mcp-viewer-agent-control.md#L583), [621-622](../../sparql-mcp-viewer-agent-control.md#L621-L622).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Each reuse target exists (cbm/turtle.rs:185-191, importer.rs:97, interact.rs:86, 96, 271; look_to.rs easing).
*   ***Proposed Edit:***
    *   File 3, after `sparql_string_literal(&str)`, insert: `` (moved to `application`, built on `cbm::turtle::escape`, which already escapes `\ " \n \r \t`; `importer::escape_literal` then calls it) ``
    *   File 9, append: `` `search_graph` reuses `interact::short` for local names and `interact::display_literal` to unquote attribute values before matching. The chip reuses `interact::button_node`. ``

### 🟢 N10 [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Line 123](../../sparql-mcp-viewer-agent-control.md#L123), [366-368](../../sparql-mcp-viewer-agent-control.md#L366-L368).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid (ingest.rs:19-47).
*   ***Proposed Edit:*** Lines 366–367. Old: `` A new `FeedKind::Agent` entry is pushed per applied command, for example `agent · frame 14`. `` New: `` A new `FeedKind::Agent(AgentFeedOp)` entry is pushed per applied command. `AgentFeedOp` names the op and mode, the count goes in `triple_count`, and the panel renders it as `agent · <op> <count>[ (<mode>)]`, for example `agent · show 14 (isolate)`. ``

### 🟢 N11 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 613-616](../../sparql-mcp-viewer-agent-control.md#L613-L616).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid (connection.rs:1-3). Folded into the B1 replacement for file 8, which returns a plain `UnboundedSender`.
*   ***Proposed Edit:*** See B1, file 8 text.

### 🟢 N12 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 389-390](../../sparql-mcp-viewer-agent-control.md#L389-L390).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. `F` writes in `Update`, and the tween overwrites it in the next `PreUpdate`.
*   ***Proposed Edit:*** Append to line 390: `` `F` also removes any running `CameraTween`. `frame_target` takes positions from `LayoutState` for the resolved `NodeId`s, a snapshot of where they are on that frame. ``

### 🟢 N13 [`egm-implementation-readiness`] - Severity: Nit
*   **Affected Scope:** [Lines 553-554](../../sparql-mcp-viewer-agent-control.md#L553-L554).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Phase 2 cannot exercise `focus: true` before Phase 3 exists.
*   ***Proposed Edit:***
    *   Phase 2 exit. Old: `Manual CUJ 1 (with a hand-written IRI list)` New: `Manual CUJ 1 with focus: false (hand-written IRI list)`.
    *   Phase 1 exit. Old: `Old-viewer and old-core compatibility tests pass.` New: `Old-viewer (file 7) and old-core (a connection.rs unit test against a fake server that ignores control) compatibility tests pass.`

### 🟢 N14 [`egm-zoom-out`] - Severity: Nit
*   **Affected Scope:** [Lines 494-496](../../sparql-mcp-viewer-agent-control.md#L494-L496).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The batch-ingestion design records the stall.
*   ***Proposed Edit:*** Append: `This target assumes write tools run off the async workers (batch-ingestion F9). Until then, a viewer_* call made during a long write may wait for it, still bounded by the tool deadline.`

### 🟢 N15 [`egm-implementation-readiness`] - Severity: Nit
*   **Affected Scope:** [Lines 357-358](../../sparql-mcp-viewer-agent-control.md#L357-L358), [367-369](../../sparql-mcp-viewer-agent-control.md#L367-L369), [568-577](../../sparql-mcp-viewer-agent-control.md#L568-L577).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. These values make the plan executable without guesswork.
*   ***Proposed Edit:***
    *   Line 358: after `emissive cyan`, insert `(base colour srgb(0.2, 0.95, 1.0), emissive LinearRgba::rgb(0.3, 2.2, 2.6))`.
    *   Line 369: append `The chip carries Hovered so block_controller_over_ui sees it.`
    *   File 2: append `crates/sparql-mcp-core/Cargo.toml declares the tokio time feature explicitly.`

### 🟢 N16 [`egm-structural`, `egm-style`, `egm-comprehension`, `egm-design`, `egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Line 27](../../sparql-mcp-viewer-agent-control.md#L27).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The paragraph at lines 36–39 is rewritten by B3.
*   ***Proposed Edit:*** Old: `Ownership is split across three documents:` New: `Ownership is split across four documents:`

### 🟢 N17 [`egm-structural`, `egm-comprehension`] - Severity: Nit
*   **Affected Scope:** [Lines 31-32](../../sparql-mcp-viewer-agent-control.md#L31-L32), [271](../../sparql-mcp-viewer-agent-control.md#L271), [535-541](../../sparql-mcp-viewer-agent-control.md#L535-L541).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Lines 31–32: replace the code spans with links, `` [`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md) `` and `` [`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md) ``.
    *   Line 202: Old `base protocol` → New `base protocol (interface-agentic-rust-stylee.md)`.
    *   Line 318: covered by M10.
    *   Lines 535, 538 and 541: link the file names.
    *   Use italic heading names for all cross-references, for example line 513: `(see *Wire protocol additions*)`.

### 🟢 N18 [`egm-style`, `egm-comprehension`, `egm-inventive-catalyst`] - Severity: Nit
*   **Affected Scope:** [Lines 227-228](../../sparql-mcp-viewer-agent-control.md#L227-L228).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The synthesizer set Nit rather than sty's Major, because the B2 and M1 text already makes matching by `id` explicit.
*   ***Proposed Edit:*** Old: `The viewer echoes it and never reorders the meaning of two ids.` New: `The viewer echoes it unchanged. Responses may arrive out of request order (a screenshot answers frames later), so the core matches them by id only.`

### 🟢 N19 [`egm-structural`] - Severity: Nit
*   **Affected Scope:** [Lines 222-225](../../sparql-mcp-viewer-agent-control.md#L222-L225).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The envelope at lines 291–294 is already switched to `text` by M4.
*   ***Proposed Edit:*** Change the fence at line 222 from ```` ```json ```` to ```` ```text ````.

### 🟢 N20 [`egm-structural`] - Severity: Nit
*   **Affected Scope:** [Lines 453-455](../../sparql-mcp-viewer-agent-control.md#L453-L455), [459-476](../../sparql-mcp-viewer-agent-control.md#L459-L476), [516](../../sparql-mcp-viewer-agent-control.md#L516).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid as presentation. The rejected sub-alternatives at screenshot and targeting are already implied by M12 and M18.
*   ***Proposed Edit:***
    *   Line 516: `### Future Improvements` → `## Future Improvements`, preceded by the `----` separator.
    *   Line 455: replace with `This is the design above. **Pros:** one MCP server; reuses the existing connection, store access and no-op semantics. **Cons:** protocol changes in two crates; control needs sparql-mcp-core running.`
    *   Alternative C: add `**Pros:** control and event traffic are isolated on separate sockets.`
    *   Alternative D: add `**Pros:** no protocol change; commands are auditable as triples.`

### 🟢 N21 [`egm-structural`, `egm-style`] - Severity: Nit
*   **Affected Scope:** [Line 103](../../sparql-mcp-viewer-agent-control.md#L103), [447](../../sparql-mcp-viewer-agent-control.md#L447), [557](../../sparql-mcp-viewer-agent-control.md#L557), [669](../../sparql-mcp-viewer-agent-control.md#L669).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Line 103: `labels density` → `label density`.
    *   Lines 446–448 and 668–670: re-wrap at about 78 columns.
    *   Line 557: `5,000-node` → `5000-node`, matching lines 304 and 503 (already part of the M12 text).

### 🟢 N22 [`egm-style`, `egm-comprehension`] - Severity: Nit
*   **Affected Scope:** Lines 47–60, 71–72, 81–89, 122, 157, 322, 335–337, 397–398, 427–431, 439, 450–451, 510–511.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Each item is a calibration or clarity edit. None changes the design.
*   ***Proposed Edit:*** One replacement per item:
    *   L47: `The agent is blind to the viewer.` → `The agent cannot point at anything in the viewer.`
    *   L51: `finding a node by eye is slow.` → `the user must scan thousands of labels to find one node.`
    *   L54: `where it would be easiest to read.` → `so the user cannot see where those nodes sit in the graph.`
    *   L71–72: `in the drawn graph, and/or in every named graph of the store through SPARQL.` → `in the drawn graph (view), in every named graph of the store through SPARQL (store), or both (both, the default).`
    *   L81–82: `with Esc or a button.` → `with Esc (a second Esc when a node is selected) or the chip's Clear button.`
    *   L86–87: delete `The agent's flow never breaks because the window is closed.`
    *   L88–89: `keeps working as today.` → `streams store events as today; the core sends no requests to a viewer that did not send control and reports not_connected for it.`
    *   L122: `in about 0.6 s.` → `in 600 ms (the default duration_ms).`
    *   L157: after `1568 px`, insert `(the longest side Claude accepts without downscaling)`.
    *   L160: `to tighten the view.` → `to zoom in until the labels separate.`
    *   L322: delete `, as decided for this feature`.
    *   L335: `built by one helper` → `built by sparql_string_literal`.
    *   L337: `runs the two at once` → `runs the two concurrently` (if M9's sequential order is not adopted).
    *   L397–398: `` `search` with `show` `` → `` the `viewer_search` tool with `show` (which chains into the show op) ``.
    *   L428–429: `Every op that needs a renderer` → `Every op that depends on windowed-only plugins`.
    *   L439: `on Windows loopback` → `on loopback`.
    *   L450–451: `It costs more setup and is less robust` → `It adds a second server and port to configure and fails when the window is closed`.
    *   L510–511: delete `, and is written down here rather than assumed away`.
    *   L495–496: `is under 100 ms` → `targets under 100 ms (measured by a timing assertion in file 7)`.

### 🟢 N23 [`egm-comprehension`] - Severity: Nit
*   **Affected Scope:** [Line 12](../../sparql-mcp-viewer-agent-control.md#L12), [238](../../sparql-mcp-viewer-agent-control.md#L238), [344-350](../../sparql-mcp-viewer-agent-control.md#L344-L350).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid, and cheap to fix.
*   ***Proposed Edit:***
    *   After line 14, insert: `Each sparql-mcp project is a named graph in one shared store. The viewer draws the active project's graph, and project_switch changes it.`
    *   At first use, add these glosses:
        *   `Decoded` (the viewer's parsed-line enum in connection.rs);
        *   `Inbox` → `FrameMessages` (the channel the connection thread fills, drained once per frame);
        *   `CoreSet` (the viewer's Net → Graph → Sync → Ui system sets);
        *   "last-batch pulse" (the brief glow on nodes touched by the latest write);
        *   `--headless` (the viewer's window-less test mode).

### 🟢 N24 [`egm-zoom-out`] - Severity: Nit
*   **Affected Scope:** Lines 131–132, 247–258, 309, 326–330, 357–358, 378–382.
*   ***AI Suggestion:*** DECLINE
*   ***Justification:*** The reviewer's three proposals each work against another finding or against the target's purpose:
    *   **Moving detail down to the implementation plan:** this pulls against the implementation-readiness findings (M8, M10, M11, N8), which ask for *more* precision. The previous viewer review (`../sparql-mcp-viewer/`) also required exact signatures. Keeping the trait signature and ranking rules in the Technical Plan helps implementers.
    *   **Dropping `max_side`:** the argument is now load-bearing for M12's downscaling.
    *   **Dropping `search … show` chaining:** CUJ 2 relies on it, and the B4 and M4 text defines its takeover behaviour.

    No edit is proposed.
