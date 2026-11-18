# Goldfish Annotated Review Report

Status: resolved (2026-11-18). All PROPOSE FIX applied in the rewritten target; ESCALATE items decided by the author (see the target's Alternatives and Future Improvements); DECLINE/CONTEST items left unchanged.

**Target:** `docs/designs/sparql-mcp-batch-ingestion.md` (lines 1-660)

**Source:** `consolidated_findings_report.md`, round 1. The IDs (B1-B8, M1-M25,
N1-N16) are the consolidated finding IDs used in that report. That report also
holds the reviewer status, the raw-to-consolidated mapping, and the F1-F9 and
Oxigraph verification tables.

**Tally:** 45 PROPOSE FIX · 1 CONTEST · 2 ESCALATE · 1 DECLINE (49 findings:
8 Blocker, 25 Major, 16 Nit).

**How the findings were checked:** Every finding was checked against the
GoldfishContext: `context/ground-truth.md` (native Windows, no WSL), the sibling
designs, the `crates/sparql-mcp-core` source, and the Oxigraph 0.5.7, oxrdfio,
oxttl and spargebra registry sources. The synthesizer re-read the Oxigraph
facts used by B2, M1 and N1 in `store.rs` and `sparql/update.rs`.

**Note on the ground truth:** `context/ground-truth.md` line 16 says
`BulkLoader` is non-transactional. That is wrong for 0.5.7 (see M1). The
same file lists two points as unverified, and both are now verified. These
corrections are for the author to make. This review did not edit the
ground-truth file.

## Prioritized Findings & AI Suggestions

### B1 [`egm-design`, `egm-inventive-catalyst`, `egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 251-275](../../sparql-mcp-batch-ingestion.md#L251-L275) (Batch processing), [line 523](../../sparql-mcp-batch-ingestion.md#L523).
*   **Discrepancy Description:** Steps 1, 2 and 4 (job is open, `(job, seq)` lookup, job total) run before the write gate. A CUJ 2 retry that runs alongside the original call passes both checks. The result is double batch records, wrong totals, a missed `seq_conflict`, and writes into a staging graph that has already been committed or dropped.
*   **Rationale:** Oxigraph RocksDB transactions have no conflict detection (`rocksdb_wrapper.rs:460-494`), so nothing else catches the race.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Three lenses found it independently, and the code confirms it. The fix is local to the batch steps.
*   ***Proposed Edit:***
    *   Location: Step 5, lines 271-275.
    *   Change: Replace step 5 with:
        ```
        5.  **Write, under the write gate, in `spawn_blocking`.** Inside one
            Oxigraph transaction, first re-check what steps 1, 2 and 4 checked
            (job state is `open`, no record for `(job, seq)`, job total within
            the limit). If a check fails, return `job_not_open`, `duplicate` or
            `seq_conflict`, or `limit_exceeded`, and write nothing. Otherwise
            insert the quads (graph = staging or target) and the batch record,
            and commit. Steps 1, 2 and 4 are fast paths only. Job totals are
            derived from the batch records, never stored as a mutable counter.
            New-versus-duplicate counts come from `Transaction::contains`
            before each insert, so they are exact without `len()`.
        ```
    *   Also add to the Phase 1 exit (line 560): "two concurrent `ingest_batch` calls with the same seq on a temporary RocksDB store produce one record".

### B2 [`egm-codebase-details`, `egm-design`, `egm-inventive-catalyst`] - Severity: Blocker
*   **Affected Scope:** [Lines 353-365](../../sparql-mcp-batch-ingestion.md#L353-L365) (Write gate), [lines 578-581](../../sparql-mcp-batch-ingestion.md#L578-L581).
*   **Discrepancy Description:** The text justifies the gate by "no transaction conflicts", but Oxigraph does not detect conflicts at all. If the request future is dropped, the gate is released while the `spawn_blocking` closure is still writing. In-memory tests serialize writers anyway (`memory.rs:71-84`), so they cannot detect either problem.
*   **Rationale:** The synthesizer verified this in the registry sources. The gate is the only isolation between writers.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The current wording would lead an implementer to rely on library behaviour that does not exist.
*   ***Proposed Edit:***
    *   Location: Lines 358-361.
    *   Change:
        *   Old: `*   **What this gives:** exact new-versus-duplicate counts without `len()`; no transaction conflicts between an agent's `update_sparql` and a running file job; and Tokio workers free for the MCP transport and the viewer channel.`
        *   New: `*   **What this gives:** Oxigraph transactions are not isolated from each other on RocksDB. Concurrent commits are written as-is, and the last writer wins with no error. The gate is therefore the only thing that keeps new-versus-duplicate counts exact and stops ledger updates from being lost. It also keeps Tokio workers free for the MCP transport and the viewer channel.`
    *   Location: After line 357, add a bullet: `*   **Cancellation.** The gate is taken with `Arc<Mutex>::lock_owned()`, and the owned guard is moved into the `spawn_blocking` closure. It is released only when the store work ends, even if the MCP request is cancelled.`
    *   Location: Line 580, file 3: append "Gate and cancellation tests run on a temporary RocksDB store; the in-memory store serializes writers and cannot detect a missing gate."

### B3 [`egm-zoom-out`, `egm-design`, `egm-codebase-details`, `egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 215-216](../../sparql-mcp-batch-ingestion.md#L215-L216), [lines 355-357](../../sparql-mcp-batch-ingestion.md#L355-L357), [lines 433-434](../../sparql-mcp-batch-ingestion.md#L433-L434), [lines 565-650](../../sparql-mcp-batch-ingestion.md#L565-L650).
*   **Discrepancy Description:** Several writers are outside the scope:
    *   `cbm_load_graph`, which takes an arbitrary `graph_iri`, including protected graphs;
    *   the recording imports;
    *   `project_create`;
    *   `mcp/importer.rs`;
    *   plugins that hold a raw `Arc<dyn SparqlStore>`;
    *   the CLI loaders.

    These bypass the gate, the limits and the guards. The `kb-workbench` skill uses `cbm_load_graph` for bulk loads, and it reads `triples_before`, `triples_after` and `delta`.
*   **Rationale:** The gate and the guards are invariants for the whole store. A gate in one tool's code cannot stop a plugin from writing.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Four lenses raised it, and the evidence is in `cbm.rs:87-146`, `recording.rs:191-229`, `plugin/mod.rs:16-21` and `SKILL.md:206`.
*   ***Proposed Edit:***
    *   Location: Replace lines 215-216.
    *   Change:
        ```
        Every store writer goes through the same `WriteGate` and protected-graph
        check. Both are enforced in a `GatedStore` decorator around the
        `SparqlStore` port, not per tool. Core code (ledger, registry, commit)
        uses a privileged handle. Plugins receive only the gated handle. Limits
        are applied at the tool boundary:

        | Writer | Gated | Limits | Protected-graph check |
        |---|---|---|---|
        | `update_sparql`, `load_ontology`, `load_ontology_file` | yes | `[ingest]` | yes |
        | `ingest_*` | yes | `[ingest]` | yes |
        | `cbm_load_graph` | yes | own cap, chunked transactions of `file_batch_triples` | yes (`graph_iri`) |
        | recording imports, `project_create`, `mcp/importer.rs` | yes | none (core-generated) | privileged |
        | `ToolPlugin`s | yes | plugin's own | yes |
        | CLI `load-file`, `code-import` | yes (same process) | none | yes |
        ```
    *   Location: Line 433-434. Replace "`kb-workbench` renders and ingests per note, which is far below 1 MiB." with "`kb-workbench` writes notes one at a time. Its code import goes through `cbm_load_graph`, which keeps its own cap and chunked transactions (see the writer table)."
    *   Location: Target Files. Add `application/tools/cbm.rs`, `recording.rs`, `project.rs`, `mcp/importer.rs` and `plugin/mod.rs` as Modify.

### B4 [`egm-design`, `egm-codebase-details`, `egm-implementation-readiness`] - Severity: Blocker
*   **Affected Scope:** [Lines 393-404](../../sparql-mcp-batch-ingestion.md#L393-L404) (Guards), [lines 583-587](../../sparql-mcp-batch-ingestion.md#L583-L587).
*   **Discrepancy Description:** A `DELETE` or `INSERT` with `GRAPH ?g` reaches protected graphs without any `DROP` or `CLEAR`. spargebra has no Add, Move or Copy variants.
*   **Rationale:** The rewriting is confirmed in `spargebra parser.rs:1284-1312`. Without this fix, CUJ 6's "always refused" claim is false.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. It is a security invariant that the design itself states.
*   ***Proposed Edit:***
    *   Location: After line 404, insert:
        ```
        *   **Variable graphs.** spargebra rewrites `ADD`, `COPY` and `MOVE`
            into `Drop` and `DeleteInsert`. The guard therefore classifies the
            rewritten operations:

            | Operation (after rewriting) | Graphs touched | Destructive | Refused unless `allow_destructive_updates` |
            |---|---|---|---|
            | `InsertData` / `DeleteData` | each quad's graph | no | protected graph → always refused |
            | `DeleteInsert`, templates with constant graphs | template graphs | only if a delete template is fully variable (`?s ?p ?o`) | as stated |
            | `DeleteInsert`, any template `GRAPH ?g` | **all graphs** | yes | yes; and always refused if a protected graph could match |
            | `Drop` / `Clear` (graph, `DEFAULT`, `NAMED`, `ALL`) | target | yes | yes |
            | `Create` | target | no | protected → refused |
            | `Load` (incl. `SILENT`) | target | n/a | always refused (`load_forbidden`) |

            The default graph counts as a project graph for this check.
        ```
    *   Location: Line 587. Change "Unit-tested against each `GraphUpdateOperation` variant." to "Unit-tested against each row of the table, including `ADD`, `COPY` and `MOVE` in their rewritten form, `WITH`, `GRAPH ?g`, and `VALUES`-bound graphs."

### B5 [`egm-implementation-readiness`, `egm-structural`, `egm-comprehension`, `egm-style`] - Severity: Blocker
*   **Affected Scope:** [Lines 59-63](../../sparql-mcp-batch-ingestion.md#L59-L63), [lines 251-351](../../sparql-mcp-batch-ingestion.md#L251-L351), [lines 409-411](../../sparql-mcp-batch-ingestion.md#L409-L411), [lines 614-617](../../sparql-mcp-batch-ingestion.md#L614-L617).
*   **Discrepancy Description:** There are no input or output schemas for the seven tools, and no complete list of error codes.
*   **Rationale:** File 8 has nothing to implement against. The sibling agent-control design has a "MCP tools (core)" section.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. It is the most-cited gap in the review.
*   ***Proposed Edit:***
    *   Location: New `### MCP tools` subsection after "Job model" (after line 249).
    *   Change: Insert the following skeleton. The author confirms the names and defaults.
        ```
        | Tool | Required args | Optional args (default) | Reply | Errors |
        |---|---|---|---|---|
        | `ingest_begin` | — | `target_graph` (active graph), `mode` (`staged`), `replace` (false), `format` (`ntriples`), `expected_batches`, `label`, `blank_nodes` (`skolemize` \| `reject`) | `{job_id, staging_graph?, limits{…}}` | `invalid_target`, `too_many_open_jobs`, `invalid_argument` |
        | `ingest_batch` | `job_id`, `seq`, `data` | — | as in step 7 | `job_not_found`, `job_not_open`, `limit_exceeded`, `parse_error`, `seq_conflict`, `blank_node_rejected`, `write_busy` |
        | `ingest_validate` | `job_id`, `checks[]` | — | `{passed, results:[{name, passed, rows[≤20]}]}` | `not_a_query`, `validate_timeout`, `job_not_open`, `wrong_mode` |
        | `ingest_commit` | `job_id` | `require_validation` (false) | `{totals, target_graph, target_triples, new_subjects[≤N], new_subject_count}` | `missing_batches`, `validation_required`, `validation_stale`, `empty_replace`, `write_busy` |
        | `ingest_abort` | `job_id` | — | `{state:"aborted", direct_data_kept: bool}` | `job_not_found`, `job_not_open` |
        | `ingest_status` | — | `job_id` | one job, or a list of open jobs: `{state, received_seqs (ranges), triples, file{bytes_read, bytes_total, tps, eta_s}?, validation?}` | `job_not_found` |
        | `ingest_file` | `path` | `target_graph`, `mode` (`direct`), `format`, `resume_job`, `force` (false) | `{job_id}` | `file_roots_unset`, `path_outside_roots`, `file_too_large`, `source_changed`, `staged_too_large` |

        **Error payload.** Every refusal is `isError: true` with
        `{"code", "message", "limit_key"?, "limit"?, "value"?, "line"?, "column"?, "job_id"?, "holder"?}`.
        `limit_key` names the `[ingest]` key; `limit` and `value` are numbers.
        ```
    *   Location: Lines 409-411. Replace them with a pointer to that table.

### B6 [`egm-implementation-readiness`, `egm-codebase-details`, `egm-comprehension`, `egm-style`] - Severity: Blocker
*   **Affected Scope:** [Lines 209](../../sparql-mcp-batch-ingestion.md#L209), [lines 235-241](../../sparql-mcp-batch-ingestion.md#L235-L241), [lines 589-597](../../sparql-mcp-batch-ingestion.md#L589-L597).
*   **Discrepancy Description:** The ledger vocabulary is undefined. The port types are left as a choice for the implementer. `write_batch(quads, extra_quads)` cannot replace totals. It is not stated which layer parses.
*   **Rationale:** The resume contract (CUJ 4) depends on the ledger format.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. B1's fix (derived totals) also removes the need to replace totals.
*   ***Proposed Edit:***
    *   Location: After line 241, insert a "Job ledger schema" table (prefix `smj: <urn:sparql-mcp:jobs#>`):
        ```
        | Subject | Predicate | Object | Notes |
        |---|---|---|---|
        | `urn:sparql-mcp:job:{id}` | `smj:state` | IRI `smj:open`… | replaced in the same transaction as each transition |
        | job | `smj:target`, `smj:mode`, `smj:replace`, `smj:format`, `smj:expectedBatches`, `smj:label`, `smj:created`, `smj:lastActivity` | typed literals (`xsd:dateTime`, `xsd:long`, `xsd:boolean`) | |
        | `urn:sparql-mcp:job:{id}:batch:{seq}` | `smj:seq`, `smj:sha256` (hex), `smj:parsed`, `smj:insertedNew`, `smj:alreadyPresent`, `smj:accepted` | literals | insert-only; totals are SUMs over these |
        | job | `smj:validation` → `{passed, afterSeqCount}` | | for `require_validation` |
        | job (file) | `smj:fileSha256`, `smj:fileBytes`, `smj:checkpointBytes`, `smj:checkpointTriples` | | see *File jobs* |
        ```
    *   Location: Lines 591-597. Replace them with:
        ```
        *   `StoredQuad { s, p, o, g: Option<String> }` replaces `[String; 3]` on every path.
        *   New methods:
            *   `write_batch(&self, quads: &[StoredQuad], ledger: &[StoredQuad], guard: BatchGuard) -> Result<BatchWrite { inserted_new, already_present }, StoreError>`.
                `BatchGuard` carries the re-checks from B1, which run inside the transaction.
            *   `parse_batch(&self, bytes: &[u8], format, limits: &Limits, bnode: BnodePolicy) -> Result<Vec<StoredQuad>, ParseError>`.
                Parsing and term checks live in the adapter. The skolemization policy is passed in.
            *   `commit_job(&self, update: &str, ledger_ops: LedgerOps) -> Result<(), StoreError>`.
                Runs as one `Transaction`, using `Transaction::update` and then the ledger ops.
            *   `graph_size(&self, graph: &str) -> Result<u64, StoreError>`: a `COUNT` scoped to that graph.
            *   `stream_file(&self, file: File, opts, batch: usize, on_batch: impl FnMut(FileBatch) -> Result<(), StoreError>) -> Result<FileSummary, StoreError>`.
        *   `len()` is removed from the ground paths (see *ground versus pattern updates*).
        ```
    *   Location: Line 209. Use the same names.

### B7 [`egm-implementation-readiness`, `egm-design`, `egm-comprehension`, `egm-structural`, `egm-style`] - Severity: Blocker
*   **Affected Scope:** [Lines 218-231](../../sparql-mcp-batch-ingestion.md#L218-L231), [lines 285-295](../../sparql-mcp-batch-ingestion.md#L285-L295), [line 338](../../sparql-mcp-batch-ingestion.md#L338).
*   **Discrepancy Description:** The diagram is malformed and missing states. There is no recovery rule for `committing`. A retried replace commit can run `MOVE <empty> TO <target>` and wipe the target.
*   **Rationale:** spargebra rewrites `MOVE` with `DROP SILENT target` first. `Transaction::update` lets the commit and the manifest change share one commit.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid, and it causes data loss on a reachable path.
*   ***Proposed Edit:***
    *   Location: Replace the diagram at lines 220-231 with:
        ```
        | From | Event | To | Side effects |
        |---|---|---|---|
        | — | `ingest_begin` | `open` | manifest created |
        | — | `ingest_file` | `running` | manifest created, background task started |
        | `open` | `ingest_batch` accepted or refused | `open` | batch record on accept only |
        | `open` | `ingest_commit` refused (`missing_batches`, `validation_*`, `empty_replace`) | `open` | none |
        | `open` | `ingest_commit` | `committed` | one transaction: commit update + state change |
        | `open`, `interrupted` | `ingest_abort` | `aborted` | staging dropped in chunks; direct data kept |
        | `open`, `interrupted` | idle > `job_idle_ttl` | `expired` | as abort |
        | `running` | EOF | `committed` (direct) or `open` (staged) | totals recorded |
        | `running` | parse or limit error | `failed` | direct batches kept |
        | `running` | core restart | `interrupted` | — |
        | `interrupted` | `ingest_file resume_job` | `running` | — |
        ```
        Add: "`committing` is never persisted. The commit and the state change run as one Oxigraph transaction (`Store::start_transaction`, `Transaction::update` for `ADD`/`MOVE` + `DROP SILENT`, the manifest quads, then `commit`), so a crash leaves the job `open` or `committed`, never in between."
    *   Location: Line 292-293. Append: "A replace commit with an empty staging graph is refused with `empty_replace`, unless `allow_empty_replace: true` is passed."

### B8 [`egm-implementation-readiness`, `egm-design`, `egm-comprehension`, `egm-structural`, `egm-codebase-details`] - Severity: Blocker
*   **Affected Scope:** [Lines 296-301](../../sparql-mcp-batch-ingestion.md#L296-L301), [lines 421-427](../../sparql-mcp-batch-ingestion.md#L421-L427), [line 559](../../sparql-mcp-batch-ingestion.md#L559).
*   **Discrepancy Description:** The `Changed` event has `u64` `triples_before` and `triples_after` fields, which the viewer reads. The design emits `Changed` without saying what goes into them. Setting the output fields to `null` breaks tests that the Phase 0 exit says must pass. There is no counterpart for a negative `delta`.
*   **Rationale:** The wire format is owned by `interface-agentic-rust-stylee.md`, so changing it is a change to another design.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Evidence: `event_sink.rs:89-93`, `sparql-mcp-viewer/src/ingest.rs:90-95`, `tests/tools_sparql.rs:122-123,145-146`, `tests/store.rs:225-231`.
*   ***Proposed Edit:***
    *   Location: After line 301, add: "`Changed.triples_before` and `triples_after` are filled with `graph_size(target)` before and after the commit. Each is one `COUNT` on the target graph only, and is computed only on the `Changed` path. The wire format does not change."
    *   Location: Lines 423-427. Replace them with:
        ```
        *   **Tool outputs.** For ground writes, `triples_before` and `triples_after`
            become `null` (deprecated for one release). `delta` is kept and equals
            `inserted_new - removed_existing`. `inserted_new` and `removed_existing`
            are new fields, both exact and cheap. Pattern updates keep real
            before and after values (they still call `len()`). `dry_run` returns
            `{"dry_run": true, "operations": ["insert_data", "drop", …], "graphs": [...], "destructive": bool}`.
            Tests updated: `tests/tools_sparql.rs` (lines 122-123, 145-146), `tests/store.rs` (225-231).
            **This is a visible change** for any skill that reads `triples_after`.
        ```
    *   Location: Line 559. Change "Existing tests pass." to "Existing tests pass, except the ones listed under *Compatibility*, which are updated."

### M1 [`egm-codebase-details`, `egm-inventive-catalyst`] - Severity: Major
*   **Affected Scope:** [Lines 471-479](../../sparql-mcp-batch-ingestion.md#L471-L479).
*   **Discrepancy Description:** `BulkLoader` is atomic by default in 0.5.7.
*   **Rationale:** `store.rs:1775-1781` (`without_atomicity` is opt-in) and `rocksdb.rs:1459-1478`. The synthesizer verified this.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** The fact is wrong. The conclusion for small batches still holds, for other reasons.
*   ***Proposed Edit:***
    *   Location: Lines 474-477.
    *   Change:
        *   Old: `*   **Cons:** It is non-transactional by design, so a failure leaves a partial write, and a ledger record cannot share an atomic write with the data. It is built for tens of millions of triples into a mostly idle store.`
        *   New: `*   **Cons:** In Oxigraph 0.5.7 it commits atomically (unless `without_atomicity()` is set), but it can only insert. It gives no read-your-writes or `contains`, so there are no new-versus-duplicate counts and no in-transaction idempotency checks. Each call starts worker threads and writes SST files, which a crash leaves behind. That cost is far too high for 5k-triple batches.`
    *   Location: Line 478. Append: "It is also a candidate for staged commits above a size threshold, and for direct file jobs."
    *   Also (author action, outside the target): correct `context/ground-truth.md` line 16.

### M2 [`egm-design`, `egm-codebase-details`, `egm-inventive-catalyst`, `egm-structural`] - Severity: Major
*   **Affected Scope:** [Lines 287-291](../../sparql-mcp-batch-ingestion.md#L287-L291), [line 377](../../sparql-mcp-batch-ingestion.md#L377), [lines 518-519](../../sparql-mcp-batch-ingestion.md#L518-L519).
*   **Discrepancy Description:** No figure is given for the commit's memory or for how long it holds the gate. `DROP GRAPH` on an empty staging graph errors.
*   **Rationale:** `update.rs:325-333`, `update.rs:404-416`, `store.rs:552-553`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The 500k default has not been measured.
*   ***Proposed Edit:***
    *   Location: Lines 287-291. Change `DROP GRAPH <staging>` to `DROP SILENT GRAPH <staging>`, and append: "The commit transaction holds about 12 index operations per triple in memory (6 for the copy, 6 for the drop). The default `max_job_triples_staged` is provisional. The Phase 1 bench measures peak RSS and gate hold time of a staged commit at 100k, 250k and 500k triples on the Windows reference machine, and the default is set so a commit holds the gate for less than `write_wait_timeout`."
    *   Location: Line 560, Phase 1 exit. Add "staged-commit RSS and duration recorded; default confirmed or lowered".

### M3 [`egm-inventive-catalyst`, `egm-design`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 76-77](../../sparql-mcp-batch-ingestion.md#L76-L77), [lines 292-293](../../sparql-mcp-batch-ingestion.md#L292-L293), [lines 403-404](../../sparql-mcp-batch-ingestion.md#L403-L404).
*   **Discrepancy Description:** A replace commit costs memory in proportion to the target graph, discards concurrent writes, and implicitly bypasses the destructive-update guard.
*   **Rationale:** `MOVE` is rewritten as Drop(target), then DeleteInsert, then Drop(source), all in one transaction.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The minimal fix is to document the behaviour and add a bound. The pointer-swap alternative is noted but not required.
*   ***Proposed Edit:***
    *   Location: Lines 292-293. Replace them with: "**Staged, replace.** `MOVE SILENT <staging> TO <target>` in one transaction. Its memory grows with the target size plus twice the staging size, so `ingest_begin` with `replace: true` is refused when `graph_size(target) + max_job_triples_staged` exceeds `max_replace_triples` (new key). Writes made to the target after `ingest_begin` are discarded by the replace; the reply reports `target_writes_discarded: true` when the target changed during the job. The commit runs through the privileged store handle, so the destructive-update guard does not apply to it."
    *   Location: Limits table. Add a `max_replace_triples` row with default 1,000,000 (provisional, confirmed by the bench).

### M4 [`egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 23-26](../../sparql-mcp-batch-ingestion.md#L23-L26), [lines 462-505](../../sparql-mcp-batch-ingestion.md#L462-L505), [lines 557-563](../../sparql-mcp-batch-ingestion.md#L557-L563).
*   **Discrepancy Description:** F1-F9 hardening is tied to a 7-tool job platform. The minimal alternative (hardening plus an idempotent direct `ingest_batch` and `ingest_status`) is never weighed. Staged mode, replace, the sweeper, `ingest_validate` and `ingest_file` with resume carry most of the Blockers.
*   **Rationale:** The author's stated intent is safe, efficient, bounded batch inserts. Most of B1, B7, M2, M3, M8, M15 and M23 lives in the optional features.
*   ***AI Suggestion:*** ESCALATE
*   ***Justification:*** The finding is about scope and product direction, so the author has to decide. The evidence supports it, but staged atomic commit and validation may be features the author explicitly wants.
*   ***Proposed Edit:*** These options are for the author to choose from:
    *   Option 1 (recommended): split the document into `sparql-mcp-write-path-hardening.md` (Phase 0, B2-B4, B8, M7, M9, M10) and this design (jobs), and ship hardening first.
    *   Option 2: keep one document but add "Alternative F: hardening + idempotent direct `ingest_batch`/`ingest_status` only", with an explicit reason for adopting each heavier feature.
    *   Option 3: keep the scope as it is, and accept the larger Phase 1-3 risk.

### M5 [`egm-codebase-details`, `egm-design`, `egm-inventive-catalyst`, `egm-implementation-readiness`, `egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Lines 39-42](../../sparql-mcp-batch-ingestion.md#L39-L42), [lines 78-79](../../sparql-mcp-batch-ingestion.md#L78-L79), [lines 264-266](../../sparql-mcp-batch-ingestion.md#L264-L266).
*   **Discrepancy Description:**
    *   The existing tools merge `_:b0` across separate calls.
    *   Turtle anonymous nodes get random ids.
    *   Per-job scope duplicates nodes when a source is re-ingested.
    *   Skolem IRIs can be forged.
*   **Rationale:** `oxrdfio parser.rs:109`, `oxttl terse.rs:268,304,321,737,762,780`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The Problem statement currently describes the opposite failure mode from the one that actually happens.
*   ***Proposed Edit:***
    *   Location: Lines 40-42. Replace "retrying may duplicate blank-node data." with "two calls that reuse a blank-node label such as `_:b0` are silently merged into one node, and Turtle anonymous nodes (`[]`) are duplicated on retry."
    *   Location: F table. Add a row `| F10 | `load_rdf_impl` parses without `rename_blank_nodes`, so labels from separate calls merge | `oxigraph.rs::load_rdf_impl` | Silent data corruption | Per-call `rename_blank_nodes()` for `load_ontology` and `load_ontology_file` |`.
    *   Location: Lines 264-266. Replace them with: "skolemize blank nodes. A labelled `_:x` becomes `<urn:sparql-mcp:genid:{job_id}:x>`, with the label percent-encoded. An anonymous node (`[]`, collection) becomes `<urn:sparql-mcp:genid:{job_id}:{seq}:a{n}>`, where n is the node's order of first appearance in the batch. Blank nodes can be rejected instead with `blank_nodes: reject`. Payload IRIs starting with `urn:sparql-mcp:` are refused (`reserved_iri`)."
    *   Location: Lines 78-79. Change the Goal to: "**Stable blank nodes.** Labelled blank nodes are skolemized per job, so `_:a` means the same node across batches and retries of one job."

### M6 [`egm-design`, `egm-inventive-catalyst`, `egm-comprehension`, `egm-style`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 67-69](../../sparql-mcp-batch-ingestion.md#L67-L69), [lines 243-246](../../sparql-mcp-batch-ingestion.md#L243-L246), [lines 448-452](../../sparql-mcp-batch-ingestion.md#L448-L452).
*   **Discrepancy Description:**
    *   The rule "only an accepted batch creates a record" is never stated.
    *   Workflow step 3 contradicts step 4.
    *   Split remainders fall outside `expected_batches`.
    *   Hashing the raw bytes is brittle.
*   **Rationale:** Several lenses found the same contradictions.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The fixes below are wording changes plus one rule for `expected_batches`. The canonical hash is offered as optional.
*   ***Proposed Edit:***
    *   Location: After line 69, add: "Only an accepted batch creates a record. A refused seq (`parse_error`, `limit_exceeded`) can be re-sent with new content. A duplicate reply replays the stored `parsed`, `inserted_new` and `already_present` values; the `job` object shows current totals."
    *   Location: Lines 448-449. Change to: "On a timeout, transport error or `write_busy`, re-send **the same seq** with the same content. A batch that already landed returns `duplicate: true`."
    *   Location: Lines 244-246. Change to: "If `expected_batches` was given, commit requires every seq from 1 to `max(expected_batches, highest received seq)` with no gaps. Otherwise it returns `missing_batches` with the list. An accepted batch cannot be replaced. To correct one, abort the job."
    *   Optional, for the author: hash a canonical form (sorted N-Triples after skolemization) instead of the raw payload.

### M7 [`egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Line 186](../../sparql-mcp-batch-ingestion.md#L186), [lines 413-419](../../sparql-mcp-batch-ingestion.md#L413-L419).
*   **Discrepancy Description:** Data written to the default graph is tagged with the active project graph. No event value is defined for the default graph.
*   **Rationale:** `ontology.rs:35,82-89,185-188`, `oxigraph.rs:247-254`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. This is the common case, not an edge case.
*   ***Proposed Edit:***
    *   Location: Line 186, Finding cell. Append: "This also happens when `graph_iri` is omitted, or for `INSERT DATA` without `GRAPH`: the data lands in the default graph."
    *   Location: After line 419, add: "Events for the default graph use `graph = \"\"` (empty string), which the viewer never shows. `load_ontology` without `graph_iri` now loads into the active graph, the same as the CLI `load-file`. This is listed under *Compatibility*."

### M8 [`egm-design`, `egm-codebase-details`, `egm-implementation-readiness`, `egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Lines 333-341](../../sparql-mcp-batch-ingestion.md#L333-L341), [lines 349-351](../../sparql-mcp-batch-ingestion.md#L349-L351).
*   **Discrepancy Description:**
    *   The file hash is computed while reading, so it cannot be used to skip a file before the load starts.
    *   The prefix-hash checkpoint is ill-defined because the reader buffers ahead.
    *   The ledger is keyed by hash only, and it is written outside the data transaction.
*   **Rationale:** `oxigraph.rs:143-144`, `oxigraph.rs:267-277`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 333-341. Replace them with:
        ```
        *   **Begin.** The core opens the file and hashes all of it (SHA-256) in a
            pre-pass, then records the hash and size in the manifest. It reads
            through `take(max_file_bytes)`.
        *   **Background run.** The core parses from a `BufReader`. For N-Triples
            and N-Quads, it cuts batches at line boundaries every
            `file_batch_triples` triples. Each batch transaction also writes the
            checkpoint (`checkpointBytes`, the exact byte offset after the last
            line, and `checkpointTriples`).
        *   **Resume.** On restart, a `running` job becomes `interrupted`.
            `ingest_file` with `resume_job` re-hashes the whole file. If the hash
            differs, it fails with `source_changed`. Otherwise it seeks to
            `checkpointBytes` and continues. A Turtle file job restarts from the
            beginning; re-inserting its quads is idempotent.
        ```
    *   Location: Lines 349-351. Replace them with: "**Idempotence.** The load ledger is keyed by (file SHA-256, target graph) and is written in the same transaction as the last batch. A replace commit or an allowed `DROP`/`CLEAR` of a graph removes that graph's ledger entries in the same transaction. A completed (hash, graph) pair is skipped unless `force: true`. `load_ontology_file` gets the same fix in Phase 0."

### M9 [`egm-design`, `egm-implementation-readiness`, `egm-codebase-details`, `egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Line 190](../../sparql-mcp-batch-ingestion.md#L190), [lines 330-332](../../sparql-mcp-batch-ingestion.md#L330-L332), [lines 428-431](../../sparql-mcp-batch-ingestion.md#L428-L431).
*   **Discrepancy Description:**
    *   On native Windows, path checks face `\\?\` prefixes, TOCTOU, UNC/NTLM leaks, ADS and device paths.
    *   The config format is unspecified.
    *   The symlink test needs elevated rights.
    *   With `file_roots` unset, F8 stays unfixed.
    *   `export_graph` and the `cbm` DB path are not covered.
*   **Rationale:** The ground truth says the environment is native Windows. The existing `read_resource` already canonicalizes both sides (`server.rs:227-234`).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid, and it is security-relevant for this environment.
*   ***Proposed Edit:***
    *   Location: Lines 330-332. Replace them with:
        ```
        *   **Path checks (Windows-first).** `file_roots` is a `Vec<PathBuf>`.
            Each root is canonicalized at startup; a missing root is a startup
            error. Example: `file_roots = ['C:\data\rdf', 'D:/dumps']` (TOML
            literal strings or forward slashes). The core rejects UNC
            (`\\host\…`), device (`\\.\…`) and alternate-data-stream (`:`
            after the drive) paths before opening anything. It then opens the
            file, gets the final path from the open handle
            (`GetFinalPathNameByHandleW`), and checks it against the roots
            component-wise and case-insensitively. Reading uses the same handle.
            The symlink-escape test uses a directory junction, which needs no
            elevation.
        ```
    *   Location: Line 190, Fix cell. Append: "; `export_graph` output and the `cbm_load_graph` `cbm_db_path` are also checked against `file_roots`. With `file_roots` unset, F8 is only a warning in this release, and the next release enforces it by defaulting to the workspace directory."

### M10 [`egm-design`, `egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Line 184](../../sparql-mcp-batch-ingestion.md#L184), [line 381](../../sparql-mcp-batch-ingestion.md#L381), [lines 599-602](../../sparql-mcp-batch-ingestion.md#L599-L602).
*   **Discrepancy Description:** With the F2 fix, a 2 GiB file is still loaded in memory as one transaction.
*   **Rationale:** `store.rs:552-553,763`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The current plan contradicts the stated intent of bounded memory.
*   ***Proposed Edit:***
    *   Location: Limits table. Add `| `max_load_file_bytes` | 64 MiB | `load_ontology_file` | One transaction; larger files use `ingest_file` |`. Change the `max_file_bytes` "Applies to" cell to `ingest_file`.
    *   Location: Line 184, Fix cell. Append: "`load_ontology_file` is capped by `max_load_file_bytes` and reads through a `BufReader`, not `fs::read`."

### M11 [`egm-design`] - Severity: Major
*   **Affected Scope:** [Lines 276-277](../../sparql-mcp-batch-ingestion.md#L276-L277), [lines 384-385](../../sparql-mcp-batch-ingestion.md#L384-L385).
*   **Discrepancy Description:** Applying the event threshold per call lets a direct or file job emit thousands of events.
*   **Rationale:** The broadcast has 1024 slots, and the viewer resyncs when it lags (`sparql-mcp-viewer.md:213-216`).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. Only one lens raised it, but the arithmetic is simple and correct.
*   ***Proposed Edit:***
    *   Location: Lines 276-277. Replace them with: "**Events (direct mode only).** While the job's total stays at or below `event_changed_threshold`, emit `Insert` events split into chunks of `event_max_triples`. Above it, emit at most one `Changed` event every 5 seconds, plus one at the end of the job. File jobs always use the `Changed` path."

### M12 [`egm-comprehension`, `egm-structural`, `egm-implementation-readiness`, `egm-design`] - Severity: Major
*   **Affected Scope:** [Lines 123-124](../../sparql-mcp-batch-ingestion.md#L123-L124).
*   **Discrepancy Description:** No `job commit` event kind exists. 60k triples is above the 50k threshold.
*   **Rationale:** Future Improvements (lines 547-548) lists job events as future work.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Old: `*   Commit makes all 60k triples visible at once. The viewer feed shows one `job commit` entry and resyncs.`
    *   New: `*   Commit makes all 60k triples visible at once. 60k is above `event_changed_threshold`, so the viewer feed shows one `changed` entry and the viewer resyncs.`

### M13 [`egm-style`, `egm-comprehension`, `egm-structural`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Lines 86-88](../../sparql-mcp-batch-ingestion.md#L86-L88), [line 164](../../sparql-mcp-batch-ingestion.md#L164), [lines 511-516](../../sparql-mcp-batch-ingestion.md#L511-L516), [lines 560-562](../../sparql-mcp-batch-ingestion.md#L560-L562).
*   **Discrepancy Description:** The 50k triples/s target is attached to a different path in different sections and assigned to the wrong phase. "Responsive" and "flat" have no thresholds.
*   **Rationale:** An exit criterion that cannot be tested cannot gate a phase.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 86-88. Change to: "**Fast.** Writes are batched transactions with no whole-store scans, run off the async runtime. Targets, recorded on the Windows reference machine (CPU, disk and RAM listed in the PR): a 5,000-triple batch in 300 ms or less (Phase 1); file jobs at 50,000 triples/s or more (Phase 3)."
    *   Location: Line 164. Change to "`query_sparql` p95 latency stays under 200 ms during the load."
    *   Location: Line 562. Change to "CUJ 5 on a generated 1 GiB file: throughput ≥ 50,000 triples/s; peak working set (`GetProcessMemoryInfo`) below 512 MiB regardless of file size; `query_sparql` p95 < 200 ms; resume after kill."

### M14 [`egm-structural`, `egm-comprehension`, `egm-implementation-readiness`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Lines 557-563](../../sparql-mcp-batch-ingestion.md#L557-L563), [lines 652-659](../../sparql-mcp-batch-ingestion.md#L652-L659).
*   **Discrepancy Description:**
    *   The Phase 1 exit includes CUJ 1, which needs Phase 2 validation and the viewer.
    *   CUJ 4 cannot run on an in-memory store.
    *   F3, F6, F8 and F9 have no stated test assertion.
*   **Rationale:** Traceability.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 560, Phase 1 exit. Change to: "CUJs 2-3 in memory; CUJ 4 and the gate tests (B1, B2) on a temporary RocksDB store; CUJ 1 without validation or viewer steps; duplicate seq is a no-op; conflicting seq is refused."
    *   Location: Line 561, Phase 2 exit. Add "CUJ 1 end to end, without the viewer".
    *   Location: Line 559, Phase 0 exit. Add: "F3: a counting `SparqlStore` test double records zero `len()` calls on ground writes; F6: `dry_run` on invalid SPARQL returns a parse error; F8: a path outside `file_roots` is refused; F9: a slow store call does not block a concurrent `ping`; CUJ 6: `DROP GRAPH <urn:project:P>` and any write to `urn:meta` are refused."
    *   Location: After the phase table, add: "Each phase starts only when the previous phase's exit criterion passes."

### M15 [`egm-design`, `egm-inventive-catalyst`] - Severity: Major
*   **Affected Scope:** [Lines 247-249](../../sparql-mcp-batch-ingestion.md#L247-L249), [line 380](../../sparql-mcp-batch-ingestion.md#L380).
*   **Discrepancy Description:** The sweeper runs at core start, so a restart more than an hour later expires the job before the agent can resume it. Drops run as one large transaction.
*   **Rationale:** This contradicts CUJ 4 and the Resumable goal.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 247-249. Replace them with: "**Expiry.** `smj:lastActivity` is updated by every job call. Idle time only counts while a core is running: on start, the core moves `lastActivity` forward by the downtime. A sweeper runs every 5 minutes and expires open or interrupted jobs that have been idle longer than `job_idle_ttl`. Staging graphs (from expiry, abort or commit) are dropped in transactions of `file_batch_triples` triples, and the gate is released between them. Direct-mode data already written stays, and the manifest records that."

### M16 [`egm-comprehension`, `egm-design`] - Severity: Major
*   **Affected Scope:** [Lines 522-526](../../sparql-mcp-batch-ingestion.md#L522-L526), [lines 376-379](../../sparql-mcp-batch-ingestion.md#L376-L379).
*   **Discrepancy Description:** Abandoned direct jobs leave partial data, which contradicts "never half-committed". The jobs graph grows without bound.
*   **Rationale:** The NFR must match the guarantees each mode actually gives.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 524-526. Change to: "*   A staged commit is all-or-nothing. A direct job is atomic per batch only: an aborted or expired direct job leaves its accepted batches in the target graph, and its manifest lists their seqs. *   A crash at any point leaves a staged job resumable or expirable, never half-committed."
    *   Location: After line 249, add: "**Retention.** The manifest and batch records of a committed, aborted or expired job are deleted 7 days after the job ends (`job_retention`)."

### M17 [`egm-style`, `egm-inventive-catalyst`, `egm-design`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Line 86](../../sparql-mcp-batch-ingestion.md#L86), [line 185](../../sparql-mcp-batch-ingestion.md#L185), [lines 271-275](../../sparql-mcp-batch-ingestion.md#L271-L275), [lines 302-305](../../sparql-mcp-batch-ingestion.md#L302-L305), [lines 514-515](../../sparql-mcp-batch-ingestion.md#L514-L515).
*   **Discrepancy Description:** The claim that writes do no scans is broader than the design: pattern updates, `with_delta`, `stats` and the commit-time `COUNT` all still scan. Staged counts measure staging, not the target.
*   **Rationale:** The F3 verification found these extra `len()` sites.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 514-515. Change to: "Ground writes do no whole-store scan, so their cost does not grow with store size. Pattern updates still call `len()`. The commit reply runs one `COUNT` on the target graph, which costs O(target graph)."
    *   Location: Line 185, Where cell. Add `sparql.rs::with_delta`, `project_create`, and `export.rs::stats`.
    *   Location: Step 7 reply. In staged mode, name the counts `new_to_staging` and `already_staged`. The commit reply reports `new_to_target`, computed in the commit transaction.

### M18 [`egm-style`, `egm-comprehension`, `egm-structural`] - Severity: Major
*   **Affected Scope:** Lines 19, 54, 66, 189, 204, 209, 270, 291, 335, 340, 342-343, 372, 376, 504, 523.
*   **Discrepancy Description:** "Ledger" has two meanings, "triples" and "quads" are mixed, and `max_job_triples` is cited although no such key exists.
*   **Rationale:** Agents read the limit keys from `ingest_begin`, so the names must match exactly.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: After line 237, add: "**Terms.** *Job ledger*: the manifest and batch records in `urn:sparql-mcp:jobs`. *Load ledger*: the existing `urn:hkb:meta:loaded-files`. Inputs, counts and limits are in *triples*. *Quads* appears only for store-level writes."
    *   Location: Lines 270, 291, 342-343. Replace `max_job_triples` with "`max_job_triples_staged` or `max_job_triples_direct`, by mode".
    *   Location: Line 335. "Every `file_batch_triples` quads" → "Every `file_batch_triples` triples". Line 340: "recorded quad count" → "recorded checkpoint". Line 372: "≤ 10k quads" → "≤ 10,000 triples".
    *   Location: Lines 66 and 204. "ledger record" → "batch record". Line 523: "data and ledger" → "data and batch record".

### M19 [`egm-structural`] - Severity: Major
*   **Affected Scope:** Whole document; insertion points after line 505 and before line 555.
*   **Discrepancy Description:** There is no Risks/Open Questions section and no list of the changes this design requires in companion designs.
*   **Rationale:** Both siblings have the second section. This design changes the `Changed` contract and adds `Changed` producers.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: New `## Risks and Open Questions` after Alternatives (after line 505):
        ```
        | Risk / question | Impact | Mitigation | Verified in |
        |---|---|---|---|
        | Staged-commit memory at the default limit | OOM, long gate hold | bench, lower the default (M2) | Phase 1 bench |
        | Concurrent writers outside the gate | lost updates, wrong counts | `GatedStore` decorator (B3) | Phase 0 RocksDB test |
        | Disk growth from staging graphs and ledger | disk use | chunked drops, retention (M15, M16) | Phase 1 |
        | Resolved: one update request = one transaction | — | `update.rs:99-121,186-222` | 2026-11-18 review |
        | Resolved: `Transaction::contains` | — | `store.rs:1279` | 2026-11-18 review |
        ```
    *   Location: New `### Required changes to the companion designs` before "Delivery phases":
        ```
        *   `interface-agentic-rust-stylee.md`: `Changed` is also emitted by
            staged and replace commits, and by direct jobs above the threshold.
            `triples_before` and `triples_after` come from per-graph `COUNT`.
            Events for the default graph use `graph = ""`.
        *   `sparql-mcp-viewer.md`: no feed-kind change (CUJ 1 shows `changed`).
        *   `sparql-mcp-viewer-agent-control.md`: the commit reply returns
            `new_subjects` (IRIs, at most 5,000) for `viewer_show`.
        ```

### M20 [`egm-implementation-readiness`, `egm-style`, `egm-structural`] - Severity: Major
*   **Affected Scope:** [Lines 367-391](../../sparql-mcp-batch-ingestion.md#L367-L391), [lines 432-434](../../sparql-mcp-batch-ingestion.md#L432-L434), [line 518](../../sparql-mcp-batch-ingestion.md#L518), [lines 567-570](../../sparql-mcp-batch-ingestion.md#L567-L570).
*   **Discrepancy Description:**
    *   Config field types and units are not defined.
    *   The term-size measurement is not defined.
    *   Four rationale cells are empty.
    *   Several numbers are asserted without measurement.
*   **Rationale:** An implementer cannot write `IngestConfig` from the current text.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 569-570, file 1. Replace it with: "`#[serde(default)] pub ingest: IngestConfig` (the section is optional; absent means all defaults). Sizes are integer bytes (`max_batch_bytes = 1048576`). Durations are integer seconds with a `_secs` suffix (`job_idle_ttl_secs = 3600`). `file_roots: Option<Vec<PathBuf>>`. Tests: absent section, partial section, `file_roots` with a Windows path."
    *   Location: Line 374-375. Add: "Measured as the UTF-8 byte length of the IRI or the literal's lexical form; datatype and language tags are checked as IRIs. `update_sparql` checks terms in `INSERT DATA`/`DELETE DATA` only."
    *   Location: Empty rationale cells. `max_file_bytes`: "Above CUJ 5's 1.2 GiB dump; bounds disk and run time." `file_batch_triples`: "Matches `max_batch_triples`." `validate_timeout`: "Keeps one bad check from running unbounded." `write_wait_timeout`: "Longer than one 10,000-triple transaction." `max_update_bytes`: "Same bound as `max_batch_bytes`."
    *   Location: Line 371 and line 518. Change "parse memory stays around 10× the input" to "parse memory is estimated at 10× the input, confirmed by `ingest_bench` in Phase 1".
    *   Location: After the table, add: "Fixed constants: sweeper interval 5 min; validation rows 20; quoted input in errors 200 bytes (truncated on a char boundary)."

### M21 [`egm-comprehension`] - Severity: Major
*   **Affected Scope:** [Line 185](../../sparql-mcp-batch-ingestion.md#L185), [line 559](../../sparql-mcp-batch-ingestion.md#L559), [line 597](../../sparql-mcp-batch-ingestion.md#L597), [lines 78-79](../../sparql-mcp-batch-ingestion.md#L78-L79), [line 271](../../sparql-mcp-batch-ingestion.md#L271).
*   **Discrepancy Description:** "Ground" and "pattern" updates, the write gate, and skolemization are used before they are defined.
*   **Rationale:** The F3 fix depends on the ground/pattern split, which is defined only in a sibling design.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Before the F table (after line 179), add: "*Ground update*: `INSERT DATA`, `DELETE DATA`, and RDF loads, where the exact quads are known before writing. *Pattern update*: `INSERT/DELETE … WHERE`, `CLEAR`, `DROP`, `ADD`, `MOVE`, `COPY` (as in `interface-agentic-rust-stylee.md`). *Write gate*: one process-wide async mutex that serializes all store writes (see *Write gate*)."
    *   Location: Line 78-79. Add "(blank nodes are replaced by job-scoped IRIs; see *Batch processing*)".

### M22 [`egm-implementation-readiness`, `egm-design`, `egm-codebase-details`] - Severity: Major
*   **Affected Scope:** [Line 191](../../sparql-mcp-batch-ingestion.md#L191), [lines 362-363](../../sparql-mcp-batch-ingestion.md#L362-L363), [lines 629-633](../../sparql-mcp-batch-ingestion.md#L629-L633).
*   **Discrepancy Description:**
    *   The async dispatch shape, the constructor change and the dispatch-test change are not specified.
    *   Reads and parsing still run on Tokio workers.
*   **Rationale:** `server.rs:99-144,347-365`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Line 191, Fix cell. Change to "Every store-touching tool runs in `spawn_blocking`. Writes also take the write gate. Parsing a batch runs in `spawn_blocking` behind a semaphore of 2."
    *   Location: Lines 631-633, file 11. Replace it with: "`SparqlMcpServer::new(store, config)` builds `Limits`, `WriteGate` and `IngestService` from `config.ingest`; tests use `new_for_test(store)`. `dispatch_core` becomes `async fn dispatch_core(&self, name, args) -> Option<Result<CallToolResult, McpError>>`. The test `every_declared_tool_has_a_dispatch_arm` becomes `#[tokio::test]` and covers the 7 new tools. The sweeper task is started in `main.rs`. The `instructions` text (`server.rs:158-162`) gains the rule: bulk data goes through `ingest_*`."

### M23 [`egm-implementation-readiness`, `egm-design`, `egm-zoom-out`] - Severity: Major
*   **Affected Scope:** [Lines 307-326](../../sparql-mcp-batch-ingestion.md#L307-L326), [line 561](../../sparql-mcp-batch-ingestion.md#L561).
*   **Discrepancy Description:** The timeout mechanism, the substitution form, the input caps, the row cap and the direct-mode behaviour are all unspecified. The tool may also duplicate `query_sparql`.
*   **Rationale:** Specifying it fully and cutting it are both reasonable. Which one depends on the M4 decision.
*   ***AI Suggestion:*** ESCALATE
*   ***Justification:*** The choice depends on M4. If the author keeps `ingest_validate`, use the edit below.
*   ***Proposed Edit (if kept):***
    *   Location: After line 326, add: "`{{staging}}` and `{{target}}` are replaced by `<iri>`, angle brackets included. At most 16 checks per call, each at most 16 KiB. Only `SELECT` and `ASK` are accepted; any other form, and any use of `SERVICE`, gets `not_a_query`. Each check runs with `SparqlEvaluator::with_cancellation_token`, cancelled by a timer after `validate_timeout`. Rows are streamed and reading stops after 20. In direct mode the tool returns `wrong_mode`. The manifest stores the number of accepted batches at validation time; commit with `require_validation` compares it to the current count."
    *   Alternative (if cut): delete the section and Phase 2. `ingest_begin` returns `staging_graph`, and the workflow step 5 tells the agent to run its checks with `query_sparql`.

### M24 [`egm-style`, `egm-design`, `egm-comprehension`, `egm-implementation-readiness`] - Severity: Major
*   **Affected Scope:** [Line 125](../../sparql-mcp-batch-ingestion.md#L125), [lines 302-305](../../sparql-mcp-batch-ingestion.md#L302-L305), [lines 455-456](../../sparql-mcp-batch-ingestion.md#L455-L456).
*   **Discrepancy Description:** The commit reply returns a count, but `viewer_show` needs IRIs. Three different names are used for the same set.
*   **Rationale:** `viewer_show` takes up to 5,000 IRIs (`sparql-mcp-viewer-agent-control.md:304`).
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:***
    *   Location: Lines 302-305. Replace them with: "**Reply:** totals, the target graph, the target graph's triple count (one `COUNT` on that graph only), `new_subject_count`, and `new_subjects`: up to 5,000 IRIs, sorted, of subjects that had no triple in the target before the commit. It is computed in the commit transaction with `Transaction::contains` on `(s, ?, ?, target)` before the copy. The agent can pass `new_subjects` to `viewer_show`."
    *   Location: Line 125 and line 455. Use "`new_subjects`".

### M25 [`egm-implementation-readiness`, `egm-codebase-details`, `egm-structural`] - Severity: Major
*   **Affected Scope:** [Lines 565-650](../../sparql-mcp-batch-ingestion.md#L565-L650).
*   **Discrepancy Description:** `ulid` and the tokio `time` feature are missing. Several module files and tests are not listed. Test paths have no crate prefix.
*   **Rationale:** `crates/sparql-mcp-core/Cargo.toml:21,36`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid and mechanical.
*   ***Proposed Edit:***
    *   Location: Before file 1, insert "#### 0. `crates/sparql-mcp-core/Cargo.toml` (Modify): add `ulid = \"1\"`; add `\"time\"` to the tokio features in `[dependencies]`. `sha2` and `spargebra` are already present."
    *   Add entries: `application/mod.rs`, `application/tools/mod.rs` and `domain/mod.rs` (Modify, to declare and re-export the new modules); `tests/tools_sparql.rs` and `tests/store.rs` (Modify, see B8); plus the writer files from B3.
    *   Location: File 12. Prefix the paths with `crates/sparql-mcp-core/tests/`. Location: file 11, `src/main.rs` → `crates/sparql-mcp-core/src/main.rs`.
    *   Location: Phase table. Add a "Files" column (Phase 0: 0, 1-6, 9-11; Phase 1: 7, 8, 12; Phase 2: 7, 8; Phase 3: 5-7; Phase 4: 14).

### N1 [`egm-style`, `egm-comprehension`, `egm-structural`] - Severity: Nit
*   **Affected Scope:** [Lines 288-291](../../sparql-mcp-batch-ingestion.md#L288-L291), [lines 273-275](../../sparql-mcp-batch-ingestion.md#L273-L275).
*   **Discrepancy Description:** Two Oxigraph assumptions are stated as fact with no source. Both are now verified.
*   **Rationale:** A citation makes the dependency auditable.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** The claims are true. Only the evidence is missing.
*   ***Proposed Edit:*** Line 289: "Oxigraph runs one update request as one transaction" → "Oxigraph 0.5.7 runs one update request as one transaction (`sparql/update.rs`, `on_store` and `execute`; verified 2026-11-18)". Line 274: append "(`Transaction::contains`, which sees the transaction's own writes)".

### N2 [`egm-comprehension`, `egm-structural`] - Severity: Nit
*   **Affected Scope:** [Line 301](../../sparql-mcp-batch-ingestion.md#L301).
*   **Discrepancy Description:** "base design CUJ 3" does not say which design.
*   **Rationale:** Two related designs have a CUJ 3.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** "(base design CUJ 3)" → "(`sparql-mcp-viewer.md`, CUJ 3: Recovering from a coarse update)".

### N3 [`egm-style`] - Severity: Nit
*   **Affected Scope:** Lines 138, 157, 371, 562, 657.
*   **Discrepancy Description:** Decimal units (GB, KB) are mixed with binary units (MiB, GiB).
*   **Rationale:** The limits use binary units, and so does the sibling design.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** "1.2 GB" → "1.2 GiB"; "1 GB" → "1 GiB" (lines 562 and 657); "a few hundred KB" → "a few hundred KiB".

### N4 [`egm-style`] - Severity: Nit
*   **Affected Scope:** Lines 39, 255, 336-340, 449-451, 654.
*   **Discrepancy Description:** Number formats, the word "record", and `seq` formatting are inconsistent. The verification command cannot be run as written.
*   **Rationale:** Consistency.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Line 654 → "`cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`." Write configured values as "10,000" (lines 161, 335). Line 255: "the record `(job, seq)`" → "the batch record for `(job_id, seq)`". Code-format `seq` at lines 68-69, 449 and 451.

### N5 [`egm-style`] - Severity: Nit
*   **Affected Scope:** Lines 13, 39-40, 233-234, 443, 446, 477.
*   **Discrepancy Description:** Several phrases are vague or uncalibrated.
*   **Rationale:** Precision.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Line 13: drop "increasingly". Lines 233-234 → "A ULID (48-bit time, 80 random bits). It sorts by time. Job ids are not a security boundary." Line 443 → "Use `direct` above `max_job_triples_staged`, or for append-only logs." Line 446: drop "about". Line 477: "a mostly idle store" → "a store with no concurrent writers".

### N6 [`egm-structural`, `egm-style`] - Severity: Nit
*   **Affected Scope:** Lines 176, 278-283, 438, 539, 555.
*   **Discrepancy Description:**
    *   Future Improvements is nested under the NFRs.
    *   The JSON fence breaks the numbered list.
    *   The heading case is mixed.
    *   Cross-reference styles vary.
*   **Rationale:** Structure.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid. The diagram itself is fixed by B7.
*   ***Proposed Edit:*** Promote "Future Improvements" (line 539) to `##`. Indent the JSON fence at lines 280-283 by four spaces under item 7. Rename the `##` headings to Title Case ("Current Write Path: Findings", "Agent Workflow (Guidance for Skills)"). Write every internal reference as an anchor link, for example `[Write gate](#write-gate)`.

### N7 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Line 142](../../sparql-mcp-batch-ingestion.md#L142), [lines 267-268](../../sparql-mcp-batch-ingestion.md#L267-L268).
*   **Discrepancy Description:** `parse_error` positions are 0-based and are sometimes missing.
*   **Rationale:** `oxrdfio error.rs:76-108,148`; `parser.rs:1174`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Lines 267-268 → "on a syntax error, return `parse_error` with `line` and `column` (1-based, when the parser provides them) and the message, and write nothing. A named-graph statement returns `named_graph_refused`."

### N8 [`egm-design`] - Severity: Nit
*   **Affected Scope:** [Lines 71-73](../../sparql-mcp-batch-ingestion.md#L71-L73), [lines 242-243](../../sparql-mcp-batch-ingestion.md#L242-L243).
*   **Discrepancy Description:** Staging is described as "private", but `list_graphs` and `GRAPH ?g` queries can see it.
*   **Rationale:** Only writes are guarded.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Line 72: "a private staging graph" → "a staging graph that is protected from writes and hidden from `list_graphs`, but readable by queries".

### N9 [`egm-design`] - Severity: Nit
*   **Affected Scope:** [Lines 258-261](../../sparql-mcp-batch-ingestion.md#L258-L261).
*   **Discrepancy Description:** The Turtle base IRI and the rule for repeating prefixes are not stated.
*   **Rationale:** Each batch is parsed independently.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Append to line 261: "Each Turtle batch is parsed on its own: it must repeat its `@prefix` lines, and relative IRIs are refused (no base IRI)."

### N10 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 405-408](../../sparql-mcp-batch-ingestion.md#L405-L408).
*   **Discrepancy Description:** The default active graph is not registered in `urn:meta`.
*   **Rationale:** `main.rs:35-36`; `project.rs:145-163`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Append: "The active graph is accepted even when it is not registered (for example `urn:project:default`)."

### N11 [`egm-inventive-catalyst`] - Severity: Nit
*   **Affected Scope:** [Lines 353-365](../../sparql-mcp-batch-ingestion.md#L353-L365).
*   **Discrepancy Description:** The reviewer proposes per-job and per-target locks instead of one global writer.
*   **Rationale:** A per-process local tool with at most 4 open jobs does not need fine-grained locks. The real cost is long critical sections, which M2 and M15 address.
*   ***AI Suggestion:*** DECLINE
*   ***Justification:*** The proposal adds lock-ordering rules and more test paths for little gain at this scale, and RocksDB serializes the final writes anyway.
*   ***Proposed Edit:*** None.

### N12 [`egm-zoom-out`] - Severity: Nit
*   **Affected Scope:** [Lines 279-283](../../sparql-mcp-batch-ingestion.md#L279-L283), [lines 572-627](../../sparql-mcp-batch-ingestion.md#L572-L627).
*   **Discrepancy Description:** The reviewer suggests moving helper names and payload examples out of the design.
*   **Rationale:** This conflicts with B5 and B6, where four lenses ask for more contract detail.
*   ***AI Suggestion:*** CONTEST
*   ***Justification:*** For an agent-facing tool API, the reply shapes and port signatures are the contract. They belong in the design.
*   ***Proposed Edit:*** None.

### N13 [`egm-codebase-details`] - Severity: Nit
*   **Affected Scope:** [Lines 599-605](../../sparql-mcp-batch-ingestion.md#L599-L605), [lines 619-622](../../sparql-mcp-batch-ingestion.md#L619-L622).
*   **Discrepancy Description:** Existing helpers that the plan could reuse are not named.
*   **Rationale:** Reuse.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** File 6: "Transactional `load_rdf_impl`: parse to a `Vec`, then `Store::extend`, or a `Transaction` with `contains` where counts are needed." File 9: add "`with_delta` (`sparql.rs:212`) loses its `len()` calls."

### N14 [`egm-implementation-readiness`, `egm-comprehension`] - Severity: Nit
*   **Affected Scope:** [Line 98](../../sparql-mcp-batch-ingestion.md#L98), [line 440](../../sparql-mcp-batch-ingestion.md#L440), [line 650](../../sparql-mcp-batch-ingestion.md#L650).
*   **Discrepancy Description:** The skill name is wrong, and the skills are not introduced.
*   **Rationale:** The skill lives at `plugins/kb-workbench/skills/sparql-first-code/SKILL.md`.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Line 440 → "This text goes into `plugins/kb-workbench/skills/sparql-first-code/SKILL.md` and the `kb-workbench` skill guidance." File 14: list these exact paths.

### N15 [`egm-implementation-readiness`] - Severity: Nit
*   **Affected Scope:** [Lines 533-537](../../sparql-mcp-batch-ingestion.md#L533-L537).
*   **Discrepancy Description:** The 200-byte quote rule has no helper. The `stats` field names are not given.
*   **Rationale:** Completeness.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Line 533 → "Error messages quote at most 200 bytes of input, cut on a char boundary by `limits::quote_input`." Line 537 → "`stats` gains `open_jobs` and `staging_triples` (summed from manifest totals, no `COUNT`)."

### N16 [`egm-style`] - Severity: Nit
*   **Affected Scope:** [Lines 484-487](../../sparql-mcp-batch-ingestion.md#L484-L487).
*   **Discrepancy Description:** The line wrap breaks early, and the same idea is stated three times.
*   **Rationale:** Style.
*   ***AI Suggestion:*** PROPOSE FIX
*   ***Justification:*** Valid.
*   ***Proposed Edit:*** Replace lines 484-487 with: "*   **Cons:** The agent needs write access inside `file_roots`, must produce the whole file before any parse or validation runs, and learns of a late error only after the full file is written."
