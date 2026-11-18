# Consolidated Findings Report

**Target:** `docs/designs/sparql-mcp-write-path-hardening.md` (lines 1-524, Draft, 2026-11-18)

**Mode:** REVIEW, focused on implementation readiness. The question: is the design ready for development, starting with Phase 0a?

**Ground truth used:**
*   `context/ground-truth.md`;
*   `AGENTS.md`;
*   the sibling designs (`sparql-mcp-batch-ingestion.md`, `sparql-mcp-viewer-agent-control.md`, `interface-agentic-rust-stylee.md`; the stale WSL assumption in the last one is not counted against the target);
*   the prior review in `../sparql-mcp-batch-ingestion/`;
*   the `crates/sparql-mcp-core` source.

## Reviewer Status

| Reviewer | Status | Raw findings (B / M / N as raised) |
| --- | --- | --- |
| `egm-implementation-readiness` | completed (third dispatch; the first two were cut off by usage-limit errors) | 6 / 12 / 2 |
| `egm-codebase-details` | **not run**: dispatched twice; the first run was cut off by a usage limit and the second had not returned when the synthesizer had to hand back | n/a |
| `egm-design` | **not run**: the first run failed with a usage-limit error; the second had not returned when the synthesizer had to hand back | n/a |
| `egm-comprehension` | not requested | n/a |

**Coverage gap: read this before acting on the verdict.** Two of the three requested lenses did not report, so the following were **not done** in this round:
*   the claim-by-claim verification of file paths, line references and APIs. This covers `RdfParser::rename_blank_nodes`, the `Transaction` API used for ground updates, `lock_owned`, `GetFinalPathNameByHandleW` and its `windows-sys` feature, the test lines cited under Compatibility, and the writer table;
*   the carry-over audit of B2, B3, B4, B8, M7, M9, M10, M17, M22 and F1-F10 from the batch-ingestion review;
*   the design-soundness probe of the guard table and the event and ledger semantics.

Some of these points appear below, but only where `egm-implementation-readiness` raised them or the synthesizer checked them directly. Re-run `egm-codebase-details` and `egm-design` before you treat the whole-design verdict as final.

The synthesizer checked one point itself:
*   **What it checked:** `SparqlStore` is a **synchronous** trait (`crates/sparql-mcp-core/src/domain/sparql_store.rs:157-196`). It has 10 methods: `query`, `update`, `update_with_outcome`, `load_rdf`, `load_rdf_with_triples`, `load_rdf_file`, `export_graph`, `list_graphs`, `triple_count`, `load_ontology_dir`.
*   **What it found:** the trait has no `write_batch`. `LoadOpts` derives `Clone` and `Default` (`:48-53`).

## Verdicts

| Scope | Verdict | Why |
| --- | --- | --- |
| Phase 0a | **READY AFTER FIXES** | Three Blockers (B1-B3) and five Majors (M1, M2, M3, M5, M6) are all concrete text edits. No open research question remains for 0a. As written today, an implementer would have to guess where the gate is taken, how the privileged handle is wired, and how `write_busy` reaches the agent. |
| Whole design | **NOT READY** | The 0b and 0c data shapes are undefined (`UpdateOutcome`, the ledger key and its migration, `Changed` for pattern updates). The guard rules leave decisions to the implementer. The phase/file mapping does not let each phase build green. The sibling design conflicts on skolem IRIs and gate reentrancy. The API verification and the carry-over audit did not run. |

## Severity Breakdown (after deduplication)

| Severity | Count |
| --- | --- |
| Blocker | 3 |
| Major | 15 |
| Nit | 2 |
| **Total** | **20** |

The synthesizer recalibrated some severities. Each such change is noted under "Severity note".

--------------------------------------------------------------------------------

## Prioritized Findings (Blockers -> Major -> Nits)

### Blockers

#### B1. Where the gate is taken is contradictory: a sync decorator versus async `WriteGate::run` - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness` (the synthesizer confirmed the sync trait)
* **Affected Scope:** Lines 161-172, 192-199, 208-209, 449-456
* **Discrepancy Description:**
    * `GatedStore` "implements SparqlStore", which is synchronous (`sparql_store.rs:157-196`), and its write methods "take the gate". The only gate API, however, is the async `WriteGate::run(label, f)`, which also does its own `spawn_blocking`. Dispatch also wraps every tool in `spawn_blocking` (l.208), so a write would hop from a blocking thread to the async gate and then to a second blocking thread.
    * A sync method cannot await. `Mutex::blocking_lock` has no timeout, and both it and `Handle::block_on` panic on an async thread.
    * Two call sites run on async threads today:
        * the CLI calls `load_rdf`, `load_rdf_file` and `load_ontology_dir` directly inside `async fn run` (`main.rs:236, 317, 344, 348`);
        * plugin `call` runs inline on a runtime worker (`server.rs:196-199`).
* **Rationale & Recommendation:**
    * Give `WriteGate` a blocking entry point (`run_blocking`, which uses `block_on(timeout(lock_owned()))`) for the decorator, alongside the async one.
    * Require every caller of a gated method to be on a blocking thread.
    * Forbid nesting.

#### B2. `PrivilegedStore` has no shape, construction or wiring - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 164, 170-172, 180, 456, 482-487
* **Discrepancy Description:** The server holds a single `store: Arc<dyn SparqlStore>` and passes it to every arm. This includes `project_create`, which writes `urn:meta` (`project.rs:86-92`, `server.rs:128`), and the recording tools (`server.rs:134-140`). The design does not say:
    * who builds the privileged handle;
    * which type it has;
    * how the privileged arms receive it;
    * how plugins are kept from it.
* **Rationale & Recommendation:**
    * Specify both structs, with one shared `WriteGate`.
    * Add a `privileged` field on `SparqlMcpServer` and list the arms that use it.
    * Leave `PluginContext.store` (gated) unchanged.
    * Make `PrivilegedStore` `pub(crate)`.

#### B3. `write_busy` and the other refusals cannot reach the agent as `isError` JSON - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 147-148, 196, 332-338, 441-447
* **Discrepancy Description:**
    * Tools map store errors to `McpError` (`sparql.rs:100-102`, `cbm.rs:144`, `project.rs:92`). An `anyhow::Error` raised inside `GatedStore` therefore becomes a JSON-RPC error string, not `isError: true` with `holder`.
    * `GateHolder`, the type of `holder`, and the source of the label are not defined.
    * `ToolError` is in `limits.rs`, file 3, which belongs to phase 0c. CUJ 5's `write_busy` needs it in 0a.
* **Rationale & Recommendation:**
    * Define `ToolError` (with `WriteBusy { holder, waited_secs }`) in 0a.
    * Downcast in every tool's error mapping and return `CallToolResult { is_error: Some(true) }`.
    * Keep argument errors as `McpError::invalid_params`.
    * Define `GateHolder { label, since }`.
* **Severity note:** the reviewer raised six Blockers. The synthesizer kept these three as Blockers because each one stops 0a code from being written without guessing. The other three became M1, M2 and M5, because their fixes are mechanical lists.

### Majors

#### M1. `GatedStore` write methods are not enumerated, and `write_batch` does not exist - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness` (confirmed by the synthesizer)
* **Affected Scope:** Lines 167-169
* **Discrepancy Description:**
    * The text lists "(`update*`, `load_*`, `write_batch`, …)". `write_batch` exists only in the sibling design (`sparql-mcp-batch-ingestion.md:647`).
    * `load_ontology_dir` (called at startup by every subcommand) and `export_graph` are not classified as reads or writes.
* **Rationale & Recommendation:** Add a method classification table, and require later port methods to be added to it.
* **Severity note:** Raised as Blocker; set to Major because the fix is a mechanical list.

#### M2. Phase 0a's protected-graph check has no classifier for `update(&str)` - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 162, 167-170, 421, 477-480
* **Discrepancy Description:**
    * `classify_update` (file 8) is in 0c. Without it, a 0a `GatedStore` cannot tell which graphs a SPARQL string touches.
    * 0a's scope text implies the decorator performs the protected-graph check.
* **Rationale & Recommendation:**
    * State that in 0a `GatedStore` provides the gate only.
    * In 0c, `GatedStore::update*` parses the request and calls `classify_update`, and the load methods check `opts.graph_iri` or the default graph.
    * List file 5 in 0c.
* **Severity note:** Raised as Blocker; set to Major because the fix is a one-sentence scope decision.

#### M3. Tools with several store calls are not atomic under per-call gating - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 167-169, 187-191, 227-228
* **Discrepancy Description:** Gating each call separately lets another writer slip in between calls of the same tool, which defeats "exact counts":
    * `import_kind` runs `CLEAR SILENT GRAPH` and then two `load_rdf` calls (`recording.rs:199-228`);
    * `with_delta` counts, writes, then counts again (`sparql.rs:220-226`).
* **Rationale & Recommendation:** Add `PrivilegedStore::exclusive(label, f)`, which holds the gate once and passes an ungated inner handle. Otherwise, state that interleaving is accepted.

#### M4. Gate reentrancy between this design and the sibling - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 164, 170; sibling `sparql-mcp-batch-ingestion.md:179, 296, 351`
* **Discrepancy Description:** The sibling writes "through `WriteGate::run`" and also through `PrivilegedStore`, which takes the gate itself. The tokio `Mutex` is not reentrant, so nesting the two deadlocks.
* **Rationale & Recommendation:**
    * State which code may hold the gate, and forbid nesting.
    * Add a "Required changes to the companion designs" bullet asking the sibling to use `exclusive`.

#### M5. `SparqlMcpServer::new`, the test builders and the CLI construction are unspecified - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 171, 482-487
* **Discrepancy Description:** The following must change but are not specified:
    * `new` (6 arguments, `server.rs:46-53`);
    * the test builder (`server.rs:334-345`);
    * `main.rs:358-365`;
    * the viewer channel's store handle (`main.rs:372`);
    * the `examples/hello-plugin` documentation.

    It is also not said how CLI subcommands build a gate.
* **Rationale & Recommendation:** Give the new signature. Say that the viewer channel keeps the raw adapter (it only reads). Give the CLI's gated and privileged construction for each subcommand.

#### M6. The async `dispatch_core` conversion is underspecified - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 208-211, 485
* **Discrepancy Description:**
    * The tool functions borrow `&self.store`, `args` and the sink (`server.rs:105-140`), but `spawn_blocking` needs `'static` data.
    * `JoinError` handling, plugin dispatch (`server.rs:194-200`) and the prior M22 semaphore are not addressed.
* **Rationale & Recommendation:**
    * Give the async signature and the clone-and-move pattern.
    * Say which arms stay inline.
    * Run the plugin loop in one `spawn_blocking`.
    * Adopt or reject the semaphore explicitly.

#### M7. The 0a exit criteria cannot be tested as written - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 202-204, 421
* **Discrepancy Description:**
    * "A slow store call does not block `ping`" passes on a multi-thread runtime even without `spawn_blocking`.
    * No mechanism for a slow store is named.
    * Calling `call_tool` needs an rmcp `Peer`.
* **Rationale & Recommendation:** Name three tests, all on a current-thread runtime with a `SlowStore` test double over a temporary RocksDB store:
    * a read completes while a write is blocked;
    * the maximum number of concurrent writers is 1;
    * cancellation: the timeout drops the first future, and the second write gets `write_busy` until the first ends.

#### M8. Files 9 and 10 in 0a contain 0b and 0c work, and the per-phase file lists do not match the file entries - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 419-424, 435-511
* **Discrepancy Description:**
    * 0a lists files 9 and 10, whose bullets are mostly limits, events, `dry_run` and `file_roots`.
    * 0b lists file 8 (0c scope) but not files 10 and 11.
    * 0c lists file 11 (F5, 0b scope).
    * The test updates are only in 0d, yet 0b breaks `tools_sparql.rs`. That leaves 0b red.
    * `examples/ingest_bench.rs` is missing from the file list.
* **Rationale & Recommendation:** Tag each file bullet with its phase, align the Files column, move the test updates into 0b, and add the bench file.

#### M9. The F3 exit criterion cannot be measured with a `SparqlStore` test double - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Line 422
* **Discrepancy Description:** The `len()` calls are inside the adapter (`oxigraph.rs:86, 91, 128, 145, 245, 263`). A test double at the port only sees `triple_count()`.
* **Rationale & Recommendation:** Add a `#[cfg(test)]` counter behind a private `store_len()` in the adapter, or rely on the bench.

#### M10. The 0b data shapes are undefined - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 241-250, 343-346, 460-464
* **Discrepancy Description:**
    * The design does not give the new `UpdateOutcome::Ground` fields, or say what happens to the accessors `triples_before()` and `triples_after()` (`sparql_store.rs:99-128`).
    * `FileLoadResult.delta()` (`:145-148`) is not addressed.
    * `LoadOpts { limits: Option<&Limits> }` adds a lifetime to a `Clone + Default` struct used in trait signatures and `'static` closures.
* **Rationale & Recommendation:** Give the exact types. Use `Option<Limits>` with `Limits: Copy`. State what `StoredQuad` derives.

#### M11. The graph of a pattern update's `Changed` event is undefined - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 225-226, 243-250, 266
* **Discrepancy Description:** Pattern updates keep store-wide `len()` calls, but `Changed` counts are described as per graph. A `GRAPH ?g` update touches every graph. Today one `Changed` event is emitted, tagged with the active graph (`sparql.rs:131-139`).
* **Rationale & Recommendation:**
    * Emit one `Changed` event per constant graph, with per-graph counts.
    * For all-graph updates, emit one active-graph `Changed` event with store-wide counts, so the viewer resyncs.

#### M12. The load ledger rekey has no schema, no migration and no rule for `DROP`/`CLEAR` - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 233-237, 473-474
* **Discrepancy Description:**
    * The ledger subject today is `<urn:hkb:meta:file:{hash}>` (`oxigraph.rs:287-288`), and it is written by a separate call (`:279`).
    * The design gives no new subject IRI or graph predicate.
    * It does not say how existing hash-only entries are treated.
    * It does not say how cleanup is attached to a `DROP` that runs as a SPARQL update.
* **Rationale & Recommendation:**
    * Give the new subject pattern and predicates.
    * Choose whether old entries are ignored or migrated.
    * Specify that cleanup is a `DELETE WHERE` on the ledger, appended in the same transaction, and that `DROP ALL` drops the whole ledger.

#### M13. Where the destructive and `reserved_iri` checks run is ambiguous, and the sibling's skolem IRIs conflict with `reserved_iri` - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 91-92, 183, 254-257, 327
* **Discrepancy Description:**
    * `allow_destructive_updates` "applies to `update_sparql`", yet plugins hold the gated store. It is unclear whether privilege also skips the destructive check, which recording's `CLEAR SILENT` (`recording.rs:199`) depends on.
    * The sibling writes `urn:sparql-mcp:genid:` IRIs through the gated path (`batch-ingestion.md:283-286, 570`), so a `GatedStore` payload check would refuse them.
* **Rationale & Recommendation:**
    * `GatedStore` applies the protected-graph and destructive checks; `PrivilegedStore` skips both.
    * `reserved_iri` is a parse-time check on agent payloads (`check_term`, and the sibling's `parse_batch` before skolemization), not a decorator check.

#### M14. Guard rules leave decisions to the implementer - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 265-266
* **Discrepancy Description:**
    * "Only if a delete template is fully variable" does not say whether `DELETE { ?s ?p ?o } WHERE { ?s a :X }` counts as destructive.
    * "Always if the WHERE clause could bind a protected graph" is undecidable in general.
* **Rationale & Recommendation:**
    * Define "destructive" syntactically: a delete template whose subject, predicate and object are all variables.
    * Treat a variable graph as touching every graph, including the protected ones. Allow an exception only when a `VALUES` or `FILTER(?g = <const>)` binding is statically constant.

#### M15. `cbm_load_graph`'s "own cap" and chunking are not specified or assigned to a phase - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 179, 322, 355-356
* **Discrepancy Description:**
    * `cbm_load_graph` loads the whole Turtle string with one `load_rdf` call (`cbm.rs:122, 140-145`). After 0b, a whole-codebase import becomes one in-memory transaction.
    * No file bullet mentions chunking.
    * The "own cap" is never defined.
    * `cbm_load_graph` receives no event sink (`server.rs:124`), which conflicts with the `AGENTS.md` `StoreEvent` invariant.
* **Rationale & Recommendation:**
    * In file 10 [0b], specify chunked gated transactions of `file_batch_triples`, with the gate released between chunks.
    * Name the cap key, or state that there is none.
    * Decide event emission.
    * Add a CUJ 5 test to the 0b exit criterion.

### Nits

#### N1. The writer table calls the CLI "same process" and is missing some CLI writers - Severity: Nit
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Line 182
* **Discrepancy Description:**
    * CLI subcommands run as separate processes. The RocksDB lock already prevents them from running alongside `serve` (`AGENTS.md:44-46`).
    * `import` (`main.rs:251`), `reload-ontology` and the startup `load_ontology_dir` (`main.rs:236, 348`) are not listed.
* **Rationale & Recommendation:** Reword the row, and add the missing writers.

#### N2. Error and `dry_run` compatibility is incomplete - Severity: Nit
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 302-305, 332-338, 357-358
* **Discrepancy Description:**
    * Some errors move from JSON-RPC errors to `isError` results (`sparql.rs:102`), and this is not listed under Compatibility.
    * The `dry_run` reply drops `status`, `triples_before`, `triples_after` and `delta` (`sparql.rs:88-95`).
    * The file-too-large code is not named. The sibling uses `file_too_large` (`batch-ingestion.md:250`).
* **Rationale & Recommendation:**
    * List the error-channel change.
    * Keep `"status": "ok"`, or list its removal.
    * Use `limit_exceeded` with `limit_key: "max_load_file_bytes"`, and align the sibling's code with it.

--------------------------------------------------------------------------------

## Verification and carry-over: not done in this round

| Item requested | Status |
| --- | --- |
| Verify `RdfParser::rename_blank_nodes`, `Transaction` APIs (ground update inside a transaction), `lock_owned`, `GetFinalPathNameByHandleW` / `windows-sys` features, Compatibility test lines, writer table, F1-F10 "Where" column | **Not done** (`egm-codebase-details` not run). Only the sync trait and the absence of `write_batch` were checked. |
| Carry-over of B2, B3, B4, B8, M7, M9, M10, M17, M22, F1-F10 from the batch-ingestion review | **Not done** (`egm-codebase-details` not run). Partial signals from `egm-implementation-readiness`: the M22 semaphore is not addressed (M6 here); the B3 writer list misses CLI `import`, `reload-ontology` and `load_ontology_dir` (N1); B8's `Changed` payload for pattern updates is still open (M11). |
| Keys, names and ownership consistency with `sparql-mcp-batch-ingestion.md` | Partly done. Conflicts found: gate reentrancy (M4), skolem IRIs versus `reserved_iri` (M13), and `file_too_large` versus `limit_exceeded` (N2). |
