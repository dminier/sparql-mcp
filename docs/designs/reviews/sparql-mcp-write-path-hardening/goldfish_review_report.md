# Goldfish Annotated Review Report

Status: partially resolved (2026-11-18). The 20 findings are applied to the target. The claim-by-claim citation check and the carry-over audit (not run in this round) are done by each implementing phase, as recorded in the target's Verification Plan.

**Target:** `docs/designs/sparql-mcp-write-path-hardening.md` (lines 1-524)

**Source:** `consolidated_findings_report.md`, round 1. The IDs (B1-B3, M1-M15, N1-N2) are the consolidated finding IDs.

**Tally:** 20 findings (3 Blocker, 15 Major, 2 Nit): 20 PROPOSE FIX, 0 CONTEST, 0 ESCALATE, 0 DECLINE.

**Verdicts:**
*   Phase 0a: **READY AFTER FIXES**, once B1-B3, M1, M2, M3, M5 and M6 are applied.
*   Whole design: **NOT READY**.

**How the findings were checked:** Every finding was checked against the code cited, the sibling design and the ground truth. The synthesizer re-read `domain/sparql_store.rs` and confirmed three points:
*   the trait is synchronous;
*   it has 10 methods;
*   it has no `write_batch`.

## Prioritized Findings & AI Suggestions

### B1 [`egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 161-172](../../sparql-mcp-write-path-hardening.md#L161-L172), [lines 192-199](../../sparql-mcp-write-path-hardening.md#L192-L199), [lines 208-209](../../sparql-mcp-write-path-hardening.md#L208-L209).
*   **Discrepancy Description:** `GatedStore` is a decorator for a synchronous trait, but the only gate API is the async `WriteGate::run`, which runs its own `spawn_blocking`. Inside a runtime, `blocking_lock` has no timeout, and `block_on` panics. The CLI (`main.rs:236, 317, 344, 348`) and plugin dispatch (`server.rs:196-199`) call the store on async threads.
*   **Rationale:** Phase 0a cannot be coded without first deciding this.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The trait was confirmed sync at `sparql_store.rs:157-196`.
*   ***Proposed Edit:***
    *   Location: Write gate, replace the "Shape" bullet (lines 192-196) with:
        ```
        *   **Shape.** `WriteGate` is `Clone` and wraps
            `Arc<tokio::sync::Mutex<GateHolder>>`, with
            `GateHolder { label: &'static str, since: Instant }`. It has two entry points:
            *   `fn run_blocking<T>(&self, label: &'static str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T>`
                takes the lock with
                `Handle::current().block_on(tokio::time::timeout(wait, mutex.clone().lock_owned()))`
                and runs `f` on the current thread. `GatedStore` and
                `PrivilegedStore` write methods use it.
            *   `async fn run<T: Send + 'static>(&self, label, f: impl FnOnce() -> anyhow::Result<T> + Send + 'static)`
                takes the lock, then moves the owned guard into `spawn_blocking`.
                It is for callers that hold no store handle.
        *   **Calling rule.** Gated and privileged store methods must be called
            from a blocking thread. `dispatch_core`, plugin dispatch and every
            CLI write wrap their call in `tokio::task::spawn_blocking`. The gate is
            not reentrant: `WriteGate::run` is never wrapped around a
            `GatedStore`/`PrivilegedStore` call.
        ```
    *   Location: "Cancellation" bullet. Append: "With `run_blocking`, the guard lives on the blocking thread that runs the store work, so dropping the request future does not release it."

### B2 [`egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 164-172](../../sparql-mcp-write-path-hardening.md#L164-L172), [line 180](../../sparql-mcp-write-path-hardening.md#L180), [lines 482-487](../../sparql-mcp-write-path-hardening.md#L482-L487).
*   **Discrepancy Description:** The privileged handle has no type, no constructor and no wiring. The server passes one `store` to every arm, including `project_create`, which writes `urn:meta` (`project.rs:86-92`), and the recording tools (`server.rs:134-140`).
*   **Rationale:** Without this, an implementer either breaks `project_create` once 0c guards land, or hands plugins a privileged handle.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: After line 172, add:
        ```
        *   **Wiring.** `pub struct GatedStore { inner: Arc<dyn SparqlStore>, gate: WriteGate, guards: GuardConfig }`
            and `pub(crate) struct PrivilegedStore { inner: Arc<dyn SparqlStore>, gate: WriteGate }`
            share one `WriteGate`. `main.rs` builds both from one `OxigraphAdapter`.
            `SparqlMcpServer` gains `privileged: Arc<dyn SparqlStore>`. The
            `project_create`, `import_recording_*` and `materialize_recording` arms,
            and `mcp/importer.rs`, receive the privileged handle. Every other arm
            and `plugin_ctx()` receive the gated handle. `PluginContext.store` keeps
            its type and never holds the privileged handle.
        ```

### B3 [`egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 147-148](../../sparql-mcp-write-path-hardening.md#L147-L148), [lines 332-338](../../sparql-mcp-write-path-hardening.md#L332-L338), [lines 441-447](../../sparql-mcp-write-path-hardening.md#L441-L447).
*   **Discrepancy Description:** Tools map store errors to `McpError` (`sparql.rs:100-102`, `cbm.rs:144`, `project.rs:92`). A `write_busy` raised in `GatedStore` would therefore surface as a JSON-RPC error string. `ToolError` is in file 3, which belongs to 0c, but CUJ 5 needs it in 0a.
*   **Rationale:** "Errors are data" is not implementable for errors raised below the tool.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: After line 335 ("Errors are data"), add:
        ```
        *   **Error channel.** `limits.rs` defines `pub enum ToolError { LimitExceeded{..},
            ParseError{..}, GraphProtected{graph}, ReservedIri{iri},
            DestructiveUpdateForbidden, LoadForbidden, PathOutsideRoots{path},
            PathRejected{path}, WriteBusy{holder: String, waited_secs: u64} }`.
            Store-level refusals are returned as `anyhow::Error::new(ToolError::…)`.
            Each tool's error mapping first tries `e.downcast_ref::<ToolError>()`
            and, on a match, returns `Ok(CallToolResult { is_error: Some(true), content: [json] })`.
            Missing or ill-typed arguments stay `McpError::invalid_params`.
            `holder` is the `GateHolder.label` (the tool or method name).
        ```
    *   Location: Phase table, 0a Files. Add "3 (`ToolError` only)".

### M1 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 167-169](../../sparql-mcp-write-path-hardening.md#L167-L169).
*   **Discrepancy Description:** `write_batch` is not a trait method. `load_ontology_dir` and `export_graph` are not classified.
*   **Rationale:** `sparql_store.rs:159-196`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Confirmed by the synthesizer.
*   ***Proposed Edit:*** Replace "(`update*`, `load_*`, `write_batch`, …)" with:
    ```
    | Port method | Class in `GatedStore` |
    |---|---|
    | `update`, `update_with_outcome`, `load_rdf`, `load_rdf_with_triples`, `load_rdf_file`, `load_ontology_dir` | gated write |
    | `query`, `export_graph` (store read; file check in the tool), `list_graphs`, `triple_count`, `graph_size` (0b) | pass-through |

    Methods added later (the batch-ingestion `write_batch`, `commit_job`, `stream_file`) are added to this table by the change that adds them.
    ```

### M2 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Line 421](../../sparql-mcp-write-path-hardening.md#L421), [lines 477-480](../../sparql-mcp-write-path-hardening.md#L477-L480).
*   **Discrepancy Description:** In 0a, the classifier needed for the protected-graph check does not exist yet.
*   **Rationale:** `guards.rs` is in 0c.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** In the 0a Scope cell, append "(`GatedStore` provides the gate only; the guards are added in 0c)". In the 0c Files cell, add 5. Under file 5, add: "[0c] `update*` parses with spargebra and calls `classify_update`; the load methods check `opts.graph_iri`, or the default graph when it is `None`."

### M3 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 187-191](../../sparql-mcp-write-path-hardening.md#L187-L191), [lines 227-228](../../sparql-mcp-write-path-hardening.md#L227-L228).
*   **Discrepancy Description:** Some tools make several store calls that must not be split. `import_kind` runs `CLEAR` and then two loads (`recording.rs:199-228`), and `with_delta` counts, writes and counts again (`sparql.rs:220-226`). Gating each call separately lets other writers interleave.
*   **Rationale:** This contradicts the gate's stated purpose of exact counts.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** After line 199, add: "**Multi-call sections.** `PrivilegedStore::exclusive<T>(&self, label, f: impl FnOnce(&dyn SparqlStore) -> anyhow::Result<T>)` holds the gate once and passes the ungated inner store. Recording imports and `with_delta` (until 0b removes its counts) use it."

### M4 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 164, 170](../../sparql-mcp-write-path-hardening.md#L164-L170), [lines 406-413](../../sparql-mcp-write-path-hardening.md#L406-L413).
*   **Discrepancy Description:** The sibling design wraps port calls in `WriteGate::run` and also uses `PrivilegedStore`, which takes the gate. The tokio `Mutex` is not reentrant, so this deadlocks.
*   **Rationale:** `sparql-mcp-batch-ingestion.md:179, 296, 351`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. It is a cross-design consistency defect.
*   ***Proposed Edit:*** Under "Required changes to the companion designs", add: "**`sparql-mcp-batch-ingestion.md`.** Job writes call `PrivilegedStore` methods or `PrivilegedStore::exclusive`, never `WriteGate::run` around a store call (the gate is not reentrant). Skolem IRIs are produced after the `reserved_iri` check (see Guards)."

### M5 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Line 171](../../sparql-mcp-write-path-hardening.md#L171), [lines 482-487](../../sparql-mcp-write-path-hardening.md#L482-L487).
*   **Discrepancy Description:** The constructor, the test builder, the CLI and the viewer-channel wiring are not specified.
*   **Rationale:** `server.rs:46-53, 334-345`; `main.rs:358-365, 372`; `examples/hello-plugin/src/lib.rs:9`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** File 9, add:
    *   "`SparqlMcpServer::new(gated, privileged, doc_store, ontology_dir, active_graph, gdrive_config, store_path)`. The test builder builds both handles from one in-memory adapter.
    *   `spawn_viewer_channel` keeps the raw adapter, because it only reads.
    *   The CLI builds `WriteGate::new(Duration::from_secs(cfg.ingest.write_wait_timeout_secs))`. `load-file`, `code-import` and `import` use `GatedStore`; `load_ontology_dir` and `reload-ontology` use `PrivilegedStore`. Each call is made inside `spawn_blocking`."

    Add `examples/hello-plugin/src/lib.rs` (docs) to file 12.

### M6 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 206-211](../../sparql-mcp-write-path-hardening.md#L206-L211).
*   **Discrepancy Description:** The design does not handle three points:
    *   the `'static` requirement of `spawn_blocking` against borrowed arguments;
    *   `JoinError`;
    *   plugin dispatch.

    The prior M22 semaphore is not mentioned.
*   **Rationale:** `server.rs:105-140, 194-200`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. This is also a carry-over gap from M22.
*   ***Proposed Edit:*** Replace lines 208-211 with:
    ```
    `pub(crate) async fn dispatch_core(&self, name: &str, args: &JsonObject) -> Option<Result<CallToolResult, McpError>>`.
    Each store-touching arm clones the `Arc`s and `args`, then awaits
    `spawn_blocking(move || tool(&store, &args, &active_graph, sink.as_deref()))`.
    A `JoinError` maps to `McpError::internal_error`. `project_switch` and
    `get_gdrive_config` stay inline. Plugin dispatch runs in one `spawn_blocking`,
    with `plugins: Arc<Vec<…>>` and the `PluginContext` moved in. The blocking pool
    is not bounded by a semaphore: one stdio client sends few concurrent calls,
    and writes are serialized by the gate.
    The test `every_declared_tool_has_a_dispatch_arm` becomes a `#[tokio::test]`.
    ```

### M7 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 202-204](../../sparql-mcp-write-path-hardening.md#L202-L204), [line 421](../../sparql-mcp-write-path-hardening.md#L421).
*   **Discrepancy Description:** The `ping` criterion passes even without `spawn_blocking`. The design names no way to make a store call slow, and calling `call_tool` needs an rmcp `Peer`.
*   **Rationale:** An exit criterion that cannot fail does not gate the phase.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Replace the 0a exit cell with:
    > "In `tests/write_path_hardening.rs`, on `#[tokio::test(flavor = "current_thread")]`, with a `SlowStore` double wrapping `OxigraphAdapter::open(tempdir)` whose `update` sleeps 2 s:
    > (a) `dispatch_core("query_sparql")` completes within 200 ms while `dispatch_core("update_sparql")` is running;
    > (b) two concurrent writes never overlap (an `AtomicUsize` of concurrent entries peaks at 1);
    > (c) the first write's future is dropped by `timeout(50 ms)`; a second write with `write_wait_timeout_secs = 1` gets `write_busy` with holder `update_sparql`, and succeeds after the first ends."

### M8 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 419-424](../../sparql-mcp-write-path-hardening.md#L419-L424), [lines 435-511](../../sparql-mcp-write-path-hardening.md#L435-L511).
*   **Discrepancy Description:** The per-phase file lists do not match the file contents. Test updates come only in 0d, so 0b leaves the test suite red. `examples/ingest_bench.rs` is not listed.
*   **Rationale:** Each phase must build and test green.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Tag each bullet of files 2, 3, 5, 7, 9 and 10 with [0a], [0b] or [0c]. Set the Files column as follows:
    *   0a = 1, 2 (`write_wait_timeout_secs` only), 3 (`ToolError` only), 4, 5, 9, 10;
    *   0b = 6, 7, 10, 11, 12 (`tools_sparql.rs`, `store.rs`, the bench);
    *   0c = 2, 3, 5, 7, 8, 9, 10, 12;
    *   0d = 12 (docs).

    Add `examples/ingest_bench.rs` (Create, 0b) to file 12.

### M9 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Line 422](../../sparql-mcp-write-path-hardening.md#L422).
*   **Discrepancy Description:** A test double at the port cannot observe the `len()` calls inside the adapter.
*   **Rationale:** `oxigraph.rs:86, 91, 128, 145, 245, 263`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Replace "a counting `SparqlStore` test double records zero `len()` calls" with "a `#[cfg(test)]` counter behind a private `OxigraphAdapter::store_len()`, which replaces every `self.store.len()`, stays at 0 across `load_rdf`, `INSERT DATA` and `DELETE DATA`".

### M10 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 343-346](../../sparql-mcp-write-path-hardening.md#L343-L346), [lines 460-464](../../sparql-mcp-write-path-hardening.md#L460-L464).
*   **Discrepancy Description:** The new `UpdateOutcome`, accessor and `FileLoadResult` shapes are not given. `Option<&Limits>` adds a lifetime to `LoadOpts`, which is `Clone + Default` (`sparql_store.rs:48-53`).
*   **Rationale:** `sparql_store.rs:99-148`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The synthesizer confirmed the `LoadOpts` derives.
*   ***Proposed Edit:*** File 6, replace the bullets with:
    > "`#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct StoredQuad { s, p, o: String, g: Option<String> }`. `UpdateOutcome::Ground { inserted: Vec<StoredQuad>, deleted: Vec<StoredQuad>, inserted_new: u64, removed_existing: u64 }`. `triples_before()` and `triples_after()` return `Option<u64>` (`None` for ground). `FileLoadResult` gains `inserted_new`, and `delta()` returns `inserted_new as i64`. `LoadOpts` gains `limits: Option<Limits>`, with `Limits: Copy`. `fn graph_size(&self, graph: &str) -> anyhow::Result<u64>`."

### M11 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 225-226](../../sparql-mcp-write-path-hardening.md#L225-L226), [lines 243-250](../../sparql-mcp-write-path-hardening.md#L243-L250).
*   **Discrepancy Description:** The graph and the counts of a pattern update's `Changed` event are undefined.
*   **Rationale:** `sparql.rs:131-139`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. This is a residual gap from the prior B8.
*   ***Proposed Edit:*** After line 250, add: "**Pattern updates.** One `Changed` per constant graph from `classify_update`, with `graph_size` before and after. For an all-graphs update, one `Changed { graph: <active graph> }` with the store-wide `len()` values that pattern updates already compute, so the viewer resyncs."

### M12 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 233-237](../../sparql-mcp-write-path-hardening.md#L233-L237), [lines 473-474](../../sparql-mcp-write-path-hardening.md#L473-L474).
*   **Discrepancy Description:** The ledger rekey has no subject IRI, no graph predicate, no migration rule and no cleanup mechanism.
*   **Rationale:** `oxigraph.rs:279, 287-288`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Replace lines 233-237 with:
    > "**Load ledger.** The subject is `<urn:hkb:meta:file:{sha256}:{sha256(graph IRI)}>`, with the existing predicates plus `meta:graph <g>`. It is written in the data transaction. Old hash-only entries are ignored, so each file is reloaded once, which is idempotent. When an allowed `DROP`/`CLEAR` of `<g>` runs, `update_with_outcome` appends `DELETE WHERE { GRAPH <urn:hkb:meta:loaded-files> { ?f meta:graph <g> ; ?p ?o } }` to the same request. `DROP ALL`/`CLEAR ALL` also clears the ledger."

### M13 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 91-92](../../sparql-mcp-write-path-hardening.md#L91-L92), [lines 254-257](../../sparql-mcp-write-path-hardening.md#L254-L257), [line 327](../../sparql-mcp-write-path-hardening.md#L327).
*   **Discrepancy Description:** It is unclear which store handle enforces the destructive check and which code runs the `reserved_iri` check. The sibling's skolem IRIs (`batch-ingestion.md:283-286, 570`) would be refused by a decorator-level `reserved_iri` check.
*   **Rationale:** Plugins hold the gated store, and recording runs `CLEAR SILENT` through the privileged one (`recording.rs:199`).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** After line 257, add: "`GatedStore` applies the protected-graph and destructive checks, for every caller including plugins. `PrivilegedStore` skips both. `reserved_iri` is checked when an agent payload is parsed (`limits::check_term`, and the batch-ingestion `parse_batch` before skolemization), not in `GatedStore`." In the Limits table, set the "Applies to" cell of `allow_destructive_updates` to "`GatedStore` (all non-privileged writers)".

### M14 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 262-275](../../sparql-mcp-write-path-hardening.md#L262-L275).
*   **Discrepancy Description:** The definition of "destructive" is ambiguous. "Could bind a protected graph" is undecidable.
*   **Rationale:** An implementer cannot write `classify_update` from these rules.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The `egm-design` probe of this table did not run. Re-check it when that lens reports.
*   ***Proposed Edit:*** After the table, add:
    > "**Rules.** A delete template is *fully variable* when its subject, predicate and object are all variables; `DELETE { ?s ?p ?o } WHERE { ?s a :X }` is destructive. A template graph is *variable* when it is `GRAPH ?g`, or when it has no `GRAPH` and no `WITH` fixes it while `USING NAMED` is present. A variable graph counts as touching every graph, protected ones included. It is refused unless `allow_destructive_updates` is set, and it is refused even then when the update has a delete template, unless `?g` is statically bound to constants by a top-level `VALUES` or `FILTER(?g = <iri>)`."

### M15 [`egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Line 179](../../sparql-mcp-write-path-hardening.md#L179), [line 322](../../sparql-mcp-write-path-hardening.md#L322), [lines 355-356](../../sparql-mcp-write-path-hardening.md#L355-L356).
*   **Discrepancy Description:** The "own cap" is never defined, and chunking is never assigned to a file or a phase. After 0b, `cbm_load_graph` becomes one in-memory transaction (`cbm.rs:122, 140-145`). It also emits no events (`server.rs:124`).
*   **Rationale:** CUJ 5 and the `AGENTS.md` `StoreEvent` invariant depend on this.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** File 10, add:
    > "[0b] `cbm_load_graph` parses the generated Turtle into `Vec<StoredQuad>`, then writes chunks of `file_batch_triples`. Each chunk is its own gated transaction, and the gate is released between chunks. It receives the event sink and emits through `emit_split`. The cap is `max_cbm_triples` (new key, default 1,000,000), or state that there is none."

    Add to the 0b exit criterion: "CUJ 5: a `query_sparql` and a second write both complete while a 300,000-triple `cbm_load_graph` runs."

### N1 [`egm-implementation-readiness`] - Severity: Nit
*   **Affected Scope:** [Line 182](../../sparql-mcp-write-path-hardening.md#L182).
*   **Discrepancy Description:** The table says "same process", but CLI subcommands run as separate processes. The `import`, `reload-ontology` and startup `load_ontology_dir` writers are missing.
*   **Rationale:** `AGENTS.md:44-46`; `main.rs:236, 251, 348`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Replace the row with two rows:
    *   "CLI `load-file`, `code-import`, `import` | yes (for uniformity; the RocksDB lock excludes a concurrent `serve`) | none | yes"
    *   "startup `load_ontology_dir`, CLI `reload-ontology` | yes | none | privileged"

### N2 [`egm-implementation-readiness`] - Severity: Nit
*   **Affected Scope:** [Lines 302-305](../../sparql-mcp-write-path-hardening.md#L302-L305), [lines 340-358](../../sparql-mcp-write-path-hardening.md#L340-L358).
*   **Discrepancy Description:** The move of errors from JSON-RPC errors to `isError` results is not listed under Compatibility. The `dry_run` reply drops `status` and the counts (`sparql.rs:88-95`). The code for an oversized file is not named, and the sibling uses `file_too_large`.
*   **Rationale:** These are visible changes for skills.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:** Add these Compatibility bullets:
    *   "Refusals and SPARQL parse errors now return `isError: true` results instead of JSON-RPC errors; argument errors stay JSON-RPC errors."
    *   "`dry_run` keeps `"status": "ok"`."
    *   "`load_ontology_file` over `max_load_file_bytes` returns `limit_exceeded` with `limit_key: "max_load_file_bytes"`."

    Under "Required changes to the companion designs", ask the batch-ingestion design to use the same code instead of `file_too_large`.

## Not covered in this round

The following were requested but not delivered, because `egm-codebase-details` and `egm-design` did not report:
*   the claim-by-claim API and line verification;
*   the full carry-over audit (B2, B3, B4, B8, M7, M9, M10, M17, M22, F1-F10);
*   the design-soundness probe.

Re-run both lenses against this target, then merge their findings into this report as round 1b.
