# Goldfish Annotated Review Report

Status: resolved. Outcome: auto accept; 15 fixes applied; 0 contested; 0 escalated; 0 declined.

**Target:** `docs/designs/sparql-mcp-viewer.md` (full file)

**Source:** `implementation_readiness_report_round2.md`, Round 2. IDs R1–R15
refer to the consolidated findings in that report, where the full
discrepancy text and the raw reviewer IDs are recorded. Round 1's report
(`implementation_readiness_report.md`) and its resolution are read-only
history and are not re-litigated here.

**Tally:** 15 PROPOSE FIX · 0 CONTEST · 0 ESCALATE · 0 DECLINE. All 15
findings were checked against the supplied `GoldfishContext` (ground-truth
note, companion design, core crate source, CI/release YAML, Bevy 0.20
release notes) before being marked PROPOSE FIX; none required overruling a
reviewer.

## Prioritized Findings & AI Suggestions

### R1 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 109-115](../../sparql-mcp-viewer.md#L109-L115) (Open Questions).
*   **Discrepancy Description:** The sole "Open Question" describes a WSL2
    localhost-forwarding risk that no longer exists now that both processes
    run natively on Windows.
*   **Rationale:** The confirmed ground truth (`context/windows-native-environment.md`)
    states there is no WSL anywhere; the loopback connection is same-OS.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Checkable directly against the author's
    confirmed ground truth note; no other open question remains to justify
    keeping the section.
*   ***Proposed Edit:***
    *   Location: Lines 109-115 (the entire "Open Questions" section).
    *   Change: Delete the section:
        ```
        ## Open Questions

        *   **WSL2 localhost forwarding.** It has not been verified on the target
            machine. Phase 0 checks it first. If it fails, WSL
            `networkingMode=mirrored` is tried next. If that fails too, the design
            returns to drafting. Binding to a non-loopback address stays out of
            scope for v1.

        ```
        leaving the document to flow directly from Non-Goals into CUJs.

### R2 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Line 572](../../sparql-mcp-viewer.md#L572) (Phase 0 exit criterion).
*   **Discrepancy Description:** Phase 0's exit criterion sends a line from
    Windows to a WSL-hosted `nc -l 7878`, with a WSL `networkingMode=mirrored`
    fallback. This gates every later phase on a cross-OS step that cannot
    occur.
*   **Rationale:** Same ground truth as R1; this is the blocking instance
    because no later phase can start until this criterion is met.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 572, Phase 0 row.
    *   Change:
        *   Old: `An empty Bevy 0.20 window opens on Windows, built from this repository. A line sent from Windows to `127.0.0.1:7878` reaches `nc -l 7878` in WSL. If it does not, retry with WSL `networkingMode=mirrored`. If that also fails, stop and return to drafting.`
        *   New: `An empty Bevy 0.20 window opens on Windows, built from this repository. A line sent from one native Windows process to `127.0.0.1:7878` is received by another native Windows process listening on that port. If it does not, stop and return to drafting.`

### R3 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 153-161](../../sparql-mcp-viewer.md#L153-L161) (CUJ 4 Given/When).
*   **Discrepancy Description:** CUJ 4's Given and When both describe a
    WSL-hosted server and a viewer launched "from WSL," which cannot be
    literally executed anymore.
*   **Rationale:** Same ground truth; the journey's substance (headless
    connect, print counts, exit code 2 on timeout) is unaffected by the
    environment correction.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 155-158.
    *   Change:
        *   Old:
            ```
            *   **Given**: `sparql-mcp serve` is running in WSL with the side-channel
                enabled.
            *   **When**: The developer runs `sparql-mcp-viewer --headless` from WSL, and
                an agent writes triples.
            ```
        *   New:
            ```
            *   **Given**: `sparql-mcp serve` is running natively on Windows with the
                side-channel enabled.
            *   **When**: The developer runs `sparql-mcp-viewer --headless` on the same
                machine, and an agent writes triples.
            ```

### R4 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 1017-1021](../../sparql-mcp-viewer.md#L1017-L1021) (Verification Plan, Compilation/Build).
*   **Discrepancy Description:** The required local build step is
    `cargo build --workspace` "in WSL," which an implementer with no WSL
    installed cannot execute.
*   **Rationale:** Conflates the CI runner's genuine need for the Bevy Linux
    apt packages with a local step that no longer applies.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid; `cd` independently confirmed `ci.yml` runs
    only on `ubuntu-latest`, so the Linux package need is already correctly
    scoped to CI and just needs to stop being described as a local WSL step.
*   ***Proposed Edit:***
    *   Location: Lines 1017-1021.
    *   Change:
        *   Old:
            ```
            *   **Compilation/Build**:
                *   `cargo build --workspace` in WSL, after installing the packages from
                    file 19.
                *   On Windows: `cargo build --release -p sparql-mcp-viewer --target
                    x86_64-pc-windows-msvc`.
            ```
        *   New:
            ```
            *   **Compilation/Build**:
                *   Local dev: `cargo build --workspace` on Windows. No WSL and no Bevy
                    Linux apt packages are needed for this step.
                *   CI only: the hosted `ubuntu-latest` runner installs the Bevy Linux
                    packages from file 19 before `cargo build --workspace` (needed
                    because that runner is Linux, not because of any local WSL step).
                *   Release: `cargo build --release -p sparql-mcp-viewer --target
                    x86_64-pc-windows-msvc`.
            ```

### R5 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 580-581](../../sparql-mcp-viewer.md#L580-L581).
*   **Discrepancy Description:** "Install the Bevy Linux packages... inside
    WSL" conflates the CI runner's need with a non-existent local step.
*   **Rationale:** Same root cause as R4, different location.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 580-581.
    *   Change:
        *   Old: `Before Phase 2, install the Bevy Linux packages from file 19 inside WSL.\nWithout them, the viewer crate does not compile there.`
        *   New: `Before Phase 2, the CI workflow (file 19) installs the Bevy Linux\npackages on the `ubuntu-latest` runner. Without them, the viewer crate does\nnot compile there. No local installation step is needed on the Windows\ndevelopment machine.`

### R6 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 54-56](../../sparql-mcp-viewer.md#L54-L56) (The Problem).
*   **Discrepancy Description:** States "the development environment is
    WSL," which is false; the underlying testability motivation still holds
    without it.
*   **Rationale:** Ground truth correction; no substantive change to the
    rationale needed.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 54-56.
    *   Change:
        *   Old: `**Testing without a window.** The viewer is a Windows GUI, and the\ndevelopment environment is WSL. If graph logic lives inside Bevy systems,\nno automated test can run it.`
        *   New: `**Testing without a window.** The viewer is a Windows GUI. If graph\nlogic lives inside Bevy systems, no automated test can run it.`

### R7 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 80-83](../../sparql-mcp-viewer.md#L80-L83) (Goals).
*   **Discrepancy Description:** A Goal claims the headless pipeline "can be
    smoke-tested from WSL."
*   **Rationale:** Same machine, same OS now; the OS reference is stale and
    unremarkable.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 83.
    *   Change:
        *   Old: `...so the pipeline can be smoke-tested from WSL.`
        *   New: `...so the pipeline can be smoke-tested without a window.`

### R8 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Line 362](../../sparql-mcp-viewer.md#L362) (Headless mode).
*   **Discrepancy Description:** "This is the automated end-to-end check
    that runs from WSL."
*   **Rationale:** The Verification Plan (line 1025) already states this
    check runs in CI; the WSL reference is stale and inconsistent with that.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 362.
    *   Change:
        *   Old: `This is the automated end-to-end check that runs from WSL.`
        *   New: `This is the automated end-to-end check that runs in CI.`

### R9 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 960-972](../../sparql-mcp-viewer.md#L960-L972) (File 16, README.md).
*   **Discrepancy Description:** The planned crate README documents "the
    Bevy Linux packages needed to build in WSL" and "the WSL2 `mirrored`
    networking note," neither of which exist anymore.
*   **Rationale:** Ground truth correction, applied to planned deliverable
    content rather than to the design document's own prose.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Replacing rather than bluntly deleting the
    first bullet keeps the CI-scoped fact available to a crate reader, per
    the reviewer's stated alternative.
*   ***Proposed Edit:***
    *   Location: Lines 964-971 (the bullet list).
    *   Change:
        *   Old:
            ```
            *   what the viewer shows;
            *   how to enable `[viewer]`;
            *   the Bevy Linux packages needed to build in WSL;
            *   how to build on Windows with a Windows `rustup` toolchain;
            *   the run flags and `--headless`;
            *   the controls;
            *   the loopback-only, unauthenticated limitation;
            *   the WSL2 `mirrored` networking note;
            *   the GPU model used for the 60 fps measurement.
            ```
        *   New:
            ```
            *   what the viewer shows;
            *   how to enable `[viewer]`;
            *   that the Bevy Linux packages (file 19) are needed only by the CI
                Linux job, not for a local Windows build;
            *   how to build on Windows with a Windows `rustup` toolchain;
            *   the run flags and `--headless`;
            *   the controls;
            *   the loopback-only, unauthenticated limitation;
            *   the GPU model used for the 60 fps measurement.
            ```

### R10 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Line 1026](../../sparql-mcp-viewer.md#L1026) (Verification Plan, E2E).
*   **Discrepancy Description:** "CUJ 4 is run by hand from WSL against
    `sparql-mcp serve`."
*   **Rationale:** Depends on R3's correction.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 1026.
    *   Change:
        *   Old: `CUJ 4 is run by hand from WSL against `sparql-mcp serve`.`
        *   New: `CUJ 4 is run by hand on Windows against a locally running\n`sparql-mcp serve`.`

### R11 [`egm-style`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 1027-1028](../../sparql-mcp-viewer.md#L1027-L1028) (Verification Plan, E2E).
*   **Discrepancy Description:** "They cannot be scripted from WSL" implies
    an environment no longer in the loop.
*   **Rationale:** The underlying fact (manual visual inspection required)
    is unaffected; only the OS reference is stale.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 1028.
    *   Change:
        *   Old: `They cannot be scripted from WSL.`
        *   New: `They require visual inspection and cannot be scripted.`

### R12 [`egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 859-864](../../sparql-mcp-viewer.md#L859-L864) (File 10, `camera.rs`).
*   **Discrepancy Description:** "Adds `PanOrbitCameraPlugins`..." — this
    identifier does not exist in Bevy 0.20.
*   **Rationale:** Confirmed against the supplied Bevy 0.20 release notes:
    the exported plugin-group symbol is `DefaultPanOrbitCameraPlugins`. This
    is a new defect introduced by a Round 1 fix that correctly adopted
    Bevy's built-in camera but misnamed the symbol.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid; checked directly against
    `context/bevy-0-20.html`, which documents `DefaultPanOrbitCameraPlugins`
    (`docs.rs/bevy_camera_controller/0.20.0`) as the bundle added alongside
    `MeshPickingPlugin`.
*   ***Proposed Edit:***
    *   Location: Line 863.
    *   Change:
        *   Old: `Adds `PanOrbitCameraPlugins` and configures `PanOrbitCamera` on the`
        *   New: `Adds `DefaultPanOrbitCameraPlugins` and configures `PanOrbitCamera``

### R13 [`egm-implementation-readiness`] - Severity: Nit
*   **Affected Scope:** [Lines 705-708](../../sparql-mcp-viewer.md#L705-L708) (File 5, `plugin.rs`).
*   **Discrepancy Description:** Defers the concrete transform-propagation
    system-set identifier to implementation time, unlike similar
    version-dependent lookups elsewhere in the plan that give a precise,
    mechanical procedure.
*   **Rationale:** `bevy_transform`'s exact set name is not in the supplied
    `GoldfishContext` (the 0.20 release notes don't cover a pre-existing,
    stable API), so this fix gives a mechanical lookup procedure rather than
    asserting an unverified identifier.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid as a Nit; worth fixing since the plan already
    sets a precedent of giving mechanical lookup rules instead of
    placeholders (e.g. the `rust-version` rule at line 605).
*   ***Proposed Edit:***
    *   Location: Lines 705-708.
    *   Change:
        *   Old:
            ```
            *   **`PostUpdate`**: `write_transforms`, then
                `animate_spawn_despawn`, then `update_edge_meshes`. These are
                chained and ordered before Bevy's transform propagation set,
                whose exact 0.20 name is checked when implementing.
            ```
        *   New:
            ```
            *   **`PostUpdate`**: `write_transforms`, then
                `animate_spawn_despawn`, then `update_edge_meshes`. These are
                chained and ordered with `.before(...)` the system set that
                `bevy_transform` registers for `GlobalTransform` propagation;
                look it up in the pinned Bevy version's `bevy_transform::systems`
                docs (`cargo doc -p bevy_transform --open`) rather than
                hardcoding a name that may not match the pinned patch version.
            ```

### R14 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Line 608](../../sparql-mcp-viewer.md#L608) (File 2, `Cargo.toml`).
*   **Discrepancy Description:** The viewer's `tokio` feature list is
    labeled "reused from the core crate" but actually differs from core's
    feature set in three ways (`rt` vs `rt-multi-thread`, added `time`,
    dropped `process`).
*   **Rationale:** Confirmed against `crates/sparql-mcp-core/Cargo.toml:20`
    (`features = ["macros", "rt-multi-thread", "process", "io-util"]`) vs.
    the viewer's planned `["rt", "net", "io-util", "time", "macros", "sync"]`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid; each difference is individually justifiable
    but the "reused" label is misleading.
*   ***Proposed Edit:***
    *   Location: Line 608 (split into two bullets).
    *   Change:
        *   Old:
            ```
            *   **Dependency specs reused from the core crate.** `sparql-mcp = { path = "../sparql-mcp-core" }`,
                `tokio = { version = "1", features = ["rt", "net", "io-util", "time", "macros", "sync"] }`,
                `clap = { version = "4", features = ["derive"] }`, `serde = "1"`,
                `serde_json = "1"`, `anyhow = "1"`, `tracing = "0.1"`, and
                `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`.
            ```
        *   New:
            ```
            *   **Dependency specs reused from the core crate.** `sparql-mcp = { path = "../sparql-mcp-core" }`,
                `clap = { version = "4", features = ["derive"] }`, `serde = "1"`,
                `serde_json = "1"`, `anyhow = "1"`, `tracing = "0.1"`, and
                `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`.
            *   **Viewer-specific `tokio` features**, not a straight reuse of the
                core crate's list: `tokio = { version = "1", features = ["rt", "net", "io-util", "time", "macros", "sync"] }`
                (single-threaded `rt` instead of core's `rt-multi-thread`; adds
                `time` for the reconnect backoff and `sync` for the inbox
                channel; drops `process`, which the viewer does not need).
            ```

### R15 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 745-746](../../sparql-mcp-viewer.md#L745-L746) (File 7, `graph.rs`).
*   **Discrepancy Description:** Re-flagged from Round 1 (`cd-N7`/R26, which
    was accepted and marked PROPOSE FIX but the edit was not actually
    applied to the text — still present at the same scope). A hand-written
    `Term` enum/parser duplicates the existing `domain::RdfTerm`.
*   **Rationale:** Confirmed against `crates/sparql-mcp-core/src/domain/sparql_store.rs:58`
    (`pub enum RdfTerm { Iri, BlankNode, Literal { value, datatype, lang } }`,
    re-exported as `domain::RdfTerm`) and `sparql.rs:139-141`'s
    `render_term`, which matches those variants. The viewer already depends
    on the core crate for `StoreEvent`, `SubjectKind` and `CLASS_TYPE_IRIS`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid; this time applying the fix directly rather
    than leaving it as a note, since Round 1's acceptance of the equivalent
    finding did not make it into the text.
*   ***Proposed Edit:***
    *   Location: Lines 745-746.
    *   Change:
        *   Old:
            ```
            *   `enum Term { Iri, Blank, Literal }`, parsed from an N-Triples
                term string.
            ```
        *   New:
            ```
            *   Reuse `sparql_mcp::domain::RdfTerm` (already `Iri`, `BlankNode`,
                and `Literal { value, datatype, lang }`) instead of a
                hand-written `Term` enum and parser; the viewer already depends
                on the core crate for `StoreEvent`, `SubjectKind` and
                `CLASS_TYPE_IRIS`.
            ```

--------------------------------------------------------------------------------

## Notes

*   **Companion-document dependency.** R1-R4 and R10 trace back to
    `interface-agentic-rust-stylee.md`'s own claim that the server and
    viewer are different-OS processes connected over WSL2 forwarding. That
    document is out of scope for this round (and was in fact deleted from
    the working tree during this session, per the author — "on s'en fout").
    If it is restored or a replacement companion document is written, it
    will need its own round to clear the same stale assumption.
*   **Roadmap note, not a finding.** The author's longer-term intent — one
    common native binary per OS, Windows now, Linux later, never a cross-OS
    split — does not change any of the fixes above; it is recorded in
    `context/windows-native-environment.md` for a future round that may
    soften the "Platforms other than Windows: Not targeted in v1" Non-Goal.
