# Stage 9: Implementation Readiness Test (Round 2)

*   **Target**: `docs/designs/sparql-mcp-viewer.md`, full file (~1045 lines).
*   **Standard**: `implementation-readiness-standard.md`, applied by
    `egm-implementation-readiness` and `egm-codebase-details`.
    `egm-style` used its built-in criteria.
*   **Primary focus this round**: ground-truth sweep for stale WSL
    dependencies. The author confirmed (see
    `reviews/sparql-mcp-viewer/context/windows-native-environment.md`) that
    both `sparql-mcp serve` and `sparql-mcp-viewer` now run **natively on
    Windows** (MSVC). There is no WSL anywhere in the dev loop or deployment
    target; the loopback connection is a same-OS connection between two
    native Windows processes. Any remaining WSL-dependent instruction,
    fallback, or rationale in the target file is a documentation defect
    against this ground truth, not a valid constraint. All three reviewers
    were briefed on this and ran it alongside their normal review vectors.
*   **GoldfishContext**:
    *   `context/windows-native-environment.md` (ground truth, read first);
    *   `interface-agentic-rust-stylee.md`;
    *   root `Cargo.toml` and core `Cargo.toml`;
    *   `config.rs`, `tools/sparql.rs` and `tools/ontology.rs`;
    *   `ci.yml` and `release.yml`;
    *   `zoom_out_report.md`;
    *   `context/bevy-0-20.html`;
    *   Round 1 reference: `implementation_readiness_report.md`,
        `goldfish_review_report.md` (read-only history, not re-litigated).
*   **Reviewer status**: all three completed. There are no retries and no
    reviewers marked "not run".

| Reviewer | Verdict | Blockers | Majors | Nits |
|---|---|---|---|---|
| `egm-implementation-readiness` (ir) | FAIL | 3 | 7 | 3 |
| `egm-codebase-details` (cd) | FAIL | 3 | 10 | 2 |
| `egm-style` (st) | FAIL | 3 | 8 | 0 |

**Round result: not clean.** 4 Blockers, 8 Majors, and 3 Nits remain. All 4
Blockers and most Majors are the WSL-staleness sweep this round was run for;
every one of them was independently found by at least two of the three
reviewers.

--------------------------------------------------------------------------------

## Consolidated Findings

The `Raw` column gives the original reviewer finding ids
(`<reviewer>-2-<n>`). Where reviewers disagreed on severity for the same
finding, the consolidated severity is the maximum reported (same policy as
Round 1). The traceability matrix is at the end.

### 🔴 Blockers

| ID | Scope (lines) | Finding | Raw |
|---|---|---|---|
| R1 | 109–115 (Open Questions) | **The sole "Open Question" describes a cross-OS risk that no longer exists.** "WSL2 localhost forwarding... has not been verified... If it fails, WSL `networkingMode=mirrored` is tried next. If that also fails, the design returns to drafting." Both server and viewer are native Windows processes on the same machine now; the loopback connection is same-OS, so there is no forwarding step and no `networkingMode` fallback to decide. Remove the bullet (and the "Open Questions" heading, since it is the only entry, unless a genuinely open question for the native-Windows design replaces it). | st-2-1, ir-2-1, cd-2-1 |
| R2 | 572 (Phase 0 row) | **Phase 0's exit criterion cannot pass as written and gates every later phase on a non-issue.** "A line sent from Windows to `127.0.0.1:7878` reaches `nc -l 7878` in WSL. If it does not, retry with WSL `networkingMode=mirrored`. If that also fails, stop and return to drafting." There is no WSL side left to send a line to. Rewrite as a same-machine, same-OS check (e.g., a Bevy window opens on Windows; a line sent to `127.0.0.1:7878` from one Windows process is received by another). Drop the WSL fallback branch entirely. | st-2-2, ir-2-2, cd-2-2 |
| R3 | 153–161 (CUJ 4 Given/When) | **CUJ 4's Given and When both describe a WSL-hosted server/viewer that no longer exists.** "**Given**: `sparql-mcp serve` is running in WSL... **When**: The developer runs `sparql-mcp-viewer --headless` from WSL..." Both processes are native Windows now; as written the journey cannot be literally executed. Rewrite Given/When as both processes running natively on Windows on the same machine — the journey's substance (headless connect, print counts, exit code 2 on timeout) is unaffected. | st-2-3, ir-2-5, cd-2-3 |
| R4 | 1017–1021 (Verification Plan, Compilation/Build) | **The required local build step names an environment the implementer won't have.** "`cargo build --workspace` in WSL, after installing the packages from file 19." An implementer with no WSL installed (the expected post-migration state) cannot execute this as specified. Split into (a) the CI-only Linux build, which genuinely needs the Bevy apt packages on the hosted `ubuntu-latest` runner (already correctly scoped via file 19/`ci.yml`), and (b) the developer's own local build, which is a native-Windows `cargo build --workspace` with no apt packages and no WSL. | st-2-9, ir-2-3, cd-2-10 |

### 🟡 Majors

| ID | Scope (lines) | Finding | Raw |
|---|---|---|---|
| R5 | 580–581 | **A build instruction conflates the CI runner's real Linux-package need with a local WSL step that no longer exists.** "Before Phase 2, install the Bevy Linux packages from file 19 inside WSL. Without them, the viewer crate does not compile there." The packages are only needed on the hosted `ubuntu-latest` CI runner (file 19/`ci.yml`), not by any local dev step. Delete the sentence or rescope it explicitly to the CI runner. | st-2-7, ir-2-4, cd-2-7 |
| R6 | 54–56 (The Problem) | **The stated rationale for "testability without a window" names the wrong dev environment.** "The viewer is a Windows GUI, and the development environment is WSL. If graph logic lives inside Bevy systems, no automated test can run it." The dev-environment claim is false; the underlying motivation for the Bevy-free `graph`/`layout` split still holds without WSL. Drop "and the development environment is WSL," keep the no-display/no-automated-test rationale. | st-2-4, ir-2-8, cd-2-4 |
| R7 | 80–83 (Goals) | **A Goal still claims the headless smoke test runs from WSL.** "...so the pipeline can be smoke-tested from WSL." Reword to "...so the pipeline can be smoke-tested without a window" (dropping the OS reference), since it is now unremarkable — same machine, same OS. | st-2-5, ir-2-7, cd-2-5 |
| R8 | 362 (Headless mode) | **Same staleness repeated in the Headless mode section.** "This is the automated end-to-end check that runs from WSL." The check now runs as a native Windows process (or its own CI runner, not WSL). Reword to remove the WSL reference. | st-2-6, ir-2-6, cd-2-6 |
| R9 | 960–972 (File 16, README.md) | **The planned crate README documents two mechanisms that no longer exist.** Line 966: "the Bevy Linux packages needed to build in WSL;" Line 971: "the WSL2 `mirrored` networking note;" There is no WSL build path for this Windows-only crate and no WSL2 networking fallback to document (the connection is same-OS loopback). Remove both bullets from the planned README content list, or replace the first with a note that the Linux packages are needed only by the CI/Linux lint-and-test job, if worth keeping at all. | st-2-8, ir-2-9, cd-2-8, cd-2-9 |
| R10 | 1026 (Verification Plan, E2E) | **The E2E verification step repeats CUJ 4's stale WSL framing.** "CUJ 4 is run by hand from WSL against `sparql-mcp serve`." Depends on R3; reword to describe both processes running natively on Windows. | st-2-10, ir-2-10, cd-2-11 |
| R11 | 1027–1028 (Verification Plan, E2E) | **A trailing clause contrasts against an environment no longer in the loop.** "CUJs 1 to 3 are checked by hand on Windows at the end of their phase. They cannot be scripted from WSL." The first sentence is accurate; drop "from WSL" from the second — reword to "They require visual inspection and cannot be scripted," with no OS reference. | st-2-11, ir-2-11, cd-2-12 |
| R12 | ~859–864 (File 10, `camera.rs`) | **Non-existent Bevy API: the plugin bundle is misnamed.** "Adds `PanOrbitCameraPlugins` and configures `PanOrbitCamera`..." Per the supplied Bevy 0.20 release notes, the actual bundle exported alongside `MeshPickingPlugin` is `DefaultPanOrbitCameraPlugins`, not `PanOrbitCameraPlugins`. As written this identifier does not exist and would fail to compile. This is a new defect introduced by a Round 1 fix that correctly adopted Bevy's built-in camera (resolving prior R19/cd-M8) but misnamed the plugin-group symbol. Correct to `DefaultPanOrbitCameraPlugins`. | ir-2-12, cd-2-13 |

### 🟢 Nits

| ID | Scope | Finding | Raw |
|---|---|---|---|
| R13 | 705–708 (File 5, `plugin.rs`) | The `PostUpdate` ordering rule defers the concrete system-set identifier to implementation time ("...before Bevy's transform propagation set, whose exact 0.20 name is checked when implementing"), unlike similar version-dependent lookups elsewhere in the plan that give a precise, mechanical procedure. Name the 0.20 transform-propagation set directly (a stable, documented identifier) or give an equally mechanical lookup rule. | ir-2-13 |
| R14 | File 2, `Cargo.toml` | The viewer's `tokio` feature list (`["rt", "net", "io-util", "time", "macros", "sync"]`) is labeled "reused from the core crate" but actually differs from core's current + planned feature set (`rt` vs `rt-multi-thread`, added `time`, dropped `process`) in three ways. Each difference is individually justifiable, but the label is misleading. Relabel or clarify that this entry is viewer-specific. | cd-2-14 |
| R15 | File 7, `graph.rs` | **Re-flagged from Round 1 (cd-N7/R26), still present at the same scope.** The hand-written `enum Term { Iri, Blank, Literal }` parsed from an N-Triples term string duplicates the existing `domain::RdfTerm` enum (confirmed via `sparql.rs`'s `render_term`, matching `RdfTerm::Iri`/`BlankNode`/`Literal{value,datatype,lang}`), even though the viewer already depends on the core crate for other wire types (`StoreEvent`, `SubjectKind`, `CLASS_TYPE_IRIS`). Reuse `domain::RdfTerm` instead of a hand-written parser. | cd-2-15 |

--------------------------------------------------------------------------------

## Notes for resolution

*   **Root cause of the Blockers/Majors.** R1–R11 all trace to a single
    stale assumption: that `sparql-mcp serve` runs in WSL while
    `sparql-mcp-viewer` runs on Windows, connected across a cross-OS
    loopback-forwarding boundary. That assumption is gone per confirmed
    ground truth. R1–R4 are Blockers because they describe a test, build, or
    verification step that literally cannot be performed as written (and in
    R1/R2 that blocks the whole phase-gated plan on a non-issue); R5–R11 are
    prose/goal statements that are now factually wrong but don't block
    execution of a concrete step.
*   **Companion-document dependency, not edited here.** `ir` and `cd`
    independently traced R1, R2, R3, and R10 back to
    `interface-agentic-rust-stylee.md`'s own claim that the server and viewer
    are "different operating-system processes on different operating
    systems" connected via "WSL2 localhost forwarding" (that document's
    Technical Plan intro / Data-flow section, roughly its lines 128–174).
    That document is out of scope for this round and was not modified; a
    follow-up round on it is needed so this document's fixes don't keep
    citing a companion that still asserts the obsolete cross-OS topology.
*   **Confirmed clean of WSL in config, not just prose.** `cd` verified
    directly that `ci.yml` runs only on `ubuntu-latest` and `release.yml`'s
    Windows leg runs on `windows-latest`; neither file mentions WSL anywhere.
    The staleness is entirely in this document's own author/dev-workflow
    prose (Open Questions, Goals, The Problem, CUJ 4, Headless mode, the
    phase table, the planned README, the Verification Plan), not in CI
    config.
*   **Correctly left alone.** Line 369's mention of "WSLg" is a reference to
    a rejected *alternative rendering approach* in the companion design's
    Alternatives Considered list — a historical record of why an option was
    rejected, not a claim about the current dev/deployment environment. All
    three reviewers who looked at it agree it is not a finding.
*   **Severity note on R3/R4.** `ir` rated the CUJ 4 Given/When (R3) as
    Major rather than Blocker, and `st`/`cd` rated the Verification-Plan
    build step (R4) as Major rather than Blocker; both were escalated to
    Blocker here under the max-severity rule because, in each case, two of
    three reviewers independently called it a Blocker and the underlying
    defect is the same "cannot be executed as written" class as the
    unanimous R1/R2 Blockers.

--------------------------------------------------------------------------------

## Traceability Matrix

| Raw finding | → Consolidated |
|---|---|
| st-2-1 | R1 |
| st-2-2 | R2 |
| st-2-3 | R3 |
| st-2-4 | R6 |
| st-2-5 | R7 |
| st-2-6 | R8 |
| st-2-7 | R5 |
| st-2-8 | R9 |
| st-2-9 | R4 |
| st-2-10 | R10 |
| st-2-11 | R11 |
| ir-2-1 | R1 |
| ir-2-2 | R2 |
| ir-2-3 | R4 |
| ir-2-4 | R5 |
| ir-2-5 | R3 |
| ir-2-6 | R8 |
| ir-2-7 | R7 |
| ir-2-8 | R6 |
| ir-2-9 | R9 |
| ir-2-10 | R10 |
| ir-2-11 | R11 |
| ir-2-12 | R12 |
| ir-2-13 | R13 |
| cd-2-1 | R1 |
| cd-2-2 | R2 |
| cd-2-3 | R3 |
| cd-2-4 | R6 |
| cd-2-5 | R7 |
| cd-2-6 | R8 |
| cd-2-7 | R5 |
| cd-2-8 | R9 |
| cd-2-9 | R9 |
| cd-2-10 | R4 |
| cd-2-11 | R10 |
| cd-2-12 | R11 |
| cd-2-13 | R12 |
| cd-2-14 | R14 |
| cd-2-15 | R15 |
