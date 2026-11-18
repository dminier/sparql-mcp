# sparql-mcp: Batch Ingestion over MCP

*   **Author:** dminier
*   **Status:** Draft (revised after EGM review round 1)
*   **Date:** 2026-11-18

--------------------------------------------------------------------------------

## Overview

Agents write to `sparql-mcp` through MCP tools such as `update_sparql` and
`load_ontology`. Those tools suit small writes. They are not built for batch
work: turning a source (a codebase, a document set, an export) into tens or
hundreds of thousands of triples, sent over many tool calls.

This document adds **ingestion jobs** to `sparql-mcp-core`. A job provides:

*   an id, and numbered batches;
*   atomic, retry-safe batches;
*   an optional staging graph, with validation before an atomic commit;
*   a job ledger in the store, so a job survives a core restart;
*   server-side streaming for large files.

This design builds on
[`sparql-mcp-write-path-hardening.md`](./sparql-mcp-write-path-hardening.md),
which ships first. That design provides the write gate, the gated store, the
limits, the guards, the per-graph and bounded events, and the definitions of
*ground* and *pattern* updates. They are not repeated here.

| Concern | Owned by |
|---|---|
| `StoreEvent` wire format | [`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md) |
| Write gate, limits, guards, event tagging | [`sparql-mcp-write-path-hardening.md`](./sparql-mcp-write-path-hardening.md) |
| Viewer ingestion feed | [`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md) |
| `viewer_show` after a commit | [`sparql-mcp-viewer-agent-control.md`](./sparql-mcp-viewer-agent-control.md) |
| Ingestion jobs (`ingest_*`) | this document |

### Terms

*   **Job ledger**: the manifest and batch records of every job, stored in the
    protected graph `urn:sparql-mcp:jobs`. This is not the *load ledger*
    (`urn:hkb:meta:loaded-files`) of the hardening design.
*   **Batch record**: one ledger entry per accepted batch.
*   **Staging graph**: `urn:sparql-mcp:staging:{job_id}`, used by staged jobs.
*   **Skolemization**: replacing blank nodes by job-scoped IRIs (see
    [Batch processing](#batch-processing-ingest_batch)).
*   Inputs, counts and limits are in triples.

## The Problem

*   **No batch semantics.** Ingesting 200k triples means chaining hundreds of
    `load_ontology` calls. Nothing ties them together. After a failure, the
    agent cannot tell which calls landed.
*   **No safe retry.** Re-sending a call after a timeout may write it twice.
    Two calls that reuse a blank-node label such as `_:b0` are merged into
    one node. Turtle anonymous nodes (`[]`) are duplicated on retry. The
    hardening design fixes the merge per call (F10), but nothing keeps a
    blank node stable *across* the calls of one logical load.
*   **No validation step.** Data goes straight into the project graph, where
    readers and the viewer see it at once. Nothing can be checked first:
    dangling references, missing labels, wrong types.
*   **No resumable large load.** `load_ontology_file` is one transaction,
    capped at `max_load_file_bytes`. A large dump has no path that streams,
    reports progress, and survives a restart.

## Goals

*   **Job-based batching.** `ingest_begin`, then `ingest_batch` for each
    batch, then optionally `ingest_validate`, then `ingest_commit` or
    `ingest_abort`. Also `ingest_status`, and `ingest_file` for server-side
    files.
*   **Atomic batches.** Each batch is parsed and checked completely. Its data
    and its batch record are then written in one transaction, which also
    re-checks the job state. A batch lands entirely, or not at all.
*   **Idempotent retries.** A batch is keyed by `(job_id, seq)`. Re-sending
    the same seq with the same content returns the original result. The same
    seq with different content is refused.
*   **Two commit modes.**
    *   `staged`, the default: batches land in the staging graph. The commit
        moves them to the target in one transaction.
    *   `direct`: each batch lands in the target at once. This mode is for
        large append-only loads.
*   **Optional replace.** `replace: true` makes a staged commit replace the
    target graph's contents.
*   **Stable blank nodes within a job.** Labelled blank nodes are skolemized
    per job, so `_:a` means the same node across the batches and retries of
    one job.
*   **Resumable.** Job state lives in the store. After a core restart,
    `ingest_status` lists the received seqs, and the agent re-sends the
    missing ones.
*   **Fast.** Targets are recorded on the Windows reference machine, with its
    CPU, disk and RAM listed in the PR:
    *   a 5,000-triple batch in 300 ms or less (Phase 1);
    *   file jobs at 50,000 triples/s or more (Phase 3).
*   **Visible in the viewer, at a bounded event rate.**
*   **Ready for `viewer_show`.** The commit reply returns `new_subjects`, a
    list of IRIs the agent can pass to `viewer_show`.

## Non-Goals

*   **Extraction and mapping.** Turning sources into RDF stays the job of the
    agent or of skills.
*   **Deletes inside jobs.** Targeted deletes stay in `update_sparql`. A full
    re-ingestion uses `replace: true`.
*   **Full SHACL.** Validation runs agent-supplied SPARQL checks.
*   **Remote sources.** No URL in `ingest_file`.
*   **Fine-grained write concurrency.** All writes go through the single write
    gate of the hardening design.

## Critical User Journeys (CUJs)

### CUJ 1: Agent ingests a code analysis in batches

*   **Given**: Project `P` is active. The agent has produced about 60k triples.
*   **When**: The agent calls `ingest_begin` (`mode: staged`), then sends 12
    N-Triples batches of about 5,000 triples with `seq` 1–12. It then calls
    `ingest_validate` with two checks, and `ingest_commit` with
    `require_validation: true`.
*   **Then**:
    *   Each `ingest_batch` returns within 300 ms, with
        `new_to_staging` and `already_staged`.
    *   Before the commit, `P` is unchanged.
    *   The commit makes all 60k triples visible at once. 60k is above
        `event_changed_threshold`, so the viewer feed shows one `changed`
        entry and the viewer resyncs.
    *   The commit reply has `new_subjects`, and the agent passes them to
        `viewer_show`.

### CUJ 2: Retry after a timeout

*   **Given**: A job with batches 1–4 accepted.
*   **When**: The call for batch 5 times out on the client side, and the agent
    re-sends batch 5 unchanged. The first attempt may still be running.
*   **Then**: Whichever attempt reaches the gate second gets `duplicate: true`
    with the first attempt's counts. The store holds batch 5 exactly once.

### CUJ 3: Oversized or malformed batch

*   **When**: The agent sends a 3 MiB batch, or one with a syntax error on line
    812.
*   **Then**:
    *   The batch is refused with `limit_exceeded` or `parse_error` (line,
        column), and nothing is written.
    *   No batch record is created, and the job stays `open`.
    *   The agent re-sends the same `seq` with corrected or smaller content.

### CUJ 4: Core restart mid-job

*   **Given**: A staged job with batches 1–30 of 50 accepted.
*   **When**: The Claude Code session restarts, which spawns a new core, and
    the agent calls `ingest_status`.
*   **Then**: The job is `open`, with `received_seqs: "1-30"` and the staged
    triple count. Downtime does not count toward expiry. The agent sends
    31–50 and commits.

### CUJ 5: Large dump file

*   **When**: The agent calls `ingest_file` on a 1.2 GiB N-Triples file
    inside `file_roots`.
*   **Then**:
    *   The call returns a `job_id` once the file is hashed.
    *   The core streams the file in batches of 10,000 triples, one
        transaction each.
    *   `ingest_status` shows bytes read, triples written, triples per
        second and an ETA.
    *   `query_sparql` p95 latency stays under 200 ms during the load.

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

### Layers

```text
MCP tools (application::tools::ingest)     arguments, error JSON
        │
IngestService (application::ingest)        state machine, idempotency, skolemization,
        │                                  events, expiry sweeper, file runner
        │  PrivilegedStore (write gate; may write jobs/staging graphs)
SparqlStore port (domain)                  parse_batch, write_batch, commit_job, graph_size, stream_file
        │
OxigraphAdapter (infrastructure)           transactions, streaming parser
```

### Job states

| From | Event | To | Side effects |
|---|---|---|---|
| — | `ingest_begin` | `open` | manifest created |
| — | `ingest_file` | `running` | manifest created, background task started |
| `open` | `ingest_batch` accepted or refused | `open` | batch record on accept only |
| `open` | `ingest_commit` refused (`missing_batches`, `validation_*`, `empty_replace`) | `open` | none |
| `open` | `ingest_commit` | `committed` | one transaction: commit update and state change |
| `open`, `interrupted`, `failed` | `ingest_abort` | `aborted` | staging graph dropped in chunks; direct data kept |
| `open`, `interrupted` | idle > `job_idle_ttl_secs` | `expired` | same as abort |
| `running` | end of file | `committed` (direct) or `open` (staged) | totals recorded |
| `running` | parse or limit error | `failed` | direct batches kept, manifest lists them |
| `running` | core restart | `interrupted` | none |
| `interrupted` | `ingest_file` with `resume_job` | `running` | none |

There is no persisted `committing` state. A commit and its state change run as
one Oxigraph transaction, so a crash leaves the job either `open` or
`committed`, never in between.

### Job ledger schema

The ledger lives in `urn:sparql-mcp:jobs`, with prefix
`smj: <urn:sparql-mcp:jobs#>`.

| Subject | Predicates | Objects | Notes |
|---|---|---|---|
| `urn:sparql-mcp:job:{id}` | `smj:state` | `smj:open`, `smj:running`, `smj:interrupted`, `smj:failed`, `smj:committed`, `smj:aborted`, `smj:expired` | replaced in the same transaction as each transition |
| job | `smj:target`, `smj:mode`, `smj:replace`, `smj:format`, `smj:expectedBatches`, `smj:label`, `smj:created`, `smj:lastActivity`, `smj:ended` | typed literals (`xsd:dateTime`, `xsd:long`, `xsd:boolean`) | `lastActivity` updated by every job call |
| `urn:sparql-mcp:job:{id}:batch:{seq}` | `smj:job`, `smj:seq`, `smj:sha256` (hex), `smj:parsed`, `smj:insertedNew`, `smj:alreadyPresent`, `smj:accepted` | literals | insert-only; job totals are sums over these, never a mutable counter |
| job | `smj:validatedAtBatches`, `smj:validationPassed` | literals | used by `require_validation` |
| job (file) | `smj:filePath`, `smj:fileSha256`, `smj:fileBytes`, `smj:checkpointBytes`, `smj:checkpointTriples` | literals | see [File jobs](#file-jobs-ingest_file) |

*   **Job id.** A ULID (48-bit time, 80 random bits), so job ids sort by time.
    Job ids are not a security boundary.
*   **Ordering.** Batches may arrive in any order. If `expected_batches` was
    given, the commit requires every seq from 1 to
    `max(expected_batches, highest received seq)`, with no gaps. Otherwise
    the commit is refused with `missing_batches` and the list of missing
    seqs.
*   **Records.** Only an accepted batch creates a record. An accepted batch
    cannot be replaced; to correct one, abort the job.
*   **Expiry.**
    *   Idle time only counts while a core runs. On start, the core moves
        every `lastActivity` forward by the downtime, which it measures from
        a heartbeat written every 5 minutes.
    *   A sweeper, also run every 5 minutes, expires `open` and
        `interrupted` jobs idle longer than `job_idle_ttl_secs`.
    *   Staging graphs, on expiry, abort or commit, are dropped in
        transactions of `file_batch_triples` triples. The gate is released
        between them.
*   **Retention.** The ledger entries of a finished job (`committed`,
    `aborted`, `expired`, `failed`) are deleted `job_retention_secs` after it
    ended.

### MCP tools

| Tool | Required | Optional (default) | Reply | Errors |
|---|---|---|---|---|
| `ingest_begin` | — | `target_graph` (the active graph), `mode` (`staged`), `replace` (false), `format` (`ntriples`), `expected_batches`, `label`, `blank_nodes` (`skolemize` \| `reject`) | `{job_id, staging_graph?, limits{…}}` | `invalid_target`, `too_many_open_jobs`, `replace_too_large`, `invalid_argument` |
| `ingest_batch` | `job_id`, `seq`, `data` | `format` (the job's) | see step 7 | `job_not_found`, `job_not_open`, `limit_exceeded`, `parse_error`, `named_graph_refused`, `reserved_iri`, `seq_conflict`, `blank_node_rejected`, `write_busy` |
| `ingest_validate` | `job_id`, `checks[]` | — | `{passed, results:[{name, passed, rows[≤20]}]}` | `not_a_query`, `validate_timeout`, `job_not_open`, `wrong_mode`, `limit_exceeded` |
| `ingest_commit` | `job_id` | `require_validation` (false), `allow_empty_replace` (false) | see [Commit](#commit) | `missing_batches`, `validation_required`, `validation_stale`, `empty_replace`, `write_busy` |
| `ingest_abort` | `job_id` | — | `{state: "aborted", direct_data_kept: bool, kept_seqs?}` | `job_not_found`, `job_not_open` |
| `ingest_status` | — | `job_id` | one job, or the list of non-finished jobs: `{job_id, state, mode, target, received_seqs (ranges, e.g. "1-30,32"), triples, file?{bytes_read, bytes_total, triples_per_s, eta_s}, validation?}` | `job_not_found` |
| `ingest_file` | `path` | `target_graph`, `mode` (`direct`), `format` (from the extension), `resume_job`, `force` (false) | `{job_id}` | `file_roots_unset`, `path_outside_roots`, `path_rejected`, `limit_exceeded` (`max_file_bytes`), `source_changed`, `staged_too_large`, `already_loaded` |

*   **Errors.** Every refusal uses the error JSON of the hardening design:
    `{"code", "message", "limit_key"?, "limit"?, "value"?, "line"?,
    "column"?, "job_id"?, "holder"?}`.
*   **Targets.** A job target must be an existing project graph
    (`urn:project:*`, recorded in `urn:meta`), or the active graph, which is
    accepted even when it is not registered (for example
    `urn:project:default`).

### Batch processing (`ingest_batch`)

1.  **Fast checks.** Before parsing, refuse when:
    *   the job does not exist, or is not `open`;
    *   `data.len() > max_batch_bytes`;
    *   `seq` is not between 1 and `max_job_batches`.

    These are fast paths only; step 5 re-checks inside the transaction.
2.  **Idempotency fast path.** Look up the batch record for `(job_id, seq)`.
    *   Same SHA-256 of the raw payload: return the stored `parsed`,
        `inserted_new` and `already_present` values with `duplicate: true`,
        plus the current job totals.
    *   Different SHA-256: return `seq_conflict`.
3.  **Parse completely**, in `spawn_blocking`, behind a semaphore of 2
    (`SparqlStore::parse_batch`).
    *   **Formats.** N-Triples (recommended) or Turtle. Named-graph syntaxes
        are refused with `named_graph_refused`. Each Turtle batch is parsed
        on its own: it must repeat its `@prefix` lines, and relative IRIs are
        refused, since there is no base IRI.
    *   **Size.** Parsing stops at `max_batch_triples + 1`, with
        `limit_exceeded`.
    *   **Terms.** Term limits and `reserved_iri` follow the hardening design.
    *   **Skolemization.**
        *   A labelled `_:x` becomes `<urn:sparql-mcp:genid:{job_id}:x>`,
            with the label percent-encoded.
        *   An anonymous node (`[]`, or a collection node) becomes
            `<urn:sparql-mcp:genid:{job_id}:{seq}:a{n}>`, where `n` is the
            node's order of first appearance in the batch. It is therefore
            stable when the same batch is retried.
        *   With `blank_nodes: reject`, any blank node is refused with
            `blank_node_rejected`.
    *   **Syntax errors** return `parse_error` with `line` and `column`
        (1-based, when the parser provides them), and nothing is written.
4.  **Job total fast path.** The job's triple total plus this batch must stay
    within `max_job_triples_staged` or `max_job_triples_direct`, depending on
    the mode.
5.  **Write, through `PrivilegedStore::exclusive`** (which takes the write gate once, on a blocking thread; the gate is not reentrant, so `WriteGate::run` is never wrapped around a store call). Inside one Oxigraph transaction:
    1.  Re-check steps 1, 2 and 4:
        *   the job is `open` → otherwise `job_not_open`;
        *   there is no record for `(job_id, seq)` → otherwise `duplicate`
            or `seq_conflict`;
        *   the total, summed from the batch records, is within the limit
            → otherwise `limit_exceeded`.

        If any check fails, return its result and write nothing.
    2.  Otherwise, for each quad (graph = staging or target), count it with
        `Transaction::contains`, which sees the transaction's own writes,
        then insert it.
    3.  Insert the batch record, update `lastActivity`, and commit.
6.  **Events (direct mode only).**
    *   While the job total stays at or below `event_changed_threshold`, the
        batch emits `Insert` events, split into chunks of
        `event_max_triples`.
    *   Above that threshold, at most one `Changed` event every 5 seconds,
        plus one at the end of the job.
    *   File jobs always use the `Changed` path.
7.  **Reply.**

    ```json
    {"job_id": "01JD…", "seq": 5, "parsed": 4998,
     "new_to_staging": 4990, "already_staged": 8, "duplicate": false,
     "job": {"batches": 5, "triples": 24970, "remaining_triples": 475030}}
    ```

    In direct mode, the counts are named `new_to_target` and
    `already_in_target`.

### Commit

*   **Staged, append.** One transaction holds both changes, joined by
    `Transaction::update` and the ledger quads:
    *   `ADD <staging> TO <target> ; DROP SILENT GRAPH <staging>`;
    *   the state change.

    Oxigraph 0.5.7 runs one update request as one transaction
    (`sparql/update.rs`, verified 2026-11-18). The transaction keeps about
    12 index operations per triple in memory: 6 for the copy and 6 for the
    drop.
*   **Staged, replace.** `MOVE SILENT <staging> TO <target>` in one
    transaction.
    *   **Memory.** It grows with the target size plus twice the staging
        size. `ingest_begin` with `replace: true` is therefore refused with
        `replace_too_large` when
        `graph_size(target) + max_job_triples_staged > max_replace_triples`.
    *   **Concurrent writes.** Writes made to the target after `ingest_begin`
        are discarded by the replace. The reply sets
        `target_writes_discarded: true` when `graph_size(target)` changed
        during the job.
    *   **Empty staging.** A replace with an empty staging graph is refused
        with `empty_replace` unless `allow_empty_replace: true`. This
        prevents a retried commit from wiping the target.
    *   **Guard.** The commit runs through `PrivilegedStore`, so the
        destructive-update guard does not apply to it.
*   **Direct.** The commit closes the job and records totals; the data is
    already in place.
*   **Events.**
    *   If the committed count is at most `event_changed_threshold`, the core
        reads the staging graph inside the commit transaction and emits split
        `Insert` events.
    *   Otherwise, and always for replace, it emits one `Changed` event. Its
        `triples_before` and `triples_after` are `graph_size(target)` before
        and after the commit.
    *   The viewer resyncs (`sparql-mcp-viewer.md`, CUJ 3: Recovering from a
        coarse update).
*   **Reply.** The reply holds:
    *   `totals`;
    *   `target_graph`;
    *   `target_triples`, from one `COUNT` on the target graph only;
    *   `new_to_target`;
    *   `new_subject_count`, and `new_subjects`: up to 5,000 IRIs, sorted,
        of subjects that had no triple in the target before the commit.
        They are computed in the commit transaction with
        `Transaction::contains`-style lookups on `(s, ?, ?, target)` before
        the copy. The agent passes `new_subjects` to `viewer_show`.
*   **Gate hold time.** The default `max_job_triples_staged` (500,000) is
    provisional. The Phase 1 bench measures peak memory and gate hold time
    for staged commits of 100k, 250k and 500k triples. The default is set so
    that a commit holds the gate for less than `write_wait_timeout_secs`.

### Validation (`ingest_validate`, staged mode)

The agent sends named checks:

```json
{"job_id": "01JD…", "checks": [
  {"name": "no-dangling-types",
   "query": "SELECT ?s WHERE { GRAPH {{staging}} { ?s a ?c } FILTER NOT EXISTS { GRAPH {{staging}} { ?c a ?k } } FILTER NOT EXISTS { GRAPH {{target}} { ?c a ?k } } } LIMIT 20",
   "expect": "empty"}]}
```

*   **Placeholders.** `{{staging}}` and `{{target}}` are replaced by
    `<iri>`, angle brackets included.
*   **Expectations.** `expect` is one of `empty`, `non_empty`, `true` or
    `false`.
*   **Caps.** At most 16 checks per call, each at most 16 KiB.
*   **Read-only.** Only `SELECT` and `ASK` are accepted. Any other form, and
    any use of `SERVICE`, is refused with `not_a_query`.
*   **Timeout.** Each check runs with
    `SparqlEvaluator::with_cancellation_token`. A timer cancels it after
    `validate_timeout_secs` (`validate_timeout`).
*   **Rows.** Rows are streamed, and reading stops after 20.
*   **Direct mode** returns `wrong_mode`.
*   **Commit with `require_validation`.** The manifest stores the number of
    accepted batches at validation time. Such a commit is refused unless:
    *   the last validation passed (`validation_required`);
    *   no batch arrived since then (`validation_stale`).

The author kept this tool after the review (finding M23). It is what makes
`require_validation` a guarantee rather than a convention.

### File jobs (`ingest_file`)

*   **Path checks.** Paths are checked against `file_roots` exactly as in the
    hardening design: Windows-first, with the final path read from the open
    handle. With `file_roots` unset, `ingest_file` is refused with
    `file_roots_unset`.
*   **Begin.**
    *   The core hashes the whole file (SHA-256) in a pre-pass, through
        `take(max_file_bytes)`.
    *   A completed `(hash, target)` pair in the load ledger is refused with
        `already_loaded` unless `force: true`.
    *   A staged file job whose size estimate (bytes ÷ 100 bytes per triple)
        exceeds `max_job_triples_staged` is refused with
        `staged_too_large`.
    *   The manifest records the path, hash and size.
*   **Run.**
    *   The file is parsed from a `BufReader`, in a background blocking task.
    *   For N-Triples, batches are cut at line boundaries every
        `file_batch_triples` triples.
    *   Each batch transaction also writes the checkpoint: `checkpointBytes`,
        the exact byte offset after the last line, and `checkpointTriples`.
    *   The gate is released between batches, so interactive writes
        interleave.
    *   The last batch transaction also writes the load-ledger entry for
        `(hash, target)`.
*   **Resume.** On restart, a `running` job becomes `interrupted`.
    `ingest_file` with `resume_job` re-hashes the whole file:
    *   a different hash fails with `source_changed`;
    *   otherwise, N-Triples seeks to `checkpointBytes` and continues;
    *   a Turtle file job restarts from the beginning, since re-inserting its
        quads is idempotent.

### Limits

These keys are added to `[ingest]`. The limits shared with ordinary writes
(`max_batch_bytes`, `max_batch_triples`, term sizes, the event thresholds,
`write_wait_timeout_secs`, `file_roots`, `file_batch_triples`) are defined in
the hardening design. `ingest_begin` returns all effective limits, so the
agent sizes its batches from data rather than guesses.

| Key | Default | Rationale |
|---|---|---|
| `max_job_batches` | 10,000 | Bounds the ledger |
| `max_job_triples_staged` | 100,000 | A commit is one in-memory transaction that holds the write gate: 100,000 triples take 6.6 s and 323 MiB, 500,000 take 48 s and 1.6 GiB (`docs/benchmarks/ingest.md`) |
| `max_job_triples_direct` | 10,000,000 | Per-batch transactions, so the bound is disk, not memory |
| `max_replace_triples` | 200,000 | Bounds the replace transaction: 100,000 + 100,000 take 9.4 s; a 250,000-triple target takes 12.8 s (`docs/benchmarks/ingest.md`) |
| `max_open_jobs` | 4 | Bounds the number of staging graphs |
| `job_idle_ttl_secs` | 3,600 | Cleans up abandoned staging data |
| `job_retention_secs` | 604,800 | Keeps finished-job ledgers for 7 days |
| `max_file_bytes` | 2,147,483,648 | Above CUJ 5's 1.2 GiB dump; bounds disk use and run time |
| `validate_timeout_secs` | 10 | Keeps one bad check from running unbounded |

Fixed constants: sweeper interval 5 minutes; validation rows 20; parse
semaphore 2.

--------------------------------------------------------------------------------

## Agent Workflow (Guidance for Skills)

This text goes into `plugins/kb-workbench/skills/sparql-first-code/SKILL.md`
and `plugins/kb-workbench/skills/kb-workbench/SKILL.md`.

1.  Call `ingest_begin` with `mode: staged`, the default. Use `direct` only
    above `max_job_triples_staged`, or for append-only logs. Read the
    returned limits.
2.  Emit **N-Triples**: no prefix state across batches, and the fastest to
    parse. Aim for 2,000–5,000 triples, and at most 512 KiB, per batch.
3.  Number batches from 1. On a timeout, a transport error or `write_busy`,
    re-send **the same `seq`** with the same content; a batch that already
    landed returns `duplicate: true`.
4.  On `parse_error` or `limit_exceeded`, nothing was recorded. Fix or shrink
    the batch and re-send it under the same `seq`. The remainder of a split
    batch goes into new seqs at the end.
5.  Run `ingest_validate` with checks that match the ontology, then call
    `ingest_commit` with `require_validation: true`.
6.  If a viewer is connected, pass the commit's `new_subjects` to
    `viewer_show`.
7.  Use `update_sparql` only for small, targeted edits, never for bulk loads.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: Raise limits on `load_ontology` and loop

*   **Pros:** No new tools.
*   **Cons:** No idempotency, no resume, no atomic multi-call commit, and no
    validation. Blank nodes are not stable across calls.
*   **Rejected.**

### Alternative B: Oxigraph `BulkLoader` for every batch

*   **Pros:** The fastest raw load. In 0.5.7 it commits atomically, unless
    `without_atomicity()` is set.
*   **Cons:**
    *   It can only insert.
    *   It offers no read-your-writes and no `contains`. So there are no
        new-versus-duplicate counts, and no in-transaction idempotency or
        state checks.
    *   Each call starts worker threads and writes SST files, which a crash
        leaves behind. That cost is far too high for 5,000-triple batches.
*   **Rejected for batches.** It remains a candidate for direct file jobs and
    for very large staged commits (see Future Improvements).

### Alternative C: Agent writes one file, then calls `ingest_file`

*   **Pros:** Reuses the streaming path, with a single call.
*   **Cons:** The agent needs write access inside `file_roots`. It must
    produce the whole file before any parse or validation runs, and learns of
    a late error only after the full file is written.
*   **Partly adopted:** `ingest_file` exists for dumps. Agent-produced data
    uses batches.

### Alternative D: Staging in a separate in-memory store

*   **Cons:** Staged data would be lost on restart. Validation checks against
    the target would span two stores. The commit would cross stores and not
    be atomic.
*   **Rejected.**

### Alternative E: Job state in process memory only

*   **Cons:** The core is spawned per Claude Code session, and sessions
    restart. Staging data would be orphaned.
*   **Rejected.**

### Alternative F: Hardening plus a minimal direct-only `ingest_batch`

*   **Pros:** A much smaller scope.
*   **Cons:** No atomic commit across batches, and no validation step.
*   **Partly adopted.** The author split the hardening into its own design,
    shipped first, and kept the full job model here.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Performance.** The targets are listed under [Goals](#goals).
    *   Ground writes do no whole-store scan. The commit reply runs one
        `COUNT` on the target graph, which costs O(target graph).
    *   All figures are measured with `examples/ingest_bench.rs`.
*   **Memory.**
    *   Parse memory is estimated at 10× `max_batch_bytes`, to be confirmed
        by the bench.
    *   A staged commit is bounded by `max_job_triples_staged`, and a replace
        by `max_replace_triples`.
    *   File jobs stream. Their peak working set, measured with
        `GetProcessMemoryInfo`, stays below 512 MiB whatever the file size.
*   **Reliability.**
    *   Each batch is exactly-once: data and batch record share one
        transaction, and the checks are re-run under the gate.
    *   A staged commit is all-or-nothing.
    *   A direct job is atomic per batch only. An aborted, expired or failed
        direct job leaves its accepted batches in the target, and its
        manifest lists their seqs.
    *   A crash leaves a staged job resumable or expirable, never
        half-committed.
*   **Security.**
    *   Limits, guards and `file_roots` follow the hardening design.
    *   Validation is read-only and time-limited, with no `SERVICE`.
    *   Skolem IRIs live in the reserved `urn:sparql-mcp:` namespace, which
        payloads cannot use.
*   **Observability.**
    *   `tracing` spans per job and per batch.
    *   `ingest_status` with no id lists the non-finished jobs.
    *   `stats` gains `open_jobs` and `staging_triples`, summed from batch
        records with no `COUNT`.
    *   Staging graphs are hidden from `list_graphs`, but readable by
        queries.

## Risks and Open Questions

| Risk / question | Impact | Mitigation | Verified in |
|---|---|---|---|
| Staged-commit memory at the default limit | High memory use, long gate hold | Bench; lower the default | Phase 1 |
| Disk growth from staging graphs and the job ledger | Disk use | Chunked drops, expiry, retention | Phase 1 |
| Raw-payload hash makes a re-serialized retry a `seq_conflict` | Agent must re-send the same bytes | Documented in the workflow. Hashing a canonical form (sorted N-Triples after skolemization) is a Future Improvement. | — |
| Replace discards concurrent writes to the target | Lost edits | `target_writes_discarded` in the reply | Phase 1 |
| Resolved: one update request is one transaction | — | `sparql/update.rs` | 2026-11-18 review |
| Resolved: `Transaction::contains` sees uncommitted writes | — | `store.rs:1279` | 2026-11-18 review |

## Future Improvements

*   **SHACL validation**, when a Rust SHACL engine fits the dependency budget.
*   **Delete and upsert batches.**
*   **`BulkLoader` paths:** for direct file jobs, and for staged commits
    above a size threshold, if the Phase 1 bench justifies it.
*   **Canonical batch hash**, so that a re-serialized retry is still
    recognized as a duplicate.
*   **Job progress events** on the viewer channel.
*   **Per-project quotas.**

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Required changes to the companion designs

*   **[`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md).**
    `Changed` is also emitted by staged and replace commits, by direct jobs
    above the threshold, and by file jobs. It is not emitted for every
    batch; see [Batch processing](#batch-processing-ingest_batch), step 6.
*   **[`sparql-mcp-viewer.md`](./sparql-mcp-viewer.md).** No feed change:
    CUJ 1 shows a `changed` entry.
*   **[`sparql-mcp-viewer-agent-control.md`](./sparql-mcp-viewer-agent-control.md).**
    The commit reply returns `new_subjects` (at most 5,000 IRIs) for
    `viewer_show`.

### Delivery phases

The [write-path hardening](./sparql-mcp-write-path-hardening.md) must be
complete first. Each phase starts only when the previous phase's exit
criterion passes.

| Phase | Scope | Files | Exit criterion |
|---|---|---|---|
| 1 | Jobs: begin, batch, commit, abort, status; ledger; skolemization; direct and staged modes; expiry; retention | 0–6, 8 | CUJs 2–3 in memory. CUJ 4 and the gate race tests on a temp RocksDB store, including two concurrent `ingest_batch` calls with the same seq producing one record. CUJ 1 without the validation and viewer steps. A duplicate seq is a no-op; a conflicting seq is refused. A retried replace commit with empty staging is refused. The bench records 5,000-triple batch latency and staged-commit memory and duration, and the default is confirmed or lowered. |
| 2 | `ingest_validate`, `require_validation` | 4, 5, 8 | Each `expect` value. A non-query and `SERVICE` are refused. A timeout is enforced. `validation_stale` after a new batch. CUJ 1 end to end, without the viewer. |
| 3 | `ingest_file`: hashing, streaming, checkpoints, resume, progress | 3, 4, 8 | CUJ 5 on a generated 1 GiB file: ≥ 50,000 triples/s; peak working set < 512 MiB; `query_sparql` p95 < 200 ms. Resume after killing the process. `source_changed` after the file is edited. |
| 4 | Docs, skill guidance, viewer tie-in | 9 | README and CHANGELOG updated; the two SKILL.md files updated; manual CUJ 1 with the viewer and `viewer_show`. |

### Implementation status

**Phase 1, slice 1 (2026-11-18): the job lifecycle.** Done:

*   the `[ingest]` keys of [Limits](#limits) (`max_open_jobs`, `max_job_batches`,
    `max_job_triples_staged`/`_direct`, `max_replace_triples`, `job_idle_ttl_secs`,
    `job_retention_secs`, `max_file_bytes`, `validate_timeout_secs`,
    `max_batch_triples`);
*   `application/ingest.rs`: the job ledger in `urn:sparql-mcp:jobs` (manifest as
    `smj:` quads), `JobState` and its transitions, ULID job ids, sequence ranges;
*   `ingest_begin`, `ingest_status`, `ingest_abort` (staging graph dropped in the
    same update request as the state change), with target validation, the
    `max_open_jobs` limit and the `invalid_*` / `job_*` codes (`ToolError::Coded`);
*   every ledger read-then-write runs under `PrivilegedStore::exclusive`, so it
    holds the write gate once; every value from outside goes through
    `sparql_text::string_literal` / `iri`, and a job id that is not a ULID is never
    put in a query.

**Phase 1, slice 2 (2026-11-18): `ingest_batch`.** Done:

*   the port primitive `SparqlStore::write_batch(BatchWrite)`: in **one**
    transaction it checks that required quads are present (the job is `open`),
    that the batch was not already accepted (`Duplicate`) or does not clash with
    one that was (`Conflict`), advances numeric counters against their ceilings
    (`CounterLimit`), and writes the data, the batch record and the new/duplicate
    counts. It knows nothing about jobs: the ledger vocabulary stays in
    `application/ingest.rs`. A refusal writes nothing. `GatedStore` refuses it for
    non-privileged callers (the ledger is a protected graph);
*   `parse_rdf_raw`, which keeps blank node labels as written (the port's
    `parse_rdf` renames them per call, which would make a retry non-idempotent);
*   `ingest_batch`: cheap checks and an idempotency fast path outside the gate,
    then parse, skolemize and check, then `write_batch`. Labelled blank nodes become
    `<urn:sparql-mcp:genid:{job}:{label}>`; parser-generated ones (32 hexadecimal
    digits) become `<urn:sparql-mcp:genid:{job}:{seq}:a{n}>` by order of first
    appearance, so a retried Turtle batch gets the same IRIs. IRIs in
    `urn:sparql-mcp:` are `reserved_iri`; IRI and literal sizes are limited
    (`max_iri_bytes`, `max_literal_bytes`); `blank_nodes: reject` is honoured;
*   a retry that overlaps the original, or loses a race for the same seq, gets the
    original counts with `duplicate: true`; a different payload for the same seq is
    `seq_conflict`;
*   direct jobs write to the target and emit `Insert` events (bounded), or one
    `Changed` at most every 5 s above `event_changed_threshold`; staged jobs emit
    nothing until commit.

Deviations from the text above, with the reason:

*   **The job's totals are running counters** (`smj:triplesTotal`, `smj:batchCount`
    on the manifest), advanced in the same transaction as the batch record, instead
    of sums over the batch records. A sum is O(batches) per batch, so O(n²) over a
    job of 10,000 batches. The counter cannot be lost: the check and the update are
    one transaction under the write gate.
*   **The port has `write_batch` and `parse_rdf_raw`**, not `parse_batch` and
    `commit_job`: parsing stays outside the transaction, and the checks are data
    (`BatchWrite`), not code.
*   `smj:lastActivity` is set by `write_batch` itself (`BatchWrite.replace`, a
    single-valued property replaced in the batch's transaction), and refreshed by a
    duplicate retry (best effort). A refused batch does not refresh it.

Tests: 30 end-to-end tests, including three on a RocksDB store with concurrent
calls (eight overlapping retries of one batch: stored once, counted once; six
different payloads for one seq: exactly one lands; ten batches against a
ceiling of four: exactly four fit). With the in-transaction checks disabled, five
of them fail.

**Phase 1, slice 3 (2026-11-18): `ingest_commit`.** Done:

*   `ingest_commit` for the three modes, under one hold of the write gate:
    staged append (`ADD SILENT` + `DROP SILENT GRAPH` + the state change), staged
    `replace` (`MOVE SILENT` + the state change) and direct (state change only).
    The data move and the state change are one update request, so one
    transaction. A batch that arrives meanwhile waits, then is refused
    (`job_not_open`): it is in the commit or not accepted, never lost in staging;
*   `missing_batches` (every seq from 1 to `expected_batches`, or to the highest
    seq sent), `empty_replace` (refused unless `allow_empty_replace`),
    `replace_too_large` (checked at `ingest_begin` from the target's size),
    `target_writes_discarded` (the target's size changed since the job began);
*   the reply carries `new_to_target`, `new_subject_count` and `new_subjects`
    (sorted, bare IRIs, at most 5,000) for staged jobs, for `viewer_show`;
*   events: a small staged append is reported triple by triple, read before the
    move and named after the **target** graph; a large one, and every replace, is one
    `Changed` with the target's real before/after sizes; a large direct job gets a
    final `Changed`;
*   `ingest_validate` (`ingest/validate.rs`) and `SparqlStore::query_bounded` (a timer thread
    sets an Oxigraph `CancellationToken`; reading stops after 20 rows). Placeholders are
    substituted **before** parsing (`{{staging}}` is not valid SPARQL), the query is then
    parsed with spargebra: only `SELECT`/`ASK`, and any `SERVICE` (also inside `FILTER
    EXISTS`) is refused, as is a string literal containing the word (the safe side). The
    job state is checked before the syntax. `expect` defaults to `empty` for a `SELECT`
    and `true` for an `ASK`; a mismatch is `not_a_query`. The outcome
    (`validationPassed`, `validatedAtBatches`) is written under the write gate only if the
    job is still open and no batch arrived meanwhile (`recorded: false` otherwise); a
    timeout records nothing. The race between a batch and the recording is covered by this
    guard but not by a test (it would need a check of controlled duration).

Tests: 45 end-to-end, five on a RocksDB store with concurrent callers (five
simultaneous commits close the job once; eight batches racing a commit are each in
the commit or refused). With the open-state check or the empty-replace check
disabled, three tests fail.

**The bench (2026-11-18)** confirmed `ingest_batch` and the hardening promise, and
**lowered the provisional ceilings**: `max_job_triples_staged` 500,000 → **100,000**
and `max_replace_triples` 1,000,000 → **200,000**. A 500,000-triple commit held the
write gate for 48 s (longer than `write_wait_timeout_secs`, 30) and took 1.6 GiB. See
[`docs/benchmarks/ingest.md`](../benchmarks/ingest.md). A faster commit (moving the quads
directly instead of `ADD` + `DROP`) is a possible optimisation, after which the ceiling
would be raised only after rerunning the bench.

**Expiry and maintenance (`ingest/maintenance.rs`).** A thread started by the server
sweeps every `sweep_interval_secs` (300): `open`/`interrupted` jobs idle longer than
`job_idle_ttl_secs` become `expired` and their staging graph is dropped in the same
transaction (re-checked under the write gate, so a batch that arrives during the sweep
wins or is refused, never lost into a dropped graph); finished jobs are purged
`job_retention_secs` after `smj:ended`. A heartbeat is written on every sweep. At start,
`recover` reads it: the downtime is added to every active job's `lastActivity` so that
time with the core off is not idle time, and jobs that were `running` become
`interrupted`. `ingest_status` reports `idle_secs` and `expires_in_secs` for unfinished jobs.
All of it takes the time as a parameter, so tests move the clock instead of sleeping.

**Not yet done:** chunked staging drops for large graphs (a staged job above
`max_job_triples_staged` is refused earlier, so abort and commit drop a bounded graph in
one request); `ingest_file`.

### Target Files to Create/Modify

#### 0. `crates/sparql-mcp-core/Cargo.toml` (Modify)

Add `ulid = "1"`. `sha2`, `spargebra` and tokio `time` are already present
(the last one is added by the hardening design).

#### 1. `crates/sparql-mcp-core/src/config.rs` (Modify)

Add the job keys from [Limits](#limits) to `IngestConfig`, with tests.

#### 2. `crates/sparql-mcp-core/src/domain/sparql_store.rs` and `domain/mod.rs` (Modify)

New port methods:

*   `parse_batch(&self, bytes: &[u8], format, limits: &Limits, bnodes: BnodePolicy) -> Result<Vec<StoredQuad>, ParseError>`;
*   `write_batch(&self, quads: &[StoredQuad], ledger: &[StoredQuad], guard: BatchGuard) -> Result<BatchWrite { inserted_new, already_present }, BatchRefusal>`.
    `BatchGuard` carries the step-5 re-checks, which run inside the
    transaction;
*   `commit_job(&self, update: &str, ledger_ops: LedgerOps, new_subjects_cap: usize) -> Result<CommitResult, StoreError>`.
    It uses one `Transaction`, with `Transaction::update` followed by the
    ledger ops;
*   `stream_file(&self, file: File, opts, batch: usize, from_offset: u64, on_batch: impl FnMut(FileBatch) -> Result<(), StoreError>) -> Result<FileSummary, StoreError>`.

`graph_size` and `StoredQuad` come from the hardening design.

#### 3. `crates/sparql-mcp-core/src/infrastructure/oxigraph.rs` (Modify)

*   Implement the new port methods.
*   Skolemization inside `parse_batch`.
*   Streaming parse with byte-offset checkpoints for N-Triples.

#### 4. `crates/sparql-mcp-core/src/application/ingest.rs` and `application/mod.rs` (Create / Modify)

`IngestService`, which holds:

*   the state machine;
*   the ledger reads and writes (`smj:`);
*   idempotency;
*   the commit SPARQL;
*   event emission (step 6 rules);
*   the sweeper, heartbeat and retention;
*   the file runner.

#### 5. `crates/sparql-mcp-core/src/application/tools/ingest.rs` and `tools/mod.rs` (Create / Modify)

The 7 tool definitions with JSON schemas, argument parsing, and replies.

#### 6. `crates/sparql-mcp-core/src/mcp/server.rs` and `src/main.rs` (Modify)

*   Hold `Arc<IngestService>`, and dispatch the `ingest_*` tools.
*   Extend `every_declared_tool_has_a_dispatch_arm` to them.
*   Start the sweeper task in `main.rs`.
*   The `instructions` text gains the rule: bulk data goes through
    `ingest_*`.

#### 7. `crates/sparql-mcp-core/examples/ingest_bench.rs` (Create or extend)

Generates N-Triples. It measures batch latency, staged-commit memory and
duration, and file-job throughput against a temporary RocksDB store.

#### 8. `crates/sparql-mcp-core/tests/ingest_jobs.rs`, `tests/ingest_file.rs` (Create)

The CUJs and exit criteria of phases 1–3. Every race and resume test runs on a
temporary RocksDB store.

#### 9. Docs (Modify)

`README.md`, `sparql-mcp.toml` (commented job keys), `CHANGELOG.md`, and the
two SKILL.md files named in [Agent Workflow](#agent-workflow-guidance-for-skills).

### Verification Plan

*   `cargo build --workspace`, `cargo test --workspace`,
    `cargo clippy --workspace -- -D warnings`, `cargo fmt --check`.
*   Bench results recorded in the PR: batch latency, staged-commit memory and
    duration, and file-job throughput and peak working set.
*   Manual runs: CUJ 1 with the viewer connected; CUJ 4 by killing the core
    mid-job.
