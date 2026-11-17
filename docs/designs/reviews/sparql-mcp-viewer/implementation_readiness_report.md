# Stage 9: Implementation Readiness Test (Round 1)

*   **Target**: `docs/designs/sparql-mcp-viewer.md`, DetailedRange lines
    492–830.
*   **Standard**: `implementation-readiness-standard.md`, applied by
    `egm-implementation-readiness` and `egm-codebase-details`.
    `egm-style` used its built-in criteria.
*   **GoldfishContext**:
    *   `interface-agentic-rust-stylee.md`;
    *   root `Cargo.toml` and core `Cargo.toml`;
    *   `config.rs`, `tools/sparql.rs` and `tools/ontology.rs`;
    *   `ci.yml` and `release.yml`;
    *   `zoom_out_report.md`;
    *   `context/bevy-0-20.html`.
*   **Reviewer status**: all three completed. There are no retries and no
    reviewers marked "not run".

| Reviewer | Verdict | Blockers | Majors | Nits |
|---|---|---|---|---|
| `egm-implementation-readiness` (ir) | FAIL | 4 | 12 | 12 |
| `egm-codebase-details` (cd) | FAIL | 2 | 11 | 9 |
| `egm-style` (st) | — | 1 | 8 | 13 |

**Round result: not clean.** Open Blockers and Majors remain.

--------------------------------------------------------------------------------

## Consolidated Findings

The `Raw` column gives the original reviewer finding ids. The traceability
matrix is at the end.

### 🔴 Blockers

| ID | Scope (lines) | Finding | Raw |
|---|---|---|---|
| R1 | 515–523, 581–594, 673–698, 736–745 | **Phases do not build in order.** <br>• Phase 2 files depend on items that arrive later: `plugin.rs` inserts `LayoutState`, `ViewerStyle`, `Selection`, `ClassFilter` and `ActivityFeed`, and registers the sub-plugins. `main.rs` uses `ViewerStyle`. `headless` uses `apply_to_graph`, which file 8 defines (Phase 3). <br>• Phase 3 renders nodes before `LayoutState` (Phase 4) exists. <br>• File 8 appears in Phase 3 and in Phase 6, with no split. "file 10 partial" is undefined. | ir-B1, cd-M4, st-M2 |
| R2 | 512–523, 784–805 | **The CI and release changes come too late.** <br>• Bevy joins the workspace in Phase 2. <br>• From then on, `ci.yml` (lines 28–32) runs clippy and tests on Linux without the Bevy system packages. <br>• `release.yml:74` builds the whole workspace, including the Linux gnu and musl targets. <br>• Files 17 and 18 are only scheduled in Phase 6, yet Phase 3's exit criterion already says "CI green with Linux deps". | ir-B4, cd-M5, st-B1 |
| R3 | 499–501, 518, 771–782 | **The ownership hand-over is incomplete.** <br>• Phase 1 applies companion files 1–13. Companion file 1 adds `crates/sparql-mcp-viewer` to `members` before the crate exists, so every cargo command fails. <br>• Companion files 21 (README) and 22 (CHANGELOG) duplicate this document's files 15 and 16 but are not handed over. | ir-B1, cd-M7, st-M1, ir-N1 |
| R4 | 494–508, 600, 606, 659 | **The wire types are undefined.** <br>• `Snapshot` (fields, triple shape, `kinds`, `graph`) is defined in neither document. <br>• `StoreEvent` is only "serde-serializable": it needs `Deserialize` plus `#[serde(tag = "event", rename_all = "lowercase")]`, and the same applies to `SubjectKind`. <br>• Nothing says how the decoder tells a snapshot line from an event line. This matters for a mid-stream snapshot after `project_switch`. | ir-B2, cd-B2, cd-M1 |
| R5 | 537–541, 747–757 | **The crate is binary-only.** <br>• `tests/headless_e2e.rs` cannot import `connection` or `graph`. <br>• A `src/lib.rs` and a `[lib]` target are missing. The core crate already uses this layout (`lib.rs:1-10`). | ir-B3, cd-B1 |

### 🟡 Majors

| ID | Scope (lines) | Finding | Raw |
|---|---|---|---|
| R6 | 494–508, 662, 754 | **No rule filters events by graph.** <br>• `load_ontology` writes to the default graph unless `graph_iri` is given (`ontology.rs:77,86`). <br>• The snapshot only covers `active_graph`, so nodes from other graphs would show up and then vanish at the next resync. <br>• The e2e test does not say which graph it writes to. | ir-B2, cd-M2 |
| R7 | 494–508, 229–231, 518 | **`project_switch` does not change server state.** <br>• It only returns an IRI (`project.rs:145-160`). <br>• `active_graph` is fixed at startup (`main.rs:33-34`), and the companion's `spawn_viewer_channel` also receives it only once. <br>• So the "snapshot after switch" behavior has nothing to rest on, and no required companion change covers it. The companion review had already raised this (critic design-2-2). | cd-M3 |
| R8 | 626–630, 651–671 | **The node storage model is undefined.** <br>• Contradiction: `Remap` maps old ids to new ids, but the test says "keeps IRI ids". <br>• There is no id policy for deletions (tombstones, reuse, compaction), so it is unclear how `LayoutState` (a `Vec`) stays aligned. <br>• No counter of mentions is defined, which orphan detection needs. <br>• A duplicate insert would duplicate edges and attributes. <br>• The rule for matching a deleted literal is undefined. <br>• Nothing says how `apply_to_graph` decides to keep positions (after a reconnect) or drop them (after a project switch). | ir-M4, ir-M5, cd-M10, st-M5 |
| R9 | 586–588, 685–696 | **The scheduling rule is wrong, and some systems have no place.** <br>• Bevy does not infer order from data access, and 0.20 randomizes the order of unconstrained systems. <br>• `animate_spawn_despawn`, `update_edge_meshes`, `apply_node_materials` and the `Ui` set have no schedule or set. <br>• `write_transforms` is not ordered against transform propagation, which causes a one-frame lag. | ir-M7, cd-M9, st-M4 |
| R10 | 598, 613–615, 686, 742 | **Bevy-free vs Bevy is unclear.** <br>• `apply_to_graph` and `drain_inbox` are called Bevy-free (line 742), yet they are Bevy systems (lines 613, 686). <br>• `Inbox` wraps a `std::sync::mpsc::Receiver`, which is `!Sync` and so cannot be a `Resource`. Use a `Mutex`, crossbeam, or `NonSend`. <br>• The per-frame `Vec` resource has no name. <br>• Nothing says where the `Resource` derive for `ConnState` lives. | st-M2, ir-M3, cd-N2 |
| R11 | 600–601 vs 179; 605–612 | **The connection is underspecified.** <br>• `ViewerMsg` is `State(ConnState)` here but `Disconnected` in the Technical Plan. <br>• `retry_in` has no type: the banner countdown needs an `Instant`. <br>• No behavior is defined for a malformed line, an unknown event, or a snapshot that never arrives. <br>• Nothing says whether a reconnect after `changed` goes through `Backoff`, which would add at least 1 s to CUJ 3, or whether the backoff resets after `Streaming`. | st-M3, cd-N2, ir-M8 |
| R12 | 677–680, 690–696, 721–722, 731–732 | **The "one shared material per style" rule conflicts with per-node effects.** <br>• Per-node fade, the 2 s pulse and the "last-batch" material (missing from the list at line 296) all clash with that rule. <br>• No mechanism is named: scale, a few stepped shared materials, or a globally animated emissive value. <br>• The pulse duration has two sources: `ViewerStyle` and a hard-coded 2 s. | ir-M6, st-M6 |
| R13 | 563–564, 812, 819 | **`--all-features` turns on `dev` (`bevy/dynamic_linking`) in CI and in the Verification Plan.** This contradicts "used only in local dev builds". | st-M8, cd-M6, ir-N3 |
| R14 | 523, 814–817 | **The performance criterion is underspecified.** <br>• The 5,000-node load script is not a listed file. <br>• It does not say `INSERT DATA`. A pattern-based update would produce `changed` events, so the measured scenario would be different. <br>• Profile and machine are not stated. <br>• Nothing says where `FrameTimeDiagnosticsPlugin` is added. | st-M7, cd-M11, ir-M11 |
| R15 | 542–562, 788–792 | **Versions and features are left to the implementer.** <br>• Placeholders: `=0.20.<latest patch>`, the MSRV, the Bevy feature names, and the `glam`, `rand`, `tokio`, `clap` and `arboard` versions. rand 0.8 and 0.9 differ. <br>• No Linux windowing backend (`x11` or `wayland`) is listed while `default-features = false`. <br>• The apt list is incomplete. <br>• The core crate's dependency specs could be reused. | ir-M2, cd-N3, cd-N5, st-N8 |
| R16 | 506–508 | **The lag behavior is not decided.** The text only says closing "works". If the companion chooses `lagged`, file 5 has no handling for it. | ir-M1 |
| R17 | 721–722 | **The class filter semantics are undefined.** <br>• Union or intersection of the selected classes? <br>• What happens to untyped instances and to class nodes? <br>• What is the initial state? <br>• Do the edges fade too? | ir-M9 |
| R18 | 638–645, 665–671, 747–757, 824–826 | **The tests do not meet the plan's own coverage rule.** <br>• Not covered: edge styles, classification by each of the five IRIs, the `kinds` map, `generation`, centroid placement, project switch, duplicates, graph filtering. <br>• `connection` has no tests (NDJSON decoding, backoff). <br>• The e2e test has no fixtures or expected values. <br>• `layout_budget_5000` has no generator, seed, step count or threshold. | ir-M10, st-N5 |
| R19 | 700–709, 545–554 | **Bevy 0.20 ships `PanOrbitCamera` (`bevy_camera_controller`).** File 9 writes its own orbit camera without recording why the built-in one was rejected. | cd-M8, ir-N4 |
| R20 | 517 | **Phase 0 has no failure branch.** If WSL2 forwarding fails, the fallback (binding to the WSL IP) contradicts the companion's "loopback only" stance. That conflict is not decided. | ir-M12 |
| R21 | 309–311, 723 | **Class labels and selected-node labels are assigned to no file.** File 10 only covers hover labels. | st-N10, ir-M11 |

### 🟢 Nits

| ID | Scope | Finding | Raw |
|---|---|---|---|
| R22 | 740–741 | `MinimalPlugins` already contains `ScheduleRunnerPlugin`, so adding a second one panics. Use `.set(ScheduleRunnerPlugin::run_loop(..))`. | ir-N2 |
| R23 | 743–745, 166, 333 | Headless mode has three different exit conditions. | st-N4 |
| R24 | 570, 585, 740 | `--bind` is the wrong name for a client flag. The timeout is named three different ways. | st-N3 |
| R25 | 571–573 | No default `DEFAULT_CONFIG_FILE` (`config.rs:30`). `Config::load` silently ignores a missing file. `viewer.enabled = false` is not handled. | cd-N6, ir-N10 |
| R26 | 653–654, 250–254 | Reuse is possible instead of a hand-written parser and duplicated IRI list: `domain::RdfTerm` (`sparql_store.rs:57-66`), the Oxigraph parser, and the companion's `classify_subjects`. | cd-N7 |
| R27 | 751–754 | The e2e test should name `OxigraphAdapter::open_in_memory()`, the tool function signatures and how the sink is wired. | cd-N8, ir-N12 |
| R28 | 532–535 | `opt-level = 3` on every dependency also slows CI and server dev builds. Only the benefit is stated. | st-N7, cd-N9 |
| R29 | 502–505 | Plain literals (`"lex"` with no tag or datatype) are not in the encoding list. | st-N2 |
| R30 | 518–520 | The phase table mixes "companion files" and "files" with two numberings. | st-N1 |
| R31 | 800–805, 801 | macOS targets are already commented out. The YAML for the Windows-only step is not given. | cd-N1, ir-N11 |
| R32 | 542–544 | Workspace field inheritance (`.workspace = true`) is not specified. | cd-N4 |
| R33 | 621–625, 677–684, 653–655, 729–732 | Visual and data details are left open: tonemapper variant, fog, `LayoutParams` defaults, `smc:` IRI, choice among several labels, local-name split, `triple_count` for `changed`/`snapshot`, `received_at` type. | ir-N5, ir-N7, ir-N8 |
| R34 | 611–612, 589 | Where `ShutdownHandle` is stored is not stated. If it is dropped, the connection stops. The `Selection` type is unspecified. | ir-N6 |
| R35 | 679–680, 694 | The edge material (unlit or translucent) is not given, and edges are not excluded from picking. | ir-N9 |
| R36 | 519 | Building in WSL for Phase 2 needs the Bevy Linux packages locally. This is not mentioned. | ir-N12 |
| R37 | 633, 512, 821–823 | Wording: "sleeps the layout below…". Who sets `awake = true`? "green" should say "pass". CUJ pass/fail criteria are not measurable for the manual checks. | st-N6, st-N12, st-N13 |
| R38 | 615, 753–756 | Vague references in the e2e steps: `load_ontology` called through MCP or directly? How is the reconnect observed? | st-N11 |

--------------------------------------------------------------------------------

## Notes for resolution

*   **Companion design.** R4, R6, R7 and R16 cannot be fixed inside this
    document alone. They require additions to "Required changes to the
    companion design", and some imply real server work. R7 means
    `project_switch` must become stateful, which the companion has not
    planned.
*   **Phase re-mapping.** R1, R2, R3 and R5 come from the same root cause:
    files were assigned to phases by feature, without checking compile
    dependencies. One coherent re-mapping resolves all four:
    *   CI and release move into Phase 2;
    *   `lib.rs` is added;
    *   `plugin.rs` registers each part when its phase lands;
    *   positions are placed in Phase 3.
*   **Accurate parts.** codebase-details confirmed the following against
    the code:
    *   the package name `sparql-mcp` and the path `../sparql-mcp-core`;
    *   `config::Config` is public;
    *   the CI steps match the plan's description;
    *   `release.yml:74` builds the whole workspace;
    *   the CHANGELOG and README structure;
    *   `serve`, `smc:label` and `MeshPickingPlugin` exist.

--------------------------------------------------------------------------------

## Traceability Matrix

| Raw finding | → Consolidated |
|---|---|
| ir-B1 | R1, R3 |
| ir-B2 | R4, R6 |
| ir-B3 | R5 |
| ir-B4 | R2 |
| ir-M1 | R16 |
| ir-M2 | R15 |
| ir-M3 | R10 |
| ir-M4, ir-M5 | R8 |
| ir-M6 | R12 |
| ir-M7 | R9 |
| ir-M8 | R11 |
| ir-M9 | R17 |
| ir-M10 | R18 |
| ir-M11 | R14, R21 |
| ir-M12 | R20 |
| ir-N1 | R3 |
| ir-N2 | R22 |
| ir-N3 | R13 |
| ir-N4 | R19 |
| ir-N5, ir-N7, ir-N8 | R33 |
| ir-N6 | R34 |
| ir-N9 | R35 |
| ir-N10 | R25 |
| ir-N11 | R31 |
| ir-N12 | R27, R36 |
| cd-B1 | R5 |
| cd-B2, cd-M1 | R4 |
| cd-M2 | R6 |
| cd-M3 | R7 |
| cd-M4 | R1 |
| cd-M5 | R2 |
| cd-M6 | R13 |
| cd-M7 | R3 |
| cd-M8 | R19 |
| cd-M9 | R9 |
| cd-M10 | R8 |
| cd-M11 | R14 |
| cd-N1 | R31 |
| cd-N2 | R10, R11 |
| cd-N3, cd-N5 | R15 |
| cd-N4 | R32 |
| cd-N6 | R25 |
| cd-N7 | R26 |
| cd-N8 | R27 |
| cd-N9 | R28 |
| st-B1 | R2 |
| st-M1 | R3 |
| st-M2 | R1, R10 |
| st-M3 | R11 |
| st-M4 | R9 |
| st-M5 | R8 |
| st-M6 | R12 |
| st-M7 | R14 |
| st-M8 | R13 |
| st-N1 | R30 |
| st-N2 | R29 |
| st-N3 | R24 |
| st-N4 | R23 |
| st-N5 | R18 |
| st-N6, st-N12, st-N13 | R37 |
| st-N7 | R28 |
| st-N8 | R15 |
| st-N10 | R21 |
| st-N11 | R38 |
