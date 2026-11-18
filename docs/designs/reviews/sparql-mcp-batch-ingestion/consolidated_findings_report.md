# Consolidated Findings Report

**Target:** `docs/designs/sparql-mcp-batch-ingestion.md` (lines 1-660, Draft, 2026-11-18)

**Mode:** REVIEW. One instance of each of 8 reviewers ran in parallel.

**Ground truth used:** `context/ground-truth.md`, the three sibling designs, the
`crates/sparql-mcp-core` source, and the registry sources for Oxigraph 0.5.7,
oxrdfio 0.2.5, oxttl 0.2.3 and spargebra 0.4.6. The stale WSL assumption in
`interface-agentic-rust-stylee.md` was not counted against the target.

## Reviewer Status

| Reviewer | Status | Raw findings (B / M / N as raised) |
| --- | --- | --- |
| `egm-comprehension` | completed | 0 / 11 / 12 |
| `egm-design` | completed | 3 / 14 / 11 |
| `egm-structural` | completed | 0 / 7 / 9 |
| `egm-style` | completed | 0 / 15 / 7 |
| `egm-zoom-out` | completed | 0 / 6 / 4 |
| `egm-implementation-readiness` | completed | 7 / 15 / 6 |
| `egm-codebase-details` | completed | 2 / 9 / 7 |
| `egm-inventive-catalyst` | completed | 0 / 8 / 2 (persona caps REVIEW severity at Major) |

All 8 reviewers completed. None needed a retry.

## Severity Breakdown (after deduplication)

| Severity | Count |
| --- | --- |
| Blocker | 8 |
| Major | 25 |
| Nit | 16 |
| **Total** | **49** |

The synthesizer recalibrated some severities where reviewers disagreed. Each
such case is noted under "Severity note" in the finding.

--------------------------------------------------------------------------------

## Verification Results

### F1-F9 ("Current write path: findings", lines 181-191)

Verified by `egm-codebase-details`. `egm-design` and `egm-implementation-readiness`
checked the same points independently.

| # | Verdict | Evidence | Note |
|---|---|---|---|
| F1 | CONFIRMED | `tools/sparql.rs:72`, `tools/ontology.rs:74,90`, `oxigraph.rs:124` (`fs::read`) | Each load also makes three full copies of the triples (`oxigraph.rs:256-259`, `ontology.rs:192`). |
| F2 | CONFIRMED | `oxigraph.rs:257-261` calls `Store::insert` per quad; Oxigraph `store.rs:752-757` opens and commits one transaction per call | |
| F3 | CONFIRMED, locations incomplete | `oxigraph.rs:86,91,128,145,245,263`; `ontology.rs:92-100`; `sparql.rs:86` | Not listed: `with_delta` (`sparql.rs:212-226`, used by `cbm_load_graph` and the recording imports: 4 to 6 `len()` calls), `project_create` (2), and `stats` (`export.rs:95`). |
| F4 | CONFIRMED, understated | `ontology.rs:177-195`, `sparql.rs:104-128`, `oxigraph.rs:310-341` | Mis-tagging also happens in the **common** case. With `graph_iri` omitted, or `INSERT DATA` without `GRAPH`, the data goes to the default graph but the event is tagged with the active project graph (`ontology.rs:35`, `oxigraph.rs:247-254`). The CLI defaults to the active graph instead (`main.rs:338`). |
| F5 | CONFIRMED | `ontology.rs:189-193`; `main.rs:370` `broadcast::channel(1024)` | |
| F6 | CONFIRMED | `sparql.rs:81-98` returns before parsing and still calls `triple_count()` | The tool schema text "parse without executing" (`sparql.rs:34,42`) is false today. |
| F7 | CONFIRMED | No guard in `update_sparql` | |
| F8 | CONFIRMED, scope incomplete | `ontology.rs:125-145` leads to `oxigraph.rs:124` | `export_graph` writes to any path (`export.rs:60-68`). `cbm_load_graph` reads any SQLite path (`cbm.rs:70-73`). |
| F9 | CONFIRMED | `server.rs:180-191` (async `call_tool` calls sync `dispatch_core` inline); plugins also run inline (`server.rs:196-199`) | Reads (`query_sparql`, `stats`, `export_graph`) also block workers. |

### Oxigraph 0.5.7 claims

The synthesizer spot-checked these in the registry `store.rs` and `sparql/update.rs`.

| Claim (target lines) | Verdict | Evidence |
|---|---|---|
| `Store::len()` "executes a full scan" (49-51) | CONFIRMED | `store.rs:507`; `Transaction::len` also scans (`store.rs:1286`) |
| `Store::insert` = one transaction per call (43-45, 184) | CONFIRMED | `store.rs:752-757` |
| `start_transaction` + `extend` + `commit` (184, 601-602) | CONFIRMED | `store.rs:580` (readable transaction), `Transaction::extend` `:1476`, `commit` `:1629`. The transaction keeps all changes in memory (`store.rs:552-553`). `Store::extend` already wraps this (`store.rs:764-774`). |
| `contains` inside a transaction (273-275), **marked unverified in ground truth** | CONFIRMED | `Transaction::contains` `store.rs:1279`. It reads through a WriteBatchWithIndex over a snapshot (`rocksdb_wrapper.rs:474-494`), so it sees the transaction's own uncommitted writes and counts duplicates within a batch correctly. |
| One SPARQL update request (`ADD …; DROP …`) = one transaction (287-291), **marked unverified in ground truth** | CONFIRMED | `update.rs:99-121` (`on_store` opens one transaction) and `update.rs:186-222` (one `commit` after `eval_all`). `Transaction::update` / `on_transaction` (`store.rs:1322-1340`) can combine a SPARQL update and the manifest quad writes in one commit. |
| `BulkLoader` is "non-transactional by design" (474-476; ground truth line 16) | **WRONG** | Atomic by default in 0.5.7. SST files are ingested at `commit()` (`rocksdb.rs:1459-1478`, `rocksdb_wrapper.rs:631-670`). `without_atomicity()` is opt-in (`store.rs:1775-1781`). The ground-truth file has the same error. |
| The write gate prevents "transaction conflicts" (358-361) | **WRONG premise** | RocksDB transactions are a plain WriteBatch or WriteBatchWithIndex with no conflict detection (`rocksdb_wrapper.rs:460-494, 1015-1024`). Concurrent writers silently lose updates. The in-memory backend serializes with a mutex (`memory.rs:71-84`), so in-memory tests cannot detect a missing gate. |
| No `oxhttp`, so no `LOAD <http…>` (103-105) | CONFIRMED | `Cargo.lock`; `LOAD` returns an error without `http-client` (`update.rs:681-690`). `LOAD SILENT` succeeds as a no-op. |
| `without_named_graphs` and `parse_error` with line and column (259-268) | PARTLY | `oxrdfio parser.rs:201`. The named-graph refusal is a `Msg` error with no location (`parser.rs:1174`). Positions are 0-based (`error.rs:148`). RDF/XML errors have no location. |
| "Stable blank nodes" across batches (78-79) | PARTLY | `rename_blank_nodes` is off by default (`oxrdfio parser.rs:109`), so labelled `_:x` keeps its label. Turtle `[]` and collections get random ids on every parse (`oxttl terse.rs:268,304,321,737,762,780`). |
| MOVE/COPY guard per `GraphUpdateOperation` variant (400-403, 585-587) | WRONG model | spargebra has no Add, Move or Copy variants. They are rewritten as `Drop`, `DeleteInsert` and `Drop` (`spargebra parser.rs:1284-1312`). |
| A graph-size port method (209, 595) | NOTE | No `graph_size` exists on `Store` or `Transaction`. It must be a `COUNT` query, which costs O(size of the graph). |
| `validate_timeout` (322, 386) | NOTE | Oxigraph has no query timeout. The only hook is `SparqlEvaluator::with_cancellation_token` (`sparql/mod.rs:346`). |

Other claims checked and found correct:
*   `event_for_another_graph_is_ignored` exists at `crates/sparql-mcp-viewer/src/graph.rs:518-532`.
*   The load ledger `urn:hkb:meta:loaded-files` is at `oxigraph.rs:21`.
*   The `write_doc` and `stats` tools exist (`server.rs:127,131`).
*   The output fields `triples_before`, `triples_after` and `delta` exist.
*   `sha2` 0.10.9 and `spargebra` 0.4.6 are already dependencies. `ulid` is **not** a dependency.

--------------------------------------------------------------------------------

## Prioritized Findings (Blockers -> Major -> Nits)

### Blockers

#### B1. Idempotency, job-state and job-total checks run outside the write gate - Severity: Blocker
* **Reviewer Lens:** `egm-design`, `egm-inventive-catalyst`, `egm-implementation-readiness`
* **Affected Scope:** Lines 251-275 (steps 1, 2 and 4 against step 5), 63-69, 128-134, 523
* **Discrepancy Description:** The job-open check, the `(job, seq)` lookup and the `max_job_triples` check all run before the gate is taken. CUJ 2 creates exactly this race: the client times out while the first call is still waiting for the gate, then re-sends the batch. Both calls see no record and both write. The result is two batch records, double-counted or lost totals, and no `seq_conflict` if the content changed. A batch can also pass the "open" check and then write after a commit, abort or sweeper run has already moved or dropped the staging graph. That leaves an orphan staging graph.
* **Rationale & Recommendation:** Keep the checks before the gate as fast paths only. Inside the gated transaction, re-check manifest state, the `(job, seq)` record and totals, using `Transaction::contains` and `quads_for_pattern`. Derive totals from insert-only batch records so they need no read-modify-write. Add a test that sends two concurrent batches with the same `seq` on RocksDB.

#### B2. The write gate's correctness model is wrong: no conflict detection, and cancellation can release the gate - Severity: Blocker
* **Reviewer Lens:** `egm-codebase-details`, `egm-design`, `egm-inventive-catalyst`
* **Affected Scope:** Lines 106-107, 353-365, 578-581, 560
* **Discrepancy Description:**
    * The text justifies the gate as preventing "transaction conflicts", but Oxigraph RocksDB transactions never detect conflicts. The real hazard is silent lost updates.
    * If rmcp drops the `call_tool` future (client cancel or timeout), a guard held by the async task is released while the `spawn_blocking` closure keeps writing.
    * The in-memory backend serializes writers with a mutex, so the planned in-memory CUJ tests cannot detect any of this.
* **Rationale & Recommendation:**
    * Restate the rationale: no isolation between writers, last writer wins.
    * Specify `Arc<Mutex>::lock_owned()` with the owned guard moved into the blocking closure.
    * Require tests of the gate and cancellation on a temporary RocksDB store.

#### B3. Many existing writers bypass the gate, limits, guards and `file_roots` - Severity: Blocker
* **Reviewer Lens:** `egm-zoom-out`, `egm-design`, `egm-codebase-details`, `egm-implementation-readiness`
* **Affected Scope:** Lines 11-12, 91-93, 215-216, 355-357, 395-399, 433-434, 565-650
* **Discrepancy Description:** The plan covers only `update_sparql`, `load_ontology` and `load_ontology_file`. These writers are not covered:
    * `cbm_load_graph` (`cbm.rs:87-146`) takes an arbitrary `graph_iri`, which can be `urn:meta` or a staging graph. It loads the whole graph unbounded and calls `len()` 4 times through `with_delta`.
    * The recording imports (`recording.rs:191-229`) run `CLEAR` and call `len()` 6 times.
    * `project_create` writes `urn:meta`.
    * `mcp/importer.rs:83`.
    * `ToolPlugin`s receive a raw `Arc<dyn SparqlStore>` (`plugin/mod.rs:16-21`).
    * The CLI `load-file` and `code-import`.

    Because the gate is the only isolation, any of these breaks exact counts and the protected-graph invariant. Line 433 says `kb-workbench` "ingests per note". In fact its documented bulk path is `cbm_load_graph`, and it reads `triples_before`, `triples_after` and `delta` (`SKILL.md:206`). Changing `load_rdf_impl` to one transaction also turns a whole-codebase `cbm_load_graph` into one memory-heavy transaction.
* **Rationale & Recommendation:**
    * List every writer with a gated / limited / protected decision for each.
    * Enforce the gate and the protected-graph check in a `SparqlStore` decorator or in the adapter, with a privileged handle for core code, so plugins cannot bypass them.
    * Say which limits apply at the tool boundary and which apply at the adapter.
    * Explain how `ingest_*` relates to `cbm_load_graph`.

#### B4. Protected-graph and destructive-update guards can be bypassed, and the operation model is wrong - Severity: Blocker
* **Reviewer Lens:** `egm-design`, `egm-codebase-details`, `egm-implementation-readiness`
* **Affected Scope:** Lines 166-172, 393-404, 583-587
* **Discrepancy Description:**
    * `DELETE { GRAPH ?g { ?s ?p ?o } } WHERE { GRAPH ?g { ?s ?p ?o } }` wipes every named graph, including `urn:meta`, the jobs graph and the ledger, with no DROP or CLEAR.
    * `INSERT { GRAPH ?g {…} } WHERE { VALUES ?g { <urn:meta> } }` writes into a protected graph.
    * spargebra has no Add, Move or Copy variants; they are rewritten as `Drop` and `DeleteInsert`. So "unit-tested against each `GraphUpdateOperation` variant" cannot express the MOVE/COPY rule.
    * Undefined: the default graph, `LOAD SILENT`, and the return type of `classify_update`.
* **Rationale & Recommendation:**
    * Treat a variable `GraphNamePattern` in an insert or delete template as touching all graphs (refuse, unless destructive updates are allowed).
    * Treat a fully variable delete template as destructive.
    * Detect COPY and MOVE from their rewritten form.
    * Refuse `LOAD` explicitly.
    * Add a table that maps each variant to the graphs it touches, whether it is destructive, and the protected-graph check.

#### B5. No tool contracts or error-code catalogue for the 7 ingest tools - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-structural`, `egm-comprehension`, `egm-style`
* **Affected Scope:** Lines 59-63, 251-351, 409-411, 614-617
* **Discrepancy Description:**
    * Arguments appear only in passing: `mode`, `replace`, `expected_batches`, `label`, `format`, the target graph, `blank_nodes`, `require_validation`, `resume_job`, `force`, `checks`. It is unclear which tool takes which argument.
    * Only `ingest_batch` has a reply example. The `begin`, `status` (with and without an id), `abort`, `validate`, `commit` and `file` replies are undefined.
    * About 14 error conditions have no code: unknown job, job not open, `max_open_jobs` reached, invalid target, validation missing, stale or failed, roots unset, path outside the roots, blank node rejected, and others.
    * The error JSON has no field for the key name versus the value (`limit`), and no holder id for `write_busy`.
    * It does not say whether errors use `isError: true` or `McpError` (which the code uses today, `server.rs:202`).
    * The sibling agent-control design has a "MCP tools (core)" section; this one does not.
* **Rationale & Recommendation:** Add a "MCP tools" subsection. For each tool, give the input schema with defaults, the reply shape, and the error codes it can return. Add one error-code table with columns code, trigger, payload fields and retryable.

#### B6. Job ledger RDF schema and port signatures are undefined - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`, `egm-comprehension`, `egm-style`
* **Affected Scope:** Lines 209, 235-241, 271-273, 324, 336-341, 589-597, 609-611
* **Discrepancy Description:**
    * The following are undefined: subject IRI patterns, predicates, datatypes, state encoding, how manifest totals are replaced, and storage of the validation result and file-job checkpoint. Two implementers would produce incompatible ledgers, and the CUJ 4 resume contract depends on the ledger.
    * The port is left open ("`[String; 4]`, or a `StoredQuad`").
    * `write_batch(quads, extra_quads)` is insert-only, so totals pile up as duplicate values.
    * The layer diagram (209) and file 5 (593-596) disagree on method names.
    * It is undecided which layer parses and skolemizes. The application layer "has no infrastructure imports" (`application/mod.rs:3`).
* **Rationale & Recommendation:** Add a ledger schema table: subject pattern, predicate, datatype, cardinality, writer. Add the full Rust trait diff with types (`StoredQuad`, `BatchWrite`, `ParsedBatch`), and say where parsing, term checks and skolemization run. Either use `ledger_inserts`/`ledger_removes`, or run the manifest change through `Transaction::update`.

#### B7. The job state machine is incomplete, and a retried replace commit can wipe the target - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-design`, `egm-comprehension`, `egm-structural`, `egm-style`
* **Affected Scope:** Lines 218-231, 285-295, 338, 524-526
* **Discrepancy Description:**
    * `running`, `interrupted` and `failed` are missing, and so is a substate for whether validation passed or is stale.
    * There is no failure or refusal edge from `committing`, and no restart rule for it.
    * It does not say whether abort applies to running jobs, whether a file job commits itself at EOF, or what a duplicate seq after commit returns.
    * The commit's manifest update is not stated to share the commit transaction. If the core crashes between the two, a retried `replace` commit runs `MOVE <empty staging> TO <target>`. spargebra emits `DROP SILENT target` first, so the target is wiped.
    * The diagram itself is malformed (lines 226 and 230).
* **Rationale & Recommendation:**
    * Replace the diagram with a transition table: from, event, to, side effects, persisted or not.
    * Commit with `start_transaction`, then `update` through `on_transaction`, then the manifest quads, then a single `commit()`.
    * Refuse a replace commit when staging is empty unless the caller opts in.
    * Define restart recovery for each persisted state.

#### B8. `Changed` event payload and output-field compatibility conflict with removing `len()` - Severity: Blocker
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-design`, `egm-comprehension`, `egm-structural`, `egm-codebase-details`
* **Affected Scope:** Lines 186-187, 296-301, 421-427, 559, 624-627
* **Discrepancy Description:**
    * `StoreEvent::Changed { graph, triples_before, triples_after }` (`event_sink.rs:89-93`) has `u64` fields that the viewer reads (`sparql-mcp-viewer/src/ingest.rs:90-95`). The wire format is owned by `interface-agentic-rust-stylee.md`.
    * The design emits `Changed` for large and replace commits but bans the `len()` calls that filled those fields, and never says what they will contain.
    * Setting `triples_before` and `triples_after` to `null` breaks `tests/tools_sparql.rs:122-123,145-146` and `tests/store.rs:225-231`. The Phase 0 exit says those tests must pass.
    * `delta` → `inserted_new` has no counterpart for `DELETE DATA`, where `delta` is negative today.
    * The `dry_run` output shape is undefined.
* **Rationale & Recommendation:**
    * Specify the `Changed` payload for each path: per-graph counts, or optional fields recorded as a change to the sibling design.
    * Add `removed_existing`.
    * List the tests to update.
    * Give before and after output schemas for each existing tool.

### Majors

#### M1. Alternative B rejects `BulkLoader` on an outdated premise - Severity: Major
* **Reviewer Lens:** `egm-codebase-details`, `egm-inventive-catalyst`
* **Affected Scope:** Lines 471-479 (and `context/ground-truth.md` line 16)
* **Discrepancy Description:** In 0.5.7 `BulkLoader` is atomic by default. Ledger quads could be committed in the same atomic ingest. The valid objections are different: it can only insert, it gives no `contains` counts, it uses threads and SST files for each call, and a crash leaves orphan SST files.
* **Rationale & Recommendation:** Rewrite the Cons with the accurate reasons. Note that it is a candidate for large staged commits and direct file jobs. Correct the ground-truth note.
* **Severity note:** `egm-codebase-details` raised this as a Blocker. The synthesizer set it to Major because the rejection of BulkLoader for small batches still holds on the accurate reasons.

#### M2. Staged-commit memory and gate hold time are asserted, not quantified - Severity: Major
* **Reviewer Lens:** `egm-design`, `egm-codebase-details`, `egm-inventive-catalyst`, `egm-structural`
* **Affected Scope:** Lines 287-291, 377, 518-519, 355-365
* **Discrepancy Description:**
    * `ADD` is rewritten as a `DeleteInsert` that collects every mutation into a `Vec` (`update.rs:325-333`). It then stages about 6 index keys per quad, plus 6 deletes per quad for `DROP <staging>`.
    * 500k triples is plausibly several hundred MB to more than 1 GB, held while the gate is held, which can exceed the 30 s `write_wait_timeout` for other writers.
    * `DROP GRAPH` on an empty staging graph errors (`update.rs:404-416`).
* **Rationale & Recommendation:**
    * Measure peak RSS and wall time of a 500k commit in the Phase 1 bench, and derive the default from that measurement.
    * Use `DROP SILENT`.
    * Consider moving the staging drop out of the commit transaction (chunked cleanup), or a BulkLoader commit above a threshold.

#### M3. Replace-mode cost depends on the target size, and it discards concurrent writes - Severity: Major
* **Reviewer Lens:** `egm-inventive-catalyst`, `egm-design`, `egm-implementation-readiness`
* **Affected Scope:** Lines 76-77, 292-293, 403-404
* **Discrepancy Description:**
    * `MOVE` clears the target inside the same transaction, so the memory needed is about |target| + 2 × |staging|, which `max_job_triples_staged` does not bound.
    * A replace commit silently discards writes made to the target since the job began.
    * The commit path's exemption from the destructive guard is not stated.
* **Rationale & Recommendation:**
    * Bound replace by the size of the target graph, or switch the project's physical graph through its `urn:meta` record (a pointer swap).
    * Document that concurrent writes are lost, or refuse them.
    * State that the commit path bypasses the guard.

#### M4. Scope is disproportionate to the stated intent: split hardening out, and justify against a minimal alternative - Severity: Major
* **Reviewer Lens:** `egm-zoom-out` (supported by the volume of B/M findings on staged, file and validate features)
* **Affected Scope:** Lines 23-26, 59-93, 176-192, 462-505, 557-563
* **Discrepancy Description:**
    * F1-F9 are defects in today's tools and directly answer "limit MCP-side risks", yet they ship tied to a 7-tool job platform.
    * The realistic minimal option (hardening plus an idempotent `ingest_batch` and `ingest_status`, direct mode only) is never weighed. Alternative A is a straw man.
    * Staged mode, replace, the sweeper and TTL, `ingest_validate` (which duplicates `query_sparql`), and `ingest_file` with resume make up most of the risk and most of the unverified surface.
* **Rationale & Recommendation:** Split the work into "Write-path hardening" (ship first) and "Batch ingestion". Add an Alternative F for the minimal option. Defer or justify each heavier feature with a concrete need.

#### M5. Blank-node handling: stability holds only for labelled nodes, the existing tools merge labels, and scope across jobs is undefined - Severity: Major
* **Reviewer Lens:** `egm-codebase-details`, `egm-design`, `egm-inventive-catalyst`, `egm-implementation-readiness`, `egm-comprehension`
* **Affected Scope:** Lines 39-42, 78-79, 264-266, 338-341
* **Discrepancy Description:**
    * Today, unrelated `load_ontology` calls that reuse `_:b0` are **merged** into one node, which is silent corruption. The Problem section states the opposite risk.
    * Turtle `[]` and collections get random ids on every parse, so skolem IRIs for them are not stable across retries, and `ingest_file` resume can split them.
    * Per-job skolemization duplicates nodes when the same source is re-ingested in a new job.
    * Users can forge `urn:sparql-mcp:genid:` IRIs.
    * The rules for escaping labels are not given.
* **Rationale & Recommendation:**
    * Add an F10: `rename_blank_nodes()` for the existing tools.
    * Scope the stability claim to labelled blank nodes, and give anonymous nodes deterministic names in order of first appearance.
    * Reject `urn:sparql-mcp:` IRIs in payloads.
    * Consider an optional source-key scope.

#### M6. Idempotency semantics conflict with splitting, `expected_batches` and the workflow text - Severity: Major
* **Reviewer Lens:** `egm-design`, `egm-inventive-catalyst`, `egm-comprehension`, `egm-style`, `egm-implementation-readiness`
* **Affected Scope:** Lines 67-69, 143-145, 238-246, 255-257, 448-452
* **Discrepancy Description:**
    * The rule "only accepted batches create a record" is implied but never stated.
    * Workflow step 3 says "on any error, re-send the same content … always safe". That contradicts step 4.
    * Split remainders at "new seqs at the end" fall outside `1..=expected_batches`.
    * The SHA-256 of raw bytes rejects semantically identical regenerated payloads, which LLM agents often produce.
    * An accepted wrong staged batch can never be corrected.
    * It is unspecified which reply fields are replayed and which are current.
* **Rationale & Recommendation:**
    * State the record rule, and limit step 3 to timeouts, transport errors and `write_busy`.
    * Define completeness as no gaps up to `max(expected_batches, highest seq)`, or let commit carry the part list.
    * Consider a canonical hash (sorted, skolemized N-Triples).
    * Document that abort is the only correction, or add per-seq staging.

#### M7. F4 understates the mis-tagging, and no default-graph convention is defined - Severity: Major
* **Reviewer Lens:** `egm-codebase-details`
* **Affected Scope:** Lines 186, 413-419
* **Discrepancy Description:** `load_ontology` with no `graph_iri`, and `INSERT DATA` with no `GRAPH`, write to the default graph but are tagged with the active project. `StoreEvent.graph` is a `String` with no default-graph value. The CLI and the MCP tool disagree about the default target.
* **Rationale & Recommendation:** Restate F4 to cover this case. Define the event graph value for the default graph. Decide whether `load_ontology` should default to the active graph.

#### M8. File-job resume, whole-file idempotence and the load ledger are underspecified - Severity: Major
* **Reviewer Lens:** `egm-design`, `egm-codebase-details`, `egm-implementation-readiness`, `egm-comprehension`
* **Affected Scope:** Lines 333-341, 349-351
* **Discrepancy Description:**
    * The "SHA-256 computed while reading" cannot support skipping a completed file before the load starts.
    * The "prefix hash at quad N" is ill-defined when `BufReader` and the parser read ahead.
    * Turtle needs a full re-parse from the start.
    * The checkpoint fields are not in the manifest.
    * The existing ledger is keyed by hash only (`oxigraph.rs:267-277`), so a second project graph is skipped.
    * The ledger is written in a separate transaction from the data (`oxigraph.rs:143-144`).
    * Entries stay stale after a replace or DROP.
* **Rationale & Recommendation:**
    * Hash the whole file in a pre-pass at begin.
    * Resume line-based formats from a byte offset at a line boundary, and restart Turtle from the beginning.
    * Key the ledger by (hash, target graph), write it in the data transaction, and invalidate entries on replace or drop.

#### M9. `file_roots` on native Windows, and other filesystem exposures - Severity: Major
* **Reviewer Lens:** `egm-design`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-comprehension`
* **Affected Scope:** Lines 190, 330-332, 381-383, 428-431, 530, 639
* **Discrepancy Description:**
    * Check-then-open has a TOCTOU window.
    * `canonicalize` returns `\\?\` paths, so roots must be canonicalized the same way and compared case-insensitively.
    * UNC paths make the core open an SMB connection, which can leak NTLM credentials and contradicts "no network loads".
    * ADS and device paths are not handled.
    * The TOML type and escaping are not given.
    * The symlink test needs elevation or a junction on Windows.
    * With `file_roots` unset (the default), F8 is not fixed.
    * `export_graph` (arbitrary write) and the `cbm_load_graph` DB path are not covered.
* **Rationale & Recommendation:**
    * Open the file first, check the path from the handle (`GetFinalPathNameByHandleW`), and read from that handle.
    * Reject UNC, device and ADS paths.
    * Give the struct and a TOML example.
    * Cover the write paths, and state when the default becomes enforcing.

#### M10. `load_ontology_file` can still do a 2 GiB load in memory, in one transaction - Severity: Major
* **Reviewer Lens:** `egm-design`, `egm-zoom-out`
* **Affected Scope:** Lines 183-184, 381, 428-431, 599-602
* **Discrepancy Description:** The F2 fix ("parse to a Vec, then one transaction") together with `max_file_bytes` = 2 GiB allows about 15M+ triples in one transaction, which Oxigraph warns against.
* **Rationale & Recommendation:** Give `load_ontology_file` a small cap and stream it in chunked transactions, or send larger files to `ingest_file`.

#### M11. Direct-mode and file jobs flood the viewer, because the event threshold is applied per call - Severity: Major
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 89-90, 276-277, 296-301, 384-385, 517-521
* **Discrepancy Description:** A 10M-triple file job emits about 2,000 events of up to 5k triples each. The 1024-slot broadcast lags, and the viewer resyncs again and again against a growing graph.
* **Rationale & Recommendation:** Apply `event_changed_threshold` cumulatively per job. Above it, emit rate-limited `Changed` events plus one final event.

#### M12. CUJ 1's "`job commit` feed entry" contradicts the event model - Severity: Major
* **Reviewer Lens:** `egm-comprehension`, `egm-structural`, `egm-implementation-readiness`, `egm-design`
* **Affected Scope:** Lines 123-124, 296-301, 385, 547-548
* **Discrepancy Description:** No `job` event kind exists, and it is listed as a Future Improvement. A 60k-triple commit is above the 50k threshold, so the feed shows a `changed`/resync entry.
* **Rationale & Recommendation:** Reword CUJ 1 to "one `changed` entry and a resync".

#### M13. Performance targets are inconsistent and cannot be tested - Severity: Major
* **Reviewer Lens:** `egm-style`, `egm-comprehension`, `egm-structural`, `egm-implementation-readiness`
* **Affected Scope:** Lines 86-88, 119-120, 164, 511-516, 560, 562, 656-657
* **Discrepancy Description:**
    * The Goals say 50k triples/s for batches, measured in Phase 1. The NFR says file jobs, which only exist in Phase 3.
    * The Phase 1 bench measures latency only.
    * "The author's machine", "stays responsive" and "memory stays flat" have no thresholds or measurement method on Windows.
* **Rationale & Recommendation:**
    * Give one target per path, each in the exit criterion of the phase that owns it.
    * Name the reference hardware, or say the result is recorded rather than gated.
    * Give p95 and peak-RSS bounds, with a Windows-capable measurement method.

#### M14. Phase exit criteria and CUJ traceability are inconsistent - Severity: Major
* **Reviewer Lens:** `egm-structural`, `egm-comprehension`, `egm-implementation-readiness`, `egm-codebase-details`
* **Affected Scope:** Lines 557-563, 637-639, 652-659
* **Discrepancy Description:**
    * The Phase 1 exit "CUJs 1-4 with an in-memory store" includes `ingest_validate` (Phase 2) and `viewer_show`, which belongs to another design.
    * CUJ 4 cannot run in memory.
    * F3, F6, F8 and F9 have no stated assertion. How is "no `len()` call" tested?
    * CUJ 6's project-graph and protected-graph refusals are not automated.
    * There is no rule that a phase starts only after the previous one passes.
* **Rationale & Recommendation:**
    * Add a traceability table: CUJ, phase, test.
    * Restrict the Phase 1 exit to CUJs 2-4, with CUJ 4 on RocksDB.
    * Give one assertion per finding, using a counting test double for F3.
    * Add the gating sentence.

#### M15. Job TTL and expiry undercut resume, and drops are unbounded - Severity: Major
* **Reviewer Lens:** `egm-design`, `egm-inventive-catalyst`
* **Affected Scope:** Lines 82-85, 147-153, 230, 247-249, 380
* **Discrepancy Description:**
    * The sweeper runs at core start and expires jobs idle for more than 1 h. A session restarted after more than an hour, which is the CUJ 4 class, loses its staged work.
    * Abort and expiry drops of 500k staging graphs are single large transactions run while holding the gate.
    * How `interrupted` file jobs interact with the TTL is not defined.
* **Rationale & Recommendation:** Count idle time only while a core is running, or add a startup grace period. Make expiry a resurrectable mark with lazy eviction. Run every drop as chunked cleanup.

#### M16. Direct-mode partial data contradicts "never half-committed", and there is no retention policy - Severity: Major
* **Reviewer Lens:** `egm-comprehension`, `egm-design`
* **Affected Scope:** Lines 74-75, 247-249, 376-379, 522-526
* **Discrepancy Description:**
    * An expired or aborted direct job leaves its batches in the target graph, with no provenance for each batch.
    * The jobs graph grows forever.
    * Four crashed jobs can block new jobs for an hour.
* **Rationale & Recommendation:**
    * Limit the "never half-committed" NFR to staged mode, and state that direct mode is atomic per batch only.
    * Add retention for committed manifests.
    * Add per-batch provenance or a warning for direct mode.

#### M17. The "no full scans" claim is broader than the design supports, and the counts measure the wrong thing - Severity: Major
* **Reviewer Lens:** `egm-style`, `egm-inventive-catalyst`, `egm-design`, `egm-codebase-details`
* **Affected Scope:** Lines 49-51, 86, 185, 271-275, 302-305, 514-515
* **Discrepancy Description:**
    * The commit reply's `COUNT` and "distinct new subjects" cost O(target graph).
    * Pattern updates still call `len()`, and so do `with_delta` and `stats`.
    * In staged mode, `inserted_new` and `already_present` are relative to staging, not the target.
* **Rationale & Recommendation:**
    * Reword the claim to "no whole-store scans on ground writes".
    * Rename the staged counts, or compute `new_to_target` at commit.
    * Consider a per-graph counter in `urn:meta`.

#### M18. Terminology drift: "ledger", triples versus quads, and limit key names - Severity: Major
* **Reviewer Lens:** `egm-style`, `egm-comprehension`, `egm-structural`
* **Affected Scope:** Lines 19, 54, 66, 189, 204, 209, 270, 291, 335, 340, 342-343, 372, 376-378, 504, 523
* **Discrepancy Description:**
    * "Ledger" means both the existing load ledger and the new job state.
    * "Triples" and "quads" are mixed within one clause (335).
    * `max_job_triples` is cited in prose, but only `max_job_triples_staged` and `max_job_triples_direct` exist.
* **Rationale & Recommendation:**
    * Define "job ledger" and "load ledger" separately.
    * Use "triples" for inputs and limits, and "quads" only at the store level.
    * Use the exact key names from the Limits table.

#### M19. No Risks/Open Questions section and no "Required changes to the companion designs" - Severity: Major
* **Reviewer Lens:** `egm-structural`
* **Affected Scope:** Whole document. Relevant passages: 28-35, 287-291, 296-301, 423-427, 553-563
* **Discrepancy Description:**
    * The ground truth lists open points, and the risks are scattered through the document.
    * Both sibling designs record the changes they need from companion documents. This design changes the `Changed` payload and the event-tagging contract owned by `interface-agentic-rust-stylee.md`, and the feed kinds in `sparql-mcp-viewer.md`, without recording them.
* **Rationale & Recommendation:**
    * Add a "Risks and Open Questions" table.
    * Add a "Required changes to the companion designs" section before "Delivery phases".

#### M20. Limits and configuration are underspecified and partly uncalibrated - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-style`, `egm-structural`
* **Affected Scope:** Lines 367-391, 371, 374-375, 432-434, 518, 567-570
* **Discrepancy Description:**
    * `IngestConfig` field types and units are not given. Durations and sizes in particular could be integers or strings.
    * It is not said whether the section is `Option<>` or `#[serde(default)]`.
    * What "term bytes" measures is not defined.
    * Four rows have "—" as their rationale.
    * The "10× the input" figure, "rarely exceeds a few hundred KB", and "above any write the current skills produce" are not measured.
    * Fixed constants are buried in prose: the 5-minute sweep, 20 rows, 200 bytes.
* **Rationale & Recommendation:**
    * Give the Rust struct with serde attributes and a TOML example.
    * Define how terms are measured.
    * Fill in the rationale cells.
    * Mark the estimates as such, to be confirmed by `ingest_bench`.
    * List the fixed constants.

#### M21. Key terms are undefined: ground versus pattern writes, write gate, skolemize, the skills - Severity: Major
* **Reviewer Lens:** `egm-comprehension`
* **Affected Scope:** Lines 78-79, 98, 185, 271, 440, 514-515, 559, 597
* **Discrepancy Description:** The F3 fix and the NFR depend on the ground/pattern split, which is defined only in `interface-agentic-rust-stylee.md`. "Write gate" and "skolemize" are used before they are defined.
* **Rationale & Recommendation:** Define ground and pattern updates near F3. Gloss each term at its first use.

#### M22. Async dispatch, constructor changes and F9 coverage are unspecified - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-design`, `egm-codebase-details`
* **Affected Scope:** Lines 191, 258-261, 362-363, 629-633
* **Discrepancy Description:**
    * `dispatch_core` is synchronous, and `every_declared_tool_has_a_dispatch_arm` calls it synchronously.
    * The `SparqlMcpServer::new` signature changes are not given.
    * Reads and the 1 MiB parse step still run on Tokio workers, with unbounded parallelism.
* **Rationale & Recommendation:**
    * Specify the async dispatch shape and the constructor or builder.
    * Run all store-touching work and parsing in `spawn_blocking`, behind a small semaphore.
    * Update the dispatch test.

#### M23. `ingest_validate` is underspecified, and arguably redundant - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-design`, `egm-zoom-out`
* **Affected Scope:** Lines 307-326, 386, 561
* **Discrepancy Description:**
    * There is no timeout mechanism. Only `with_cancellation_token` exists.
    * Whether `{{staging}}` substitution includes angle brackets is not stated.
    * Missing: a cap on the number of checks and query size, how the 20-row cap is applied, the allowed query forms, the direct-mode error, and a marker for "no batch since validation".
    * The zoom-out reviewer argues the tool duplicates `query_sparql`.
* **Rationale & Recommendation:** Specify all of these, or cut Phase 2 and return the staging IRI so agents can validate with `query_sparql`. The second option overlaps M4.

#### M24. The commit reply cannot drive `viewer_show` - Severity: Major
* **Reviewer Lens:** `egm-style`, `egm-design`, `egm-comprehension`, `egm-implementation-readiness`
* **Affected Scope:** Lines 125, 302-305, 455-456
* **Discrepancy Description:**
    * The target is called "new classes", "distinct new subjects" and "new top-level subjects" in different places.
    * The reply returns a count, but `viewer_show` takes up to 5,000 IRIs.
* **Rationale & Recommendation:** Define one term. Return a capped list of IRIs plus the count, and give the algorithm and its cost.

#### M25. Target files and dependencies are incomplete - Severity: Major
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-codebase-details`, `egm-structural`
* **Affected Scope:** Lines 233, 247, 322, 364, 565-650
* **Discrepancy Description:**
    * `ulid` is not a dependency.
    * The tokio `time` feature is enabled only in dev-dependencies.
    * Not listed: `Cargo.toml`, `application/mod.rs`, `application/tools/mod.rs`, `domain/mod.rs`, the writer files from B3, and the tests from B8.
    * Test paths lack the crate prefix.
    * The phase table has no files column.
* **Rationale & Recommendation:** Add the missing entries with full repository-relative paths, and add a files column to the phase table.
* **Severity note:** `egm-implementation-readiness` raised this as a Blocker. The synthesizer set it to Major because it is mechanical to fix.

### Nits

#### N1. Two Oxigraph assumptions are stated without evidence (both now verified) - Severity: Nit
* **Reviewer Lens:** `egm-style`, `egm-comprehension`, `egm-structural`
* **Affected Scope:** Lines 184-185, 273-275, 288-291
* **Discrepancy Description:** The ground truth marked single-transaction updates and `Transaction::contains` as unverified. This review verified both as true.
* **Rationale & Recommendation:** Cite `store.rs:1279` and `update.rs:99-121,186-222`. Prefer the explicit `on_transaction` commit described in B7.
* **Severity note:** `egm-structural` and `egm-style` rated this Major because it was unverified. It is Nit now that both claims are confirmed.

#### N2. "base design CUJ 3" is ambiguous - Severity: Nit
* **Reviewer Lens:** `egm-comprehension`, `egm-structural`
* **Affected Scope:** Lines 300-301
* **Discrepancy Description:** Two related designs have a CUJ 3. The intended one is `sparql-mcp-viewer.md`, CUJ 3 (Recovering from a coarse update).
* **Rationale & Recommendation:** Name the file and the CUJ title.
* **Severity note:** Both reviewers rated this Major. The synthesizer set it to Nit because it is a one-line fix.

#### N3. Decimal and binary units are mixed - Severity: Nit
* **Reviewer Lens:** `egm-style`
* **Affected Scope:** Lines 138, 157, 371, 562, 657
* **Discrepancy Description:** GB and KB are used next to MiB and GiB.
* **Rationale & Recommendation:** Use binary units throughout.

#### N4. Formatting consistency - Severity: Nit
* **Reviewer Lens:** `egm-style`
* **Affected Scope:** Lines 39, 255, 336-340, 449-451, 654
* **Discrepancy Description:**
    * "10k" and "10,000" are used for the same limit.
    * "record" means three different things.
    * `seq` is not always code-formatted.
    * The verification command packs four commands into one.
* **Rationale & Recommendation:** Normalize these, and list the four cargo commands separately.

#### N5. Vague or uncalibrated wording - Severity: Nit
* **Reviewer Lens:** `egm-style`
* **Affected Scope:** Lines 13, 39-40, 233-234, 443, 446, 477
* **Discrepancy Description:** "increasingly", "hundreds of calls", "unguessable enough", "about 500k" (which is exactly the limit), "mostly idle".
* **Rationale & Recommendation:** Drop these words or calibrate them. State that job ids are not a security boundary.
* **Severity note:** Raised as Major. Set to Nit.

#### N6. Document structure and diagram - Severity: Nit
* **Reviewer Lens:** `egm-structural`, `egm-style`
* **Affected Scope:** Lines 176, 218-231, 278-283, 438, 539, 555
* **Discrepancy Description:**
    * Future Improvements is nested under the NFRs.
    * The heading case is mixed.
    * The cross-reference style is inconsistent.
    * The JSON fence breaks the numbered list.
    * Agent workflow has normative numbers that are not tied to limit keys.
    * There is no phase-gating sentence.
* **Rationale & Recommendation:** Apply the structural fixes.

#### N7. `parse_error` line and column are not always available - Severity: Nit
* **Reviewer Lens:** `egm-codebase-details`
* **Affected Scope:** Lines 142, 267-268, 410
* **Discrepancy Description:** Positions are 0-based. Positions are missing for RDF/XML and for the named-graph refusal.
* **Rationale & Recommendation:** Make the fields optional, report them 1-based, and give the named-graph refusal its own code.

#### N8. Staging graphs are not private - Severity: Nit
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 71-73, 121-122, 242-243
* **Discrepancy Description:** `list_graphs`, `stats`, `export_graph` and `GRAPH ?g` queries all see staging graphs.
* **Rationale & Recommendation:** Say "not in the target graph", or filter staging graphs out of `list_graphs`.

#### N9. Turtle batches need self-contained prefixes and a base - Severity: Nit
* **Reviewer Lens:** `egm-design`
* **Affected Scope:** Lines 258-261, 445-447
* **Discrepancy Description:** The base IRI and the rule for repeating prefixes in each batch are not stated.
* **Rationale & Recommendation:** State that each Turtle batch must repeat its `@prefix` lines, and give the base IRI or refuse relative IRIs.

#### N10. The default active graph is not registered in `urn:meta` - Severity: Nit
* **Reviewer Lens:** `egm-codebase-details`
* **Affected Scope:** Lines 405-408
* **Discrepancy Description:** `urn:project:default` is never created in `urn:meta` (`main.rs:35-36`), and `project_switch` does not validate (`project.rs:145-163`).
* **Rationale & Recommendation:** State whether an unregistered active graph is an allowed target.

#### N11. A single global writer serializes jobs that write to separate graphs - Severity: Nit
* **Reviewer Lens:** `egm-inventive-catalyst`
* **Affected Scope:** Lines 353-365, 379
* **Discrepancy Description:** The reviewer proposes per-job and per-target locks.
* **Rationale & Recommendation:** Do not change the design. The real cost is long critical sections, which M2 and M15 address.
* **Severity note:** Raised as Major. Set to Nit.

#### N12. The architecture sections contain implementation detail - Severity: Nit
* **Reviewer Lens:** `egm-zoom-out`
* **Affected Scope:** Lines 279-283, 313-319, 572-627
* **Discrepancy Description:** Helper names and payload examples appear in the design.
* **Rationale & Recommendation:** This conflicts with B5 and B6, which ask for more contract detail. Keep the detail.

#### N13. Reuse opportunities - Severity: Nit
* **Reviewer Lens:** `egm-codebase-details`
* **Affected Scope:** Lines 599-605, 619-622, 322
* **Discrepancy Description:** `Store::extend`, `with_cancellation_token`, and the `with_delta` helper are not named.
* **Rationale & Recommendation:** Name them in the relevant file items.

#### N14. Skill names and descriptions - Severity: Nit
* **Reviewer Lens:** `egm-implementation-readiness`, `egm-comprehension`
* **Affected Scope:** Lines 98, 433-434, 440, 650
* **Discrepancy Description:** The skill is `plugins/kb-workbench/skills/sparql-first-code/SKILL.md`, not `sparql-first`. The skills are not introduced.
* **Rationale & Recommendation:** Give the paths and a one-line description of each skill.

#### N15. Small unplaced rules - Severity: Nit
* **Reviewer Lens:** `egm-implementation-readiness`
* **Affected Scope:** Lines 533, 535-537
* **Discrepancy Description:** The 200-byte quote rule has no helper (and needs truncation on a char boundary). The `stats` field names are not given, and "staging triples" should come from manifest totals.
* **Rationale & Recommendation:** Name the helper, and give the `stats` fields.

#### N16. Alternative C has a broken line wrap and repeats itself - Severity: Nit
* **Reviewer Lens:** `egm-style`
* **Affected Scope:** Lines 484-487
* **Discrepancy Description:** The line breaks early, and the same idea is stated three times.
* **Rationale & Recommendation:** Rewrite it as one sentence.
