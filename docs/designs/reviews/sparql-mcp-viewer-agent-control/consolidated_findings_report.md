# Consolidated Findings: `sparql-mcp-viewer-agent-control.md` (Round 1)

*   **Target**: `docs/designs/sparql-mcp-viewer-agent-control.md`, full file
    (lines 1–674), status Draft, 2026-11-18.
*   **Mode**: REVIEW. Eight Goldfish reviewers ran in parallel, one instance
    each.
*   **GoldfishContext**:
    *   `context/ground-truth.md`: the five author decisions, the environment
        (native Windows, no WSL), and verified code facts;
    *   companion designs `sparql-mcp-viewer.md` and
        `interface-agentic-rust-stylee.md` (its stale WSL assumption is not
        counted as a defect of the target);
    *   sibling design `sparql-mcp-batch-ingestion.md`;
    *   source under `crates/sparql-mcp-core` and `crates/sparql-mcp-viewer`;
    *   registry sources: Bevy 0.20.1 (`bevy_camera_controller`,
        `bevy_render`, `bevy_image`), tokio 1.52.1 and rmcp 1.5.0.
*   **Standards**:
    *   `feature-spec-standard.md` (structural, comprehension);
    *   `implementation-readiness-standard.md` (implementation-readiness);
    *   `zoom-out-standard.md` (zoom-out);
    *   built-in persona criteria for the others.
*   **Synthesizer verification**: before assigning severity, the synthesizer
    checked these claims in the source:
    *   `read_line` sits inside `select!` (viewer_channel.rs:77,
        connection.rs:136-140);
    *   `is_actively_controlled` excludes zoom-only motion
        (bevy_camera_controller-0.20.1 component.rs:195-203);
    *   literal-only subjects are drawn (graph.rs:215, 242-248);
    *   the viewer camera is `Hdr` (camera.rs:46), and Bevy's
        `save_to_disk` drops alpha under HDR (screenshot.rs:198);
    *   headless mode uses only `CorePlugin` (headless.rs:34-37,
        plugin.rs:55-98);
    *   `apply_node_materials` runs in `CoreSet::Sync`, before `Ui`
        (plugin.rs:64-67, 99-104);
    *   the batch-ingestion design emits `Changed` above
        `event_changed_threshold` and then calls `viewer_show`
        (batch-ingestion.md:297-305, 455);
    *   the viewer reconnects on `changed` (connection.rs:147-155).

## Reviewer Status

| Reviewer | Short | Status | Raw B / M / N |
|---|---|---|---|
| `egm-comprehension` | cmp | completed | 0 / 6 / 11 |
| `egm-design` | des | completed | 2 / 8 / 9 |
| `egm-structural` | str | completed | 0 / 2 / 10 |
| `egm-style` | sty | completed | 0 / 7 / 13 |
| `egm-zoom-out` | zo | completed | 0 / 3 / 6 |
| `egm-implementation-readiness` | ir | completed (verdict NOT READY) | 1 / 14 / 8 |
| `egm-codebase-details` | cd | completed | 4 / 11 / 12 |
| `egm-inventive-catalyst` | ic | completed | 0 / 8 / 1 (3 sub-items) |

No reviewer failed, and no retry was needed.

**Round result: not clean.** After deduplication there are **4 Blockers,
22 Majors and 24 Nits** (50 findings in total).

Several severities differ from what one reviewer proposed. The final
severity is the synthesizer's call, made after checking the code. Each
change is noted in the finding:

*   cd-B2, cd-B3 and cd-B4 were downgraded. Their factual errors are real,
    but each is local and quick to fix.
*   ic-F1 was upgraded to Blocker together with zo-1, des-M3 and cmp-6,
    because it breaks the flagship cross-design journey.

--------------------------------------------------------------------------------

## Findings Table

### 🔴 Blockers

| ID | Lines | Finding | Raised by |
|---|---|---|---|
| B1 | 234–236, 275–278, 575–577, 613–616, 645–649 | `read_line` inside the new `select!` arms is not cancel-safe, and the line caps cannot be enforced with it | des-B1, ir-B1, cd-M1 |
| B2 | 275–278, 346–348, 575–577, 614–616 | Reading and writing in one task on both sides can deadlock the connection | des-B2 |
| B3 | 36–39, 261–274, 497–499, 533–544 | Reconnect-on-`changed` breaks the headline `ingest_commit` → `viewer_show` flow. Fix it at the source with an in-band resnapshot | zo-1, ic-F1, des-M3, cd-M3, cmp-6 |
| B4 | 145–148, 378–382, 396–401, 626–628 | `is_actively_controlled()` ignores wheel zoom, and the tween is not ordered against input or momentum, so "the user always wins" fails | cd-B1, des-M4, ir-M2, cd-M4 |

### 🟡 Majors

| ID | Lines | Finding | Raised by |
|---|---|---|---|
| M1 | 265–274 | The one-shot retry re-applies non-idempotent ops (`orbit`, `zoom`). Stale or unknown response ids are unhandled | des-M1, ir-M9, cd-M2, ic-F1 |
| M2 | 36–39, 275–278, 304, 350–353 | A request can overtake already-queued store events, so `viewer_show` reports fresh IRIs as `missing` | des-M2, ic-F2 |
| M3 | 270–274, 423, 497–499 | The "about 15 s" ceiling is wrong (8 s / 22 s), and the timeout scope (per attempt or per call) is undefined | str-2, sty-M1, cmp-2, des-N3, ir-N1, cd-M7, ic-F9a |
| M4 | 166–167, 249–253, 289–298, 396–401 | A partly applied command (`user_controlling_camera`) is labelled `Rejected` / `applied:false`. The envelope is incomplete | sty-M3, cmp-3, des-M8, ir-M7, ic-F9b, str-8 |
| M5 | 296–298, 304 | "Bad arguments" (soft reject) and "malformed arguments" (`invalid_params`) are not told apart | sty-M4, cmp-4 |
| M6 | 120–121 vs 357–362 | CUJ 1's "highlight ring" contradicts the material-swap rendering design | sty-M5, cmp-1, des-N6, ir-M4, cd-M6 |
| M7 | 357–362, 554, 630–633 | Isolate edge fading never refreshes (`update_edge_meshes` dirty check), and material precedence has no testable pure function | ir-M5, cd-M6 |
| M8 | 302, 383–388 | `look_at`, `distance`, the orbit pivot and the `reset` pose have no source in `PanOrbitCamera` | des-M5, ir-M3, cd-M5 |
| M9 | 70–73, 229–231, 308, 337–340 | `in_view` for store hits cannot be computed, and the "literal-only subjects are not drawn" claim is false | cmp-5, des-M6, ic-F8, ir-M8, cd-B2 (downgraded), zo-2 |
| M10 | 316–318, 324–340, 354–356, 581–584 | The IRI wire form (bare vs `<…>`), the merge key and the `matched_on` values are undefined | ir-M8, cd-N3, cmp-11 |
| M11 | 331–337, 494–496 | Store search is a synchronous, unbounded full scan (it includes `cbm:sourceCode` literals), and its label predicates and exact query are unspecified | des-M7, cd-M8, ir-M14, cd-N5, str-1 |
| M12 | 234–236, 411–423, 503–505, 557 | The screenshot byte budget is uncalibrated, an oversize line fails silently, the HDR alpha channel is wrong, and a minimized window is not handled | sty-M7, ic-F6, ic-F7, cd-M10, des-N8, ir-N8 |
| M13 | 645–649 | File 14 leaves the PNG dependency choice to the implementer | ir-M1, cd-M10 |
| M14 | 560–661 | The file list is incomplete: broken call sites, exhaustive `ViewerMsg` matches, the re-export, and the skill paths | ir-M10, cd-M9, sty-N3, cmp-15 |
| M15 | 342–348, 425–431, 640–643 | Headless wiring: `AgentPlugin` lives only in `ViewerPlugin`, so headless requests would time out | des-N1, ir-M11, cd-M11 |
| M16 | 92–93, 651–655, 667 | The headless e2e harness (file 15) is infeasible as written: the rmcp dev-dep has no `client`, and the test runs no Bevy app | cd-B4 (downgraded), ir-M13 |
| M17 | 79–83, 311–314, 394, 405–409, 620–622, 637 | `UserActivityTracker` semantics are undefined. Check-then-act on `user.active` is a race. "The user always wins" overclaims | ir-M12, ic-F4, sty-M2 |
| M18 | 105–106, 261–264, 506–512 | The controlled-viewer target flips on every reconnect, and any local process silently takes control | ic-F3, des-N2, des-N7, str-1 |
| M19 | 492–514 (section missing) | There is no Risks & Mitigations section, which the feature-spec standard requires | str-1 |
| M20 | 149–151, 363–365, 402–404, 480–488 | The user can take back control but cannot restore the previous view | ic-F5 |
| M21 | 66–69, 300–309, 363–365, 554–555 | The v1 tool surface goes beyond the CUJs (`viewer_filter`, `viewer_select`, `orbit`/`reset`) | zo-3 |
| M22 | 229–231, 320–340, 621–622 | Search is implemented twice (a viewer matcher and a store matcher) with different matching rules | zo-2 |

### 🟢 Nits

| ID | Lines | Finding | Raised by |
|---|---|---|---|
| N1 | 201–202 | "Single top-level key" is false: event lines carry several keys | cd-B3 (downgraded) |
| N2 | 350–353, 494–495 | `handle_agent_requests` in `CoreSet::Ui` runs after `Sync`, so its effect renders one frame late | des-N5, cd-M11 |
| N3 | 402–404, 635–638 | The two-system `Esc` chain can clear both the selection and the highlight on one press | ir-M6, cd-N1 |
| N4 | 234–236, 304, 503–505 | 5000 long IRIs exceed the 1 MiB request cap, and the viewer cannot cap its reader because of snapshots | des-N4, ir-N2, cd-M1 |
| N5 | 506–512 | Viewer responses are a prompt-injection path. Validate the screenshot and cap strings | des-N7 |
| N6 | 107–109, 598–602 | A bind failure is invisible: the tools report `not_connected` forever with no reason | des-N8, cd-N8, cmp-16 |
| N7 | 265–269, 411–423 | Pending-entry cleanup on MCP cancellation, the order of parallel calls, and screenshots taken mid-tween | des-N8 |
| N8 | 247–258, 564–566, 579–596 | Core API details: `dispatch` signature, `tool_defs`, serde tagging, `Option` vs `NullViewerControl`, parity test | ir-M14, ir-N6, zo-4, cd-N9 |
| N9 | 324–336, 378–382, 583, 604–609 | Missed reuse: escaping helper, `interact::short`/`display_literal`, test helpers, `LookTo` easing | zo-5, cd-N4, cd-N6, cd-M9, cd-M4 |
| N10 | 123, 366–368, 638 | `FeedEntry` cannot carry "agent · show 14 (isolate)", and the two examples differ | ir-N5, cd-N7 |
| N11 | 611–616 | `connection.rs` is Bevy-free, so `spawn` should return a plain sender, not `AgentOutbox` | cd-N2 |
| N12 | 389–390 | `F` during an in-flight tween is overwritten. The source of `frame_target` positions is unnamed | cd-N11 |
| N13 | 553–554 | Phase 2 exit cites CUJ 1, which needs focus (Phase 3). The old-core compatibility test has no file | ir-N3, ir-N4 |
| N14 | 494–496 | The latency NFR ignores the write-path runtime stall (batch-ingestion F9) | zo-9 |
| N15 | 357–358, 367–368, 645–649 | Missing concrete values: material colours, chip `Hovered`, the core's tokio `time` feature | ir-N7 |
| N16 | 27–39 | "Three documents" heads a table that lists four. The batch-ingestion paragraph is in the present tense and overclaims | str-3, sty-N1, sty-N2, cmp-8, des-N9, cd-N10, cd-N12 |
| N17 | 31, 202, 271, 318, 535–541 | Companion docs are unlinked, "base design" is ambiguous, and cross-reference style is inconsistent | str-4, str-5, cmp-7 |
| N18 | 227–228 | "Never reorders the meaning of two ids" is unclear. Responses can arrive out of order | sty-M6, cmp-9, ic-F9c |
| N19 | 222–225, 291–294 | Invalid JSON inside ```` ```json ```` fences | str-7 |
| N20 | 437–488, 516 | Future Improvements is nested under NFRs. The Pros/Cons format is uneven, and sub-decisions list no alternatives | str-6, str-9, str-10 |
| N21 | 103, 304, 447, 503, 557, 669 | Mechanical: line wrap, 5000 vs 5,000, "labels density" | str-11, str-12, sty-N13 |
| N22 | 47–60, 71–72, 81–89, 122, 157–160, 322, 335–337, 397–398, 427–431, 439, 450–451, 510–511 | Wording calibration batch | sty-N4…N13, cmp-13, cmp-14, cmp-17 |
| N23 | 12, 238, 344–350, 359, 425 | Project-internal identifiers are used without a gloss. The relation between project graph and named graph is never stated | cmp-10, cmp-12 |
| N24 | 131–132, 247–258, 309, 326–330, 357–358, 378–382 | Altitude drops in the Technical Plan, the unused `max_side`, `search … show` chaining | zo-6, zo-7, zo-8 |

--------------------------------------------------------------------------------

## Full Discrepancy Texts

### B1. `read_line` is not cancel-safe inside the planned `select!` loops; the caps cannot be enforced - Severity: Blocker
*   **Reviewer Lens:** `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 234–236 (line caps), 275–278 (single writer), 575–577 (file 2), 613–616 (file 8), 645–649 (file 14).
*   **Discrepancy Description:**
    *   Both sides read with `reader.read_line(&mut line)`: viewer_channel.rs:77, and connection.rs:136-140, which also calls `line.clear()` at the top of each loop iteration.
    *   tokio 1.52.1 documents `read_line` as "not cancellation safe … some data may have been partially read, and this data is lost" (async_buf_read_ext.rs:130-138).
    *   Today the hazard is latent: the core reads only the first line, and the viewer's only other `select!` arm returns.
    *   The plan changes that. The core gets a response-reader arm next to the 200 ms `resync.tick()` and event arms, and the viewer gets an outbound-response arm. Any multi-MiB line (a snapshot or a base64 screenshot) that spans one wake-up is then truncated and lost.
    *   `read_line` also buffers without bound, so the "≤ 1 MiB request / ≤ 8 MiB response, a line over the cap is dropped" rule has no mechanism.
*   **Rationale & Recommendation:**
    *   Specify a cancel-safe, length-capped framed reader on both sides: `tokio_util::codec::FramedRead` with `LinesCodec::new_with_max_length`, polled through `StreamExt::next()`.
    *   On `MaxLineLengthExceeded`, discard the line and continue.
    *   The core already has `tokio-util` with `codec` (core Cargo.toml:22). The viewer needs it added in file 14.
    *   Snapshot lines are unbounded, so the viewer's reader cap must either exempt snapshots or be large. See N4.

### B2. Reading and writing in one task can deadlock the connection - Severity: Blocker
*   **Reviewer Lens:** `egm-design`
*   **Affected Scope:** Lines 275–278, 346–348, 575–577, 614–616.
*   **Discrepancy Description:**
    *   The core's `handle_connection` stays the single writer and also reads responses in the same `select!`. The viewer writes responses with `write_half.write_all` inside a branch body of its read loop.
    *   Deadlock scenario: the viewer is writing an up-to-8 MiB screenshot response while the core is in `write_all` of a large snapshot (viewer_channel.rs:107, 168) or an event burst.
    *   Both TCP send buffers fill and neither side reads.
    *   The request times out, but the connection stays wedged and store events stop. There is no recovery path short of the peer closing.
*   **Rationale & Recommendation:**
    *   Split each side into a reader task and a writer task.
    *   Core: the reader routes `response` lines into the hub's `pending` map, and the writer owns events, snapshots and requests.
    *   Viewer: the reader feeds `Inbox`, and a writer drains the outbox.
    *   Add a file 7 test that sends a large response while a large snapshot is being written.

### B3. Reconnect-on-`changed` collides with control; fix it at the source - Severity: Blocker
*   **Reviewer Lens:** `egm-zoom-out`, `egm-inventive-catalyst`, `egm-design`, `egm-codebase-details`, `egm-comprehension`
*   **Affected Scope:** Lines 36–39 (Overview hand-off), 261–274 (target viewer, pending map, reconnects), 497–499 (timeouts), 533–544 (required companion changes).
*   **Discrepancy Description:**
    *   The viewer closes and reopens its TCP connection on every `{"event":"changed"}` (connection.rs:147-155).
    *   The core emits `Changed` for every pattern-based `update_sparql`. The sibling batch-ingestion design emits it for any commit above `event_changed_threshold` (50,000) and for replace (batch-ingestion.md:297-300, 385).
    *   That design's own CUJ is "`ingest_commit`, then `viewer_show`" (batch-ingestion.md:125, 305, 455), and lines 36–39 of the target promote it. That sequence lands in the reconnect window every time a large commit happens.
    *   The design patches the symptom: a controllable-connection list, promotion, a 2 s re-registration wait and a one-shot retry.
    *   The re-registration point is never stated. If registration happens after the snapshot, a > 50k-triple snapshot query plus serialization can exceed the 2 s wait plus the 3 s timeout.
    *   On `Lagged`, the core closes the socket (viewer_channel.rs:115-117), and the viewer goes through a ≥ 1 s backoff, not an immediate reconnect (connection.rs:142, 173).
    *   Lines 36–39 describe the interaction as "bounded StoreEvents keep the feed responsive", which misses this.
    *   The core already re-snapshots in-band on an active-graph change (viewer_channel.rs:100-108). `sparql-mcp-viewer.md:500` lists "Resnapshot on the same connection" as a future option.
*   **Rationale & Recommendation:**
    *   For controllable viewers, resync on `changed` and `Lagged` in-band: the viewer sends `{"cmd":"snapshot"}` on the open connection, or the core pushes one, and the connection is not reopened.
    *   Add this to *Required changes to the companion designs*.
    *   Keep reconnect-plus-retry only for real disconnects.
    *   Register with the hub right after the handshake, before the snapshot.
    *   Rewrite lines 36–39 to describe the real interaction.

### B4. User camera input is not reliably detected; the tween fights input and momentum - Severity: Blocker
*   **Reviewer Lens:** `egm-codebase-details`, `egm-design`, `egm-implementation-readiness`
*   **Affected Scope:** Lines 145–148 (CUJ 3), 378–382 (`CameraTween` ordering), 396–401 (user takeover), 626–628 (file 10).
*   **Discrepancy Description:**
    *   The doc says `is_actively_controlled()` is true while "the user is dragging or scrolling". Bevy 0.20.1 returns `!self.current_motion.is_zooming_only() && (…)` (component.rs:195-203). It explicitly excludes zoom, and it stays true for an 80 ms debounce of momentum.
    *   (a) Wheel zoom therefore neither cancels a tween nor triggers `user_controlling_camera`, which breaks author decision 2 for the most common camera gesture.
    *   (b) "Ordered before `update_camera_positions`" is not enough. The vendored input chain (`PanOrbitCameraInputMessage::receive_messages`, camera_input.rs:56-68) also runs in `PreUpdate`. If the tween writes `Transform` before that chain, the user's anchor is computed from a stale pose, so CUJ 3's "no jump" fails and cancellation lags one frame.
    *   (c) `CurrentMotion::Momentum` keeps applying deltas after release (component.rs:375-385), and a new tween fights it. Bevy's own `LookTo` sets `current_motion = Stationary` when it starts (look_to.rs:139-140).
*   **Rationale & Recommendation:**
    *   Detect takeover with `current_motion.is_user_controlled()`, which includes zoom-only motion.
    *   Order the tween system `.after(<input chain>).before(PanOrbitCamera::update_camera_positions)`.
    *   Set `Stationary` when a tween starts.
    *   Remove the tween without writing `Transform` on any frame of user control.

### M1. One-shot retry is unsafe for non-idempotent ops; stale ids are unhandled - Severity: Major
*   **Reviewer Lens:** `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-inventive-catalyst`
*   **Affected Scope:** Lines 265–274.
*   **Discrepancy Description:**
    *   A request may already be in the viewer's `Inbox`, or already applied, when the connection drops.
    *   A retry then applies relative ops twice: `orbit` by `yaw_deg`, `zoom` by `factor`.
    *   Responses produced during a disconnect can be written on the new connection with ids the hub no longer tracks.
    *   The doc does not say whether `pending` is global or per connection, whether a retry reuses the id, or what happens to unknown ids.
    *   `Timeout` maps to `applied:false` even though the command may still be applied later.
*   **Rationale & Recommendation:**
    *   Retry only idempotent ops, or send absolute target poses for `orbit` and `zoom`.
    *   Give each retry a new id.
    *   Make `pending` hub-global, and drop unknown ids at debug level.
    *   Clear the viewer's outbox backlog on reconnect.
    *   Report `Timeout` as "unknown", not "not applied".

### M2. Requests can overtake store events (read-your-writes) - Severity: Major
*   **Reviewer Lens:** `egm-design`, `egm-inventive-catalyst`
*   **Affected Scope:** Lines 36–39, 275–278, 304, 350–353.
*   **Discrepancy Description:**
    *   Writes travel on the broadcast and requests on the hub `mpsc`, and `tokio::select!` picks randomly among ready branches.
    *   After a commit, the insert event and the `viewer_show` request can both be ready, and the request may be written first.
    *   The viewer then resolves IRIs before the nodes exist, returns them in `missing`, and frames on absent positions.
*   **Rationale & Recommendation:**
    *   In the writer, use `biased;` with the event arm first, and drain `try_recv` before writing a request.
    *   As an alternative, add a monotonic `seq` on events and an `after_seq` on requests.
    *   Add a file 7 test: insert, then `show` the same IRI, and expect `missing: []`.

### M3. The timeout ceiling is miscomputed and its scope is undefined - Severity: Major
*   **Reviewer Lens:** `egm-structural`, `egm-style`, `egm-comprehension`, `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-inventive-catalyst`
*   **Affected Scope:** Lines 270–274, 423, 497–499.
*   **Discrepancy Description:**
    *   With per-attempt timeouts, the worst case is 3 + 2 + 3 = 8 s for an ordinary op and 10 + 2 + 10 = 22 s for a screenshot. Neither matches "about 15 s".
    *   `viewer_search scope:both show:*` chains a store query, which has no deadline, then `search_view` and `show`, each with its own budget.
    *   The doc does not say whether the retry gets a fresh budget.
*   **Rationale & Recommendation:** Define one total per-tool deadline that covers waits, retries and store work, and state the real numbers.

### M4. Partial application vs `Rejected`; the envelope is incomplete - Severity: Major
*   **Reviewer Lens:** `egm-style`, `egm-comprehension`, `egm-design`, `egm-implementation-readiness`, `egm-inventive-catalyst`, `egm-structural`
*   **Affected Scope:** Lines 166–167, 249–253, 289–298, 396–401.
*   **Discrepancy Description:**
    *   `user_controlling_camera` is listed as a `Rejected` outcome (`applied:false`), yet "any highlight still applies". An agent reading `applied:false` will re-send the command or tell the user that nothing changed.
    *   `ViewerOutcome::Applied` has no `reason`.
    *   The envelope shows no `reason`.
    *   CUJ 5 omits `result` and `user`, which contradicts "every tool returns … this envelope".
*   **Rationale & Recommendation:**
    *   Add `reason: Option<String>` to `Applied`.
    *   Return `applied:true, camera_moved:false, reason:"user_controlling_camera"` when only the camera step is skipped. Keep `applied:false` for `viewer_camera` itself.
    *   Show `reason?` in the envelope, and state which fields are omitted when the viewer is `not_connected` or `timeout`.

### M5. Bad vs malformed arguments - Severity: Major
*   **Reviewer Lens:** `egm-style`, `egm-comprehension`
*   **Affected Scope:** Lines 296–298, 304.
*   **Discrepancy Description:** One class of arguments gives a soft `Rejected` and the other raises `invalid_params`, with no boundary between them. For example, a reader cannot tell which path 5001 IRIs, an unknown `action`, or `frame` without `iris` takes.
*   **Rationale & Recommendation:** Define "malformed" as failing the schema or the core-side limits, rejected before contacting the viewer. Define "unappliable" as well-formed but rejected by the viewer, with `applied:false` plus `reason`. Give examples of each.

### M6. "Highlight ring" vs material swap - Severity: Major
*   **Reviewer Lens:** `egm-style`, `egm-comprehension`, `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 120–121 vs 357–362.
*   **Discrepancy Description:**
    *   CUJ 1 says the nodes "keep their material and gain a highlight ring". The Rendering section replaces the material with `agent_highlight_material`.
    *   No ring mesh or entity exists in the design or the code (render.rs:380-400).
    *   The Phase 2 manual check has no clear pass condition.
*   **Rationale & Recommendation:** Align CUJ 1 with the material swap, which matches the code.

### M7. Isolate edge fading will not refresh; precedence is not unit-testable - Severity: Major
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 357–362, 554, 630–633.
*   **Discrepancy Description:**
    *   Edges are bucketed by `update_edge_meshes` (render.rs:299-338, `PostUpdate`), not by `apply_node_materials`.
    *   `update_edge_meshes` early-returns unless the layout is awake, the graph generation changed, or `ClassFilter` changed. Toggling `Isolate` on a settled layout therefore fades nothing.
    *   Material precedence is inline inside a Bevy system, so Phase 2's "unit tests for material precedence" have nothing to call.
*   **Rationale & Recommendation:**
    *   `update_edge_meshes` takes `Option<Res<AgentHighlight>>`, adds `is_changed()` to its dirty check, and gets the isolate rule.
    *   Extract a pure `node_material_choice(…)` function and unit-test it.

### M8. `look_at`, `distance` and `reset` have no source - Severity: Major
*   **Reviewer Lens:** `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 302, 383–388.
*   **Discrepancy Description:**
    *   `PanOrbitCamera` keeps no persistent pivot. An anchor exists only during `UserControlled` (component.rs:164-170), and otherwise there is only `last_anchor_depth`.
    *   `viewer_status` returns `look_at` and `distance`, `orbit` rotates "about the current look-at point", and `zoom` scales "the distance to the look-at point". None of these is defined after a user pan.
    *   The `reset` "start pose" is a literal inside `spawn_camera` (camera.rs:42).
*   **Rationale & Recommendation:**
    *   Add a `CameraFocus { look_at }` resource, set by the frame operations.
    *   When it is invalid, fall back to `position + forward · |last_anchor_depth|`.
    *   Extract a `START_POSE` const.

### M9. `in_view` for store hits; false "literal-only" claim - Severity: Major
*   **Reviewer Lens:** `egm-comprehension`, `egm-design`, `egm-inventive-catalyst`, `egm-implementation-readiness`, `egm-codebase-details` (proposed Blocker), `egm-zoom-out`
*   **Affected Scope:** Lines 70–73, 229–231, 308, 337–340.
*   **Discrepancy Description:**
    *   The core has no copy of the viewer graph, and no op asks the viewer whether an IRI is drawn.
    *   With `both`, `in_view` depends on `search_view` results, which are capped by `limit` (default 25) and match different fields. A drawn store hit outside the view top-25 is reported `in_view:false`, and the `show` chain then leaves it out.
    *   With `store`, the doc gives no mechanism at all.
    *   The rationale at lines 339–340, "literal-only subjects" are not drawn, is false. `ViewerGraph::insert_triple` creates a node for every subject (graph.rs:215) and stores literal objects as attributes (graph.rs:242-248).
*   **Rationale & Recommendation:**
    *   Let `search_view` take `also_check: [iri]` and return a `drawn` subset (no new op), or add a `resolve` op.
    *   Delete the false parenthetical and name the real causes: a reconnect window, or a blank-node subject.
*   **Severity note:** cd proposed Blocker under its "contradicts code" rule. The synthesizer set Major because the error is a single parenthetical, and the real gap (no membership mechanism) is design incompleteness.

### M10. IRI wire form and merge key - Severity: Major
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`, `egm-comprehension`
*   **Affected Scope:** Lines 316–318, 324–340, 354–356, 581–584.
*   **Discrepancy Description:**
    *   `ViewerGraph.index`, `Selection`, `ClassFilter.hidden` and the planned `AgentHighlight.terms` are keyed by N-Triples `<iri>` strings (graph.rs:27-34, interact.rs:33, 43). Store results are bare IRIs (viewer_channel.rs:185).
    *   The doc says only that the core "accepts `<…>` or a bare IRI". It does not say what travels on the wire, what `both` merges on, or which form `hide_classes` and `show_classes` take.
    *   The values of `matched_on` are not listed.
*   **Rationale & Recommendation:** State that every IRI on the wire is bare, that the viewer wraps and unwraps at its lookups, and that the merge key is the bare IRI. List the values of `matched_on`.

### M11. Store search: synchronous, unbounded, under-specified - Severity: Major
*   **Reviewer Lens:** `egm-design`, `egm-codebase-details`, `egm-implementation-readiness`, `egm-structural`
*   **Affected Scope:** Lines 331–337, 494–496.
*   **Discrepancy Description:**
    *   `CONTAINS(LCASE(…))` over every literal in every named graph is a full scan, and `code-import --with-source` stores whole files as `cbm:sourceCode` literals (main.rs:76-81).
    *   `SparqlStore::query` is synchronous (sparql_store.rs:159). Called from async `call_tool`, it blocks a runtime worker, so the store search cannot "run at once" with the view search.
    *   No deadline covers it.
    *   "Best `rdfs:label`/`skos:prefLabel`" has no tie-break, and it differs from the viewer's own label rule (`rdfs:label`, then `smc:label`; graph.rs:300-313).
    *   The doc gives no exact query.
*   **Rationale & Recommendation:**
    *   Run the query in `spawn_blocking` under the tool deadline.
    *   Cap literal length or allow-list predicates.
    *   Align the label predicates with the viewer, and give the SELECT text.

### M12. Screenshot: byte budget, silent oversize, HDR alpha, minimized window - Severity: Major
*   **Reviewer Lens:** `egm-style`, `egm-inventive-catalyst`, `egm-codebase-details`, `egm-design`, `egm-implementation-readiness`
*   **Affected Scope:** Lines 234–236, 411–423, 503–505, 557.
*   **Discrepancy Description:**
    *   "Usually well under 2 MiB" is unmeasured. The 8 MiB line cap allows about 6 MiB of PNG, which is above common downstream image limits (about 5 MB).
    *   An oversized line is dropped, and the agent waits out a silent 10 s timeout.
    *   The Phase 5 "PNG size check" has no threshold.
    *   The camera is `Hdr` (camera.rs:46). Bevy's own `save_to_disk` discards alpha because "the alpha channel … stores brightness values" under HDR (screenshot.rs:198). Converting to RGBA8 can produce a semi-transparent PNG.
    *   A minimized window may never fire `ScreenshotCaptured`, which ends in a 10 s timeout. This is unverified on Bevy 0.20.1.
*   **Rationale & Recommendation:**
    *   Set a hard byte budget that the viewer checks before sending, stepping `max_side` down if needed, with a fast `payload_too_large` error.
    *   Convert with `to_rgb8`.
    *   Fail fast with `window_not_visible`.
    *   Give the Phase 5 check a numeric threshold.

### M13. File 14 dependency choice is left open - Severity: Major
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 645–649.
*   **Discrepancy Description:**
    *   File 14 says "Bevy `png` feature or `image` crate … whichever … (verify in Phase 5)". That is a decision deferred to the implementer.
    *   Verified facts:
        *   `Image::try_into_dynamic()` is unconditional in bevy_image 0.20.1 (image_texture_conversion.rs:180) and handles `Bgra8UnormSrgb`;
        *   bevy_image uses `image` 0.25 without re-exporting it;
        *   `image` 0.25.10 and `base64` 0.23.1 are in Cargo.lock.
*   **Rationale & Recommendation:** Name `image = { version = "0.25", default-features = false, features = ["png"] }` and `base64 = "0.23"`, plus `tokio-util` (B1). Remove "verify in Phase 5".

### M14. The file list is incomplete - Severity: Major
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`, `egm-style`, `egm-comprehension`
*   **Affected Scope:** Lines 558, 560–661.
*   **Discrepancy Description:** These paths are missing from the file list:
    *   `infrastructure/mod.rs`, which must re-export `ViewerHub` (mod.rs:12).
    *   The callers of `serve_viewer_channel` / `spawn_viewer_channel`, which break when the signature changes: core `tests/viewer_channel.rs:57,124,164`, `examples/viewer_load.rs:76`, and viewer `tests/headless_e2e.rs:75`.
    *   The exhaustive `ViewerMsg` matches, which break when a variant is added: plugin.rs:133-160, ingest.rs:78-105 and 137-156, headless.rs:50-56, headless_e2e.rs:34-46. `ViewerMsg` derives `Clone`, so `AgentRequest` must too.
    *   The skill paths. "`sparql-first`" does not exist. The real files are `plugins/kb-workbench/skills/{kb-workbench,sparql-first-code}/SKILL.md`.
*   **Rationale & Recommendation:** Add the paths and the required match arms.

### M15. Headless wiring - Severity: Major
*   **Reviewer Lens:** `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 342–348, 425–431, 640–643.
*   **Discrepancy Description:**
    *   Headless builds only `CorePlugin` (headless.rs:34-37). `AgentPlugin` is added only to `ViewerPlugin`.
    *   `connection::spawn`, which creates the outbox, runs in `CorePlugin::build` (plugin.rs:57).
    *   A headless viewer would send `control:1`, receive requests and never answer them, so every request times out.
    *   The headless `status` fields are not stated.
*   **Rationale & Recommendation:**
    *   `CorePlugin` inserts the outbox.
    *   A core-level agent subset (`status`, `search_view`, and an `unsupported_headless` fallback) runs in both modes.
    *   `AgentPlugin` adds the windowed handlers.
    *   Headless `status` returns `camera:null` and `visible:null`.

### M16. The headless e2e harness is infeasible as written - Severity: Major
*   **Reviewer Lens:** `egm-codebase-details` (proposed Blocker), `egm-implementation-readiness`
*   **Affected Scope:** Lines 92–93, 651–655, 667.
*   **Discrepancy Description:**
    *   The viewer's dev-dependency is `rmcp = { … features = ["macros"] }` (Cargo.toml:45). It has no `client` and no transport, so "already a dev-dependency" is misleading.
    *   `headless_e2e.rs` drives `connection::spawn` with no Bevy app (lines 1-4, 94).
    *   `headless::run` blocks in `app.run()` and has no stop hook.
    *   The doc does not say whether the core runs in-process or as a binary.
*   **Rationale & Recommendation:**
    *   Run `SparqlMcpServer` in-process over `tokio::io::duplex` with `with_viewer_control(hub)`.
    *   Spawn the headless viewer as a child process via `env!("CARGO_BIN_EXE_sparql-mcp-viewer")`.
    *   Declare the rmcp features `["macros", "client"]` explicitly.
*   **Severity note:** the synthesizer downgraded cd-B4 to Major because this is a plan gap, not a false statement about shipped behaviour.

### M17. User-activity semantics undefined; check-then-act race; overclaim - Severity: Major
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-inventive-catalyst`, `egm-style`
*   **Affected Scope:** Lines 79–83, 311–314, 394, 405–409, 620–622, 637.
*   **Discrepancy Description:**
    *   The doc does not say what counts as "input in the last 3 s".
    *   Agent and user write the same `ClassFilter` and `Selection`, so change detection cannot tell them apart.
    *   It does not say when `overrode_last_agent_command` resets, or whether it is computed before or after the current command.
    *   Agent turns take longer than 3 s, so `user.active` is nearly always stale when the agent acts on it.
    *   "The user always wins" overclaims: the only enforced protection is camera input during active control.
*   **Rationale & Recommendation:**
    *   Define `UserActivityTracker` (fields, the user input sources that set it, the reset rule).
    *   Add a monotonic `user.epoch` and an optional `if_user_epoch` precondition, so the agent can avoid overwriting changes it never saw. This keeps "the agent decides".
    *   Calibrate line 394.

### M18. The controlled-viewer target is unstable and silently takeable - Severity: Major
*   **Reviewer Lens:** `egm-inventive-catalyst`, `egm-design`, `egm-structural`
*   **Affected Scope:** Lines 105–106, 261–264, 506–512.
*   **Discrepancy Description:**
    *   "Most recently connected wins" combines with reconnect-on-`changed`: with two viewers open, control flips each time a `changed` makes both reconnect.
    *   A retry can land on a different window from the one the agent was driving.
    *   Any local process that connects with `control:1` becomes the target at once and silently. The security note covers fake answers, not this takeover.
*   **Rationale & Recommendation:**
    *   Add a per-process `instance` id to the handshake. Reconnects from the same instance keep the slot.
    *   Report `instance` in `viewer_status`.

### M19. No Risks & Mitigations section - Severity: Major
*   **Reviewer Lens:** `egm-structural`
*   **Affected Scope:** Between line 488 (end of Alternatives) and line 492 (NFRs). The section is missing.
*   **Discrepancy Description:** The feature-spec standard requires failure modes and their mitigations. The target spreads them across the NFR bullets, and several have no mitigation: the store-search full scan, a silent timeout on an oversized response, control switching between viewers, and the spoofing viewer.
*   **Rationale & Recommendation:** Add a `## Risks & Mitigations` table that collects the risks behind B1–B4, M1, M11, M12 and M18.

### M20. The user cannot restore the previous view - Severity: Major
*   **Reviewer Lens:** `egm-inventive-catalyst`
*   **Affected Scope:** Lines 149–151, 363–365, 402–404, 480–488.
*   **Discrepancy Description:**
    *   Agent `filter`, `select` and camera moves overwrite the user's state.
    *   `Esc` and Clear undo only the highlight, and `reset` goes to the app's start pose.
    *   "Undo with the same UI" means rebuilding the previous filter by hand.
*   **Rationale & Recommendation:** Add a one-level `UserViewCheckpoint` (camera, filter, selection), saved before the first agent mutation after any user input. The chip's "Restore my view" restores it.

### M21. The v1 tool surface goes beyond the CUJs - Severity: Major
*   **Reviewer Lens:** `egm-zoom-out`
*   **Affected Scope:** Lines 66–69, 300–309, 363–365, 554–555.
*   **Discrepancy Description:**
    *   No CUJ uses `viewer_filter`, a standalone `viewer_select`, or `orbit`/`reset`.
    *   `viewer_select` duplicates `viewer_show`'s `select?` argument.
    *   Filter and select are what drive the shared-state discussion (Alternative E) and the tracking of filter and selection overrides.
*   **Rationale & Recommendation:** Defer `viewer_filter`, the standalone `viewer_select`, and `orbit`/`reset` to Future Improvements, or add CUJs that need them.

### M22. Search is implemented twice - Severity: Major
*   **Reviewer Lens:** `egm-zoom-out`
*   **Affected Scope:** Lines 229–231, 320–340, 621–622.
*   **Discrepancy Description:**
    *   The design has a viewer-side matcher with five-tier ranking and a store-side SPARQL matcher with no ranking, plus merge logic. The two will disagree on the same text.
    *   The drawn graph is a snapshot of the active graph, which the store also holds.
    *   Ground truth fixes the scope of search (view and store), not the number of engines.
*   **Rationale & Recommendation:** Run search once in the core, and compute `in_view` through `resolve_iris`. Alternatively, justify why two engines are needed.

### N1. "Single top-level key" is false - Severity: Nit
*   **Reviewer Lens:** `egm-codebase-details` (proposed Blocker)
*   **Affected Scope:** Lines 201–202.
*   **Discrepancy Description:** Event lines are internally tagged and carry several top-level keys, for example `{"event":"insert","graph":…,"triples":…,"kinds":…}` (connection.rs:250). The decoder dispatches on which key is present (connection.rs:219-231).
*   **Rationale & Recommendation:** Say "a discriminating top-level key (`snapshot`, `event`, `request`, `response`)" and give the order in which the decoder checks them. Downgraded from Blocker: this is a one-phrase fix with no design impact.

### N2. `CoreSet::Ui` placement adds a frame of lag - Severity: Nit
*   **Reviewer Lens:** `egm-design`, `egm-codebase-details`
*   **Affected Scope:** Lines 350–353, 494–495.
*   **Discrepancy Description:** The sets are chained Net → Graph → Sync → Ui (plugin.rs:64-67), and `apply_node_materials` runs in Sync. A request handled in Ui therefore renders on the next frame, and the response's visible count describes the old state.
*   **Rationale & Recommendation:** Run the handler in `CoreSet::Graph`, after `apply_to_graph`.

### N3. The `Esc` chain spans two systems - Severity: Nit
*   **Reviewer Lens:** `egm-implementation-readiness` (proposed Major), `egm-codebase-details`
*   **Affected Scope:** Lines 402–404, 635–638.
*   **Discrepancy Description:** `clear_selection_on_escape` (interact.rs:209-213) is its own system. A second system can see the selection already cleared in the same frame, so one press clears both the selection and the highlight.
*   **Rationale & Recommendation:** Use one `escape_chain` system that applies exactly one effect per press.

### N4. The 1 MiB request cap vs 5000 IRIs - Severity: Nit
*   **Reviewer Lens:** `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 234–236, 304, 503–505.
*   **Discrepancy Description:** 5000 IRIs at about 210 bytes each exceed 1 MiB, so the call times out silently. The viewer cannot cap its reader at 1 MiB, because snapshot lines are unbounded.
*   **Rationale & Recommendation:** The core enforces the request cap at send time and returns `request_too_large`.

### N5. Viewer responses are untrusted input - Severity: Nit
*   **Reviewer Lens:** `egm-design`
*   **Affected Scope:** Lines 506–512.
*   **Discrepancy Description:** Labels and images from the viewer go straight into the agent's context, which is a prompt-injection path.
*   **Rationale & Recommendation:** The core validates that a screenshot is a real PNG within `max_side`, and caps hit counts and string lengths. Tool descriptions label viewer-returned text as data.

### N6. A bind failure is invisible - Severity: Nit
*   **Reviewer Lens:** `egm-design`, `egm-codebase-details`, `egm-comprehension`
*   **Affected Scope:** Lines 107–109, 284, 598–602.
*   **Discrepancy Description:**
    *   main.rs:372 drops the `JoinHandle` of `spawn_viewer_channel`, so a bind error is never logged.
    *   A second session lists the tools, which then say `not_connected` forever with no reason.
    *   Line 284 also omits that the tools are not listed when `bind` does not parse.
*   **Rationale & Recommendation:** Log the bind error, and report `listener: "bind_failed"` in `viewer_status`. State the listing rule precisely.

### N7. Cancellation, parallel calls, and screenshots mid-tween - Severity: Nit
*   **Reviewer Lens:** `egm-design`
*   **Affected Scope:** Lines 265–269, 411–423.
*   **Discrepancy Description:**
    *   The doc does not say whether a dropped MCP future removes its `pending` entry.
    *   Parallel `viewer_*` calls have no defined order.
    *   A screenshot taken during a `CameraTween` captures a frame in mid-flight.
*   **Rationale & Recommendation:** Use a drop guard on `pending` entries, and document that calls are processed in arrival order. Have the screenshot wait for any tween to finish, bounded by its timeout.

### N8. Core API details - Severity: Nit
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-zoom-out`, `egm-codebase-details`
*   **Affected Scope:** Lines 247–258, 564–566, 579–596.
*   **Discrepancy Description:**
    *   `viewer::dispatch(…)` has no signature, and no tool-definitions function is named.
    *   `ViewerOp`'s serde tagging is unspecified.
    *   `Option<Arc<dyn ViewerControl>>` and `NullViewerControl` encode the same fact twice. The existing pattern is `event_sink: Arc<dyn EventSink>` defaulting to `NullEventSink` (server.rs:42, 62).
    *   No parity test covers the viewer tools (server.rs:347-365 covers only the core tools).
*   **Rationale & Recommendation:** Pin the signatures and the serde attributes, follow the EventSink pattern, and add the parity test.

### N9. Missed reuse - Severity: Nit
*   **Reviewer Lens:** `egm-zoom-out`, `egm-codebase-details`
*   **Affected Scope:** Lines 324–336, 378–382, 583, 604–609.
*   **Discrepancy Description:** The design writes new code where existing code fits:
    *   `cbm::turtle::escape` (turtle.rs:185-191) already escapes `\ " \n \r \t`, and `importer::escape_literal` is a weaker copy;
    *   `interact::short` and `interact::display_literal` already unquote N-Triples literals before matching;
    *   `tests/viewer_channel.rs:22-47` has the fake-viewer helpers;
    *   `LookTo` has bezier easing;
    *   `interact::button_node` exists for the chip.
*   **Rationale & Recommendation:** Reference these, and consolidate the escaping helpers into `sparql_string_literal`.

### N10. Feed entry shape - Severity: Nit
*   **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`
*   **Affected Scope:** Lines 123, 366–368, 638.
*   **Discrepancy Description:** `FeedEntry` holds only a `Copy` `kind` and a `triple_count`, and `label()` returns `&'static str` (ingest.rs:19-47). "agent · show 14 (isolate)" needs a detail field. CUJ 1 and line 367 give different examples.
*   **Rationale & Recommendation:** Define `FeedKind::Agent(AgentFeedOp)` and the formatting rule.

### N11. `connection.rs` is Bevy-free - Severity: Nit
*   **Reviewer Lens:** `egm-codebase-details`
*   **Affected Scope:** Lines 611–616.
*   **Discrepancy Description:** The module doc of connection.rs (lines 1-3) says it has no Bevy dependency, so `spawn` cannot return the Bevy `AgentOutbox` resource.
*   **Rationale & Recommendation:** `spawn` returns a plain `mpsc` sender, and `agent.rs` wraps it in `AgentOutbox`.

### N12. `F` vs an in-flight tween - Severity: Nit
*   **Reviewer Lens:** `egm-codebase-details`
*   **Affected Scope:** Lines 389–390.
*   **Discrepancy Description:** An active tween overwrites `F`'s `Transform` on the next `PreUpdate`. The doc does not say where `frame_target` gets its positions from.
*   **Rationale & Recommendation:** `F` removes any tween. `frame_target` takes `LayoutState` positions filtered by the resolved `NodeId`s.

### N13. Phase exit criteria - Severity: Nit
*   **Reviewer Lens:** `egm-implementation-readiness`
*   **Affected Scope:** Lines 553–554.
*   **Discrepancy Description:** The Phase 2 exit runs CUJ 1, which needs `focus:true` camera motion from Phase 3. The "old-core compatibility" test has no file.
*   **Rationale & Recommendation:** Use "CUJ 1 with `focus:false`", and add a connection.rs unit test against an old-style fake server.

### N14. Latency NFR vs the write-path stall - Severity: Nit
*   **Reviewer Lens:** `egm-zoom-out`
*   **Affected Scope:** Lines 494–496.
*   **Discrepancy Description:** Batch-ingestion F9 records that synchronous write tools stall Tokio workers, including the viewer channel. The "< 100 ms" target cannot hold during a long write.
*   **Rationale & Recommendation:** State the assumption and its bound.

### N15. Missing concrete values - Severity: Nit
*   **Reviewer Lens:** `egm-implementation-readiness`
*   **Affected Scope:** Lines 357–358, 367–368, 645–649.
*   **Discrepancy Description:**
    *   `agent_highlight_material` has no colour values.
    *   The chip needs `Hovered` so that `block_controller_over_ui` sees it (camera.rs:102-119).
    *   The core relies on the tokio `time` feature only through feature unification (core Cargo.toml:21).
*   **Rationale & Recommendation:** Give the values, and declare the dependency explicitly.

### N16. Overview count and batch-ingestion paragraph - Severity: Nit
*   **Reviewer Lens:** `egm-structural`, `egm-style`, `egm-comprehension`, `egm-design`, `egm-codebase-details`
*   **Affected Scope:** Lines 27–39.
*   **Discrepancy Description:**
    *   "Three documents" heads a table with four rows.
    *   "The two agent-facing features" are never named.
    *   `ingest_commit` and `event_max_triples` exist only in the sibling design but are written in the present tense.
    *   The sibling design says `event_max_triples` bounds broadcast memory. It does not claim that the feed stays responsive.
*   **Rationale & Recommendation:** Covered by the B3 rewrite, plus "four documents".

### N17. Links and "base design" - Severity: Nit
*   **Reviewer Lens:** `egm-structural`, `egm-comprehension`
*   **Affected Scope:** Lines 31, 202, 271, 318, 535–541; cross-reference style at 99, 106, 232–233, 513.
*   **Discrepancy Description:** `interface-agentic-rust-stylee.md` is not linked. "Base design" is ambiguous. Section references switch between italic and plain text.
*   **Rationale & Recommendation:** Link every companion document, name the document wherever "base design" appears, and use one style for cross-references.

### N18. Response ordering wording - Severity: Nit
*   **Reviewer Lens:** `egm-style` (proposed Major), `egm-comprehension`, `egm-inventive-catalyst`
*   **Affected Scope:** Lines 227–228.
*   **Discrepancy Description:** "Never reorders the meaning of two ids" has no clear meaning. Screenshot responses do arrive after later requests.
*   **Rationale & Recommendation:** Say that responses may arrive out of order and are matched by `id`.

### N19. Invalid JSON in `json` fences - Severity: Nit
*   **Reviewer Lens:** `egm-structural`
*   **Affected Scope:** Lines 222–225, 291–294.
*   **Discrepancy Description:** The `{…}` placeholders and the `"a"|"b"` unions in these blocks are not valid JSON.
*   **Rationale & Recommendation:** Use `text` fences, or give concrete examples.

### N20. Document structure - Severity: Nit
*   **Reviewer Lens:** `egm-structural`
*   **Affected Scope:** Lines 437–488, 516.
*   **Discrepancy Description:**
    *   `### Future Improvements` sits under the NFRs.
    *   Alternatives B, C and D lack Pros, while A and E have them.
    *   Sub-decisions list no alternatives: the store-search mechanism, screenshot transport, and the targeting rule.
*   **Rationale & Recommendation:** Promote the heading to H2, give every alternative a uniform Pros/Cons/verdict, and add a one-line rejected alternative for each sub-decision.

### N21. Mechanical issues - Severity: Nit
*   **Reviewer Lens:** `egm-structural`, `egm-style`
*   **Affected Scope:** Lines 103, 304, 447, 503, 557, 669.
*   **Discrepancy Description:**
    *   Lines 447 and 669 overflow the wrap width.
    *   "5000" and "5,000" are both used.
    *   "labels density" should be "label density".
*   **Rationale & Recommendation:** Fix each in place.

### N22. Wording calibration batch - Severity: Nit
*   **Reviewer Lens:** `egm-style`, `egm-comprehension`
*   **Affected Scope:** Lines 47–60, 71–72, 81–89, 122, 157–160, 322, 335–337, 397–398, 427–431, 439, 450–451, 494–496, 510–511.
*   **Discrepancy Description:**
    *   The first Problem heading does not match its bullet, and the Problem section uses subjective wording.
    *   The Goals use "and/or".
    *   Some sentences are redundant: line 86-87, "as decided for this feature", "rather than assumed away".
    *   "Keeps working as today" is vague.
    *   The `Esc` goal is understated.
    *   "About 0.6 s" should cite the default `duration_ms`.
    *   The 1568 px limit has no rationale.
    *   The escaping helper is unnamed, and "at once" should be "concurrently".
    *   "`search` with `show`" is ambiguous.
    *   The headless rejection list has no stated reason.
    *   "Windows loopback" and "less robust" in Alternative A.
    *   The "< 100 ms" target is never measured.
*   **Rationale & Recommendation:** Apply each reviewer's replacement.

### N23. Glossary - Severity: Nit
*   **Reviewer Lens:** `egm-comprehension`
*   **Affected Scope:** Lines 12, 238, 344–350, 359, 425.
*   **Discrepancy Description:** `Decoded`, `Inbox`, `FrameMessages`, `CoreSet::Ui`, "last-batch pulse" and `--headless` are used without a gloss. The relation between a project and a named graph is never stated.
*   **Rationale & Recommendation:** Add inline glosses, and one Overview sentence on projects and named graphs.

### N24. Altitude, `max_side`, `search … show` chaining - Severity: Nit
*   **Reviewer Lens:** `egm-zoom-out`
*   **Affected Scope:** Lines 131–132, 247–258, 309, 326–330, 357–358, 378–382.
*   **Discrepancy Description:**
    *   The Technical Plan includes implementation detail: the full Rust trait signature, ranking tiers, colours, lerp/slerp.
    *   `max_side` has the same default and maximum, and no CUJ uses it.
    *   Chaining `show` from `viewer_search` adds takeover edge cases.
*   **Rationale & Recommendation:** The reviewer suggests moving this detail down to the implementation plan and dropping both arguments. See the goldfish report for the disposition.

--------------------------------------------------------------------------------

## Traceability Matrix (raw reviewer id → consolidated id)

| Reviewer | Raw → Consolidated |
|---|---|
| cmp | 1→M6, 2→M3, 3→M4, 4→M5, 5→M9, 6→B3, 7→N17, 8→N16, 9→N18, 10→N23, 11→M10, 12→N23, 13→N22, 14→N22, 15→M14, 16→N6, 17→N22 |
| des | B1→B1, B2→B2, M1→M1, M2→M2, M3→B3, M4→B4, M5→M8, M6→M9, M7→M11, M8→M4, N1→M15, N2→M18, N3→M3, N4→N4, N5→N2, N6→M6, N7→N5/M18, N8→N6/N7/M12, N9→N16 |
| str | 1→M19 (also M11, M18), 2→M3, 3→N16, 4→N17, 5→N17, 6→N20, 7→N19, 8→M4, 9→N20, 10→N20, 11→N21, 12→N21 |
| sty | M1→M3, M2→M17, M3→M4, M4→M5, M5→M6, M6→N18, M7→M12, N1→N16, N2→N16, N3→M14, N4–N13→N22 (N13 also N21) |
| zo | 1→B3, 2→M22 (also M9), 3→M21, 4→N8, 5→N9, 6→N24, 7→N24, 8→N24, 9→N14 |
| ir | B1→B1, M1→M13, M2→B4, M3→M8, M4→M6, M5→M7, M6→N3, M7→M4, M8→M10/M9, M9→M1, M10→M14, M11→M15, M12→M17, M13→M16, M14→M11/N8, N1→M3, N2→N4, N3→N13, N4→N13, N5→N10, N6→N8, N7→N15, N8→M12 |
| cd | B1→B4, B2→M9, B3→N1, B4→M16, M1→B1/N4, M2→M1, M3→B3, M4→B4/N9, M5→M8, M6→M6/M7, M7→M3, M8→M11, M9→M14/N9, M10→M12/M13, M11→N2/M15, N1→N3, N2→N11, N3→M10, N4→N9, N5→M11, N6→N9, N7→N10, N8→N6, N9→N8, N10→N16, N11→N12, N12→N16 |
| ic | F1→B3/M1, F2→M2, F3→M18, F4→M17, F5→M20, F6→M12, F7→M12, F8→M9, F9a→M3, F9b→M4, F9c→N18 |
