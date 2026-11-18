# sparql-mcp: Batch Ingestion over MCP

*   **Author:** dminier
*   **Status:** Draft
*   **Date:** 2026-11-18

--------------------------------------------------------------------------------

## Overview

Agents write to `sparql-mcp` through three MCP tools: `update_sparql`,
`load_ontology` (inline RDF) and `load_ontology_file`. These tools work for
small, hand-sized writes. They are not built for what agents increasingly do:
batch work that turns a source (a codebase, a document set, an export) into
tens or hundreds of thousands of triples, sent over many tool calls.

This document adds **batch ingestion jobs** to `sparql-mcp-core`. A job has an
id, numbered batches, per-batch atomicity, retry-safe idempotency, an optional
staging graph with validation before commit, a persistent ledger so a job can
resume after a crash, and server-side streaming for large files. Every input
is bounded by limits set in `[ingest]` of `sparql-mcp.toml`.

The work also hardens the existing write tools. A review of the current write
path (see *Current write path: findings*) found unbounded inputs, non-atomic
loads, full-store scans on every write, and events tagged with the wrong
graph.

Related designs:

| Concern | Owned by |
|---|---|
| Side-channel, `StoreEvent` wire format | `interface-agentic-rust-stylee.md` |
| Viewer rendering of ingestion (feed, last-batch pulse) | `sparql-mcp-viewer.md` |
| Agent control of the viewer (`viewer_show` after a commit) | `sparql-mcp-viewer-agent-control.md` |
| Ingestion jobs, limits, write-path hardening | this document |

## The Problem

*   **No batch semantics.** An agent that ingests 200k triples must chain
    hundreds of `load_ontology` calls. Nothing ties them together. After a
    failure, the agent cannot tell which calls landed, and retrying may
    duplicate blank-node data.
*   **No atomicity.** `load_rdf_impl` inserts quads one by one, each in its
    own transaction. A parse error at quad 5,001 leaves the first 5,000 in
    the store.
*   **No limits.** Payload size, triple count, literal length and file size
    are all unbounded. One oversized tool call can exhaust memory in the
    core, in the viewer broadcast buffer, and in the viewer.
*   **Writes cost O(store).** Every write calls `Store::len()` two to four
    times. In Oxigraph 0.5, `len()` "executes a full scan". Ingestion slows
    down as the store grows.
*   **Unsafe destructive updates.** `update_sparql` accepts `DROP ALL` and
    `CLEAR` on any graph, including the project registry `urn:meta` and the
    load ledger `urn:hkb:meta:loaded-files`.
*   **No validation step.** Data goes straight into the project graph. There
    is no place to check it (dangling references, missing labels, wrong
    types) before readers and the viewer see it.

## Goals

*   **Job-based batching.** `ingest_begin` → `ingest_batch` × N →
    (`ingest_validate`) → `ingest_commit` or `ingest_abort`. Also
    `ingest_status`, and `ingest_file` for server-side file loads.
*   **Atomic batches.** A batch is fully parsed and checked before anything
    is written, then written in one transaction together with its ledger
    record. A batch either lands entirely or not at all.
*   **Idempotent retries.** A batch is keyed by `(job_id, seq)`. Re-sending
    the same seq with the same content is a no-op that returns the original
    result. The same seq with different content is rejected.
*   **Two commit modes.**
    *   `staged` (default): batches land in a private staging graph. Commit
        moves them into the target graph in one transaction. Abort drops the
        staging graph.
    *   `direct`: each batch lands in the target graph at once. This is for
        large append-only loads.
*   **Optional replace.** `replace: true` makes a staged commit replace the
    target graph's contents, for re-ingesting a source from scratch.
*   **Stable blank nodes.** Blank nodes are skolemized per job, so `_:a` means
    the same node across batches and retries.
*   **Bounded everything.** Batch bytes, batch triples, term size, job size,
    open jobs, idle time and file size all have configured limits. A refusal
    names the limit and the value.
*   **Resumable.** Job state lives in the store, not in process memory. After
    a core restart, `ingest_status` lists received batches and the agent
    re-sends the missing ones.
*   **Fast.** Writes are batched transactions with no full scans, and run off
    the async runtime. Target throughput is at least 50k triples/s for N-Triples
    batches on the author's machine, measured in Phase 1.
*   **Visible in the viewer.** Each direct batch or staged commit produces
    bounded `StoreEvent`s, so the ingestion feed shows real progress.
*   **Hardened existing tools.** `update_sparql`, `load_ontology` and
    `load_ontology_file` get the same limits, transactional loading, correct
    event graph tagging and a destructive-update guard.

## Non-Goals

*   **Extraction and mapping.** Turning sources into RDF stays the agent's
    job, or the job of skills such as `kb-workbench`. The core receives RDF.
*   **Deletes inside jobs.** Jobs insert. Targeted deletes stay in
    `update_sparql`. Full re-ingestion uses `replace: true`.
*   **Full SHACL.** Validation runs agent-supplied SPARQL checks, not a
    SHACL engine (see Future Improvements).
*   **Remote sources.** No `LOAD <http://…>`, and no URL in `ingest_file`.
    The lockfile has no `oxhttp`, so Oxigraph's HTTP client is not compiled
    in, and this design keeps it that way.
*   **Multi-writer concurrency.** One core process owns the RocksDB store,
    and its writes are serialized (see *Write gate*).

## Critical User Journeys (CUJs)

### CUJ 1: Agent ingests a codebase analysis in batches

*   **Given**: Project `P` is active. The agent has produced about 60k
    triples from a code analysis.
*   **When**: The agent calls `ingest_begin` (`mode: staged`). It then sends
    12 N-Triples batches of about 5k triples with `seq` 1–12, runs
    `ingest_validate` with two checks, and calls `ingest_commit`.
*   **Then**:
    *   Each `ingest_batch` returns within 300 ms with parsed, new and
        duplicate counts.
    *   Before commit, project graph `P` is unchanged and the viewer shows
        nothing new.
    *   Commit makes all 60k triples visible at once. The viewer feed shows
        one `job commit` entry and resyncs.
    *   The agent calls `viewer_show` on the new classes.

### CUJ 2: Network hiccup and retry

*   **Given**: A job with batches 1–4 accepted.
*   **When**: The tool call for batch 5 times out on the client side, and the
    agent re-sends batch 5 unchanged.
*   **Then**: If batch 5 had landed, the reply is `duplicate: true` with the
    original counts. If it had not, it lands now. Either way, the store holds
    batch 5 exactly once.

### CUJ 3: Oversized or malformed batch

*   **When**: The agent sends a 3 MiB batch, or a batch with a syntax error on
    line 812.
*   **Then**:
    *   The batch is rejected with `limit_exceeded` (`max_batch_bytes`, the
        limit and the actual value) or `parse_error` (line, column, message).
    *   Nothing from that batch is written, and the job stays open.
    *   The agent splits the batch or fixes the line, and re-sends the same
        `seq`.

### CUJ 4: Core restarts mid-job

*   **Given**: A staged job with batches 1–30 of 50 accepted.
*   **When**: The Claude Code session is restarted, which spawns a new core,
    and the agent calls `ingest_status` with the job id.
*   **Then**: Status is `open`, with received seqs 1–30 and staged triple
    count. The agent sends 31–50 and commits.

### CUJ 5: Large dump file

*   **When**: The agent calls `ingest_file` on a 1.2 GB N-Triples file inside
    an allowed root.
*   **Then**:
    *   The call returns a job id at once.
    *   The core streams the file in batches of 10k triples without loading
        it into memory, and writes each batch as one transaction.
    *   `ingest_status` shows bytes read, triples written and throughput.
    *   Interactive `query_sparql` calls stay responsive during the load.

### CUJ 6: Destructive update refused

*   **When**: An agent calls `update_sparql` with `DROP ALL`.
*   **Then**: The call is refused with `destructive_update_forbidden`, and the
    tool result explains the `[ingest] allow_destructive_updates` setting.
    `DROP GRAPH <urn:project:P>` is refused the same way unless the setting is
    on. Writes to protected graphs are always refused.

--------------------------------------------------------------------------------

## Current write path: findings

These findings were verified in the code on 2026-11-18. Each one is fixed in
Phase 0.

| # | Finding | Where | Risk | Fix |
|---|---|---|---|---|
| F1 | No size limit on `update`, `turtle`, or file size | `tools/sparql.rs`, `tools/ontology.rs`, `oxigraph.rs::load_rdf_file` (`fs::read`) | Memory exhaustion of core and viewer | `[ingest]` limits on every write tool |
| F2 | `load_rdf_impl` calls `Store::insert` per quad, which is one transaction per quad | `oxigraph.rs::load_rdf_impl` | Partial loads on parse error, and slow | Parse fully, then one transaction (`start_transaction` + `extend`) |
| F3 | `Store::len()` (full scan) 2–4 times per write | `update_with_outcome`, `load_rdf_impl`, `load_rdf_file`, `load_ontology` | Write cost grows with store size | Count new quads with `contains` per quad under the write gate. Keep `len()` only for pattern updates. |
| F4 | Events are tagged with the **active** graph even when the data went to another graph (`graph_iri` argument, or `INSERT DATA { GRAPH <g> {…} }`). `ground_quads` drops the quad's graph. | `ontology.rs::emit_insert`, `sparql.rs::update_sparql`, `oxigraph.rs::ground_quads` | The viewer draws triples that are not in the active graph | Carry the graph per quad. Emit events per actual graph. |
| F5 | One `StoreEvent` carries every triple of a load, whatever its size, through a 1024-slot broadcast | `emit_insert` | Unbounded memory held for slow viewers | Split events at `event_max_triples`. Above `event_changed_threshold`, emit `Changed`. |
| F6 | `dry_run` returns before parsing anything | `sparql.rs::update_sparql` | Agents trust a validation that does not happen | Parse with `spargebra` in dry run, and report the operation kinds |
| F7 | No guard on `DROP`/`CLEAR` (including `ALL`) or on writes to `urn:meta` and `urn:hkb:meta:*` | `update_sparql` | Project registry or load ledger wiped by one call | Destructive-update guard and protected graphs |
| F8 | `load_ontology_file` reads any path the process can read | `ontology.rs::load_ontology_file` | Exposure of files outside the workspace | `[ingest] file_roots` allow-list (see *Compatibility*) |
| F9 | Write tools are synchronous and run on Tokio worker threads | `server.rs::call_tool` → `dispatch_core` | A long load stalls the runtime, including the viewer channel | `spawn_blocking` under the write gate |

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

### Layers

```text
MCP tools (application::tools::ingest)        ingest_begin / batch / validate / commit / abort / status / file
        │  argument parsing, limits, error codes
        ▼
IngestService (application::ingest)           job state machine, idempotency, skolemization,
        │                                     event splitting, ledger records
        ▼
WriteGate (application::write_gate)           one writer at a time, spawn_blocking
        │
        ▼
SparqlStore port (domain)                     new: write_batch(quads, ledger) / run_update / graph_size / staging ops
        │
        ▼
OxigraphAdapter (infrastructure)              transactions, streaming parser
```

`update_sparql`, `load_ontology` and `load_ontology_file` go through the same
`Limits` checks and the same `WriteGate`.

### Job model

```text
            ingest_begin
                 │
                 ▼
   ┌────────── open ◀──────── ingest_batch (seq n) ──┐
   │             │  └─────────────────────────────────┘
   │  ingest_abort│ ingest_commit
   │             ▼
   │         committing ──(1 transaction)──▶ committed
   ▼
 aborted            open ──(idle > job_idle_ttl)──▶ expired (staging dropped)
```

*   **Job id.** A ULID, for example `01JD…`. It sorts by time and is
    unguessable enough for a local tool.
*   **Manifest.** Stored in `urn:sparql-mcp:jobs` (a protected graph): id,
    label, target graph, mode, replace flag, format, `expected_batches`,
    state, created and updated times, and totals.
*   **Batch records.** One per accepted batch: seq, SHA-256 of the raw
    payload, parsed, new and duplicate counts, and acceptance time. A batch
    record is written **in the same transaction** as the batch data. That
    is what makes batches exactly-once.
*   **Staging graph.** `urn:sparql-mcp:staging:{job_id}` (reserved prefix,
    protected from every other tool).
*   **Ordering.** Batches may arrive in any order. If `expected_batches` was
    given, commit requires seqs `1..=expected_batches` to be present, and
    otherwise returns `missing_batches` with the list.
*   **Expiry.** A sweeper runs on core start and every 5 minutes. It expires
    open jobs idle longer than `job_idle_ttl` and drops their staging graph.
    Direct-mode data already written stays, and the manifest records that.

### Batch processing (`ingest_batch`)

1.  **Cheap checks, before parsing:** the job exists and is `open`;
    `data.len() <= max_batch_bytes`; `seq` is between 1 and `max_job_batches`.
2.  **Idempotency.** Look up the record `(job, seq)`. If it exists with the
    same SHA-256, return its stored result with `duplicate: true`. If it
    exists with a different SHA-256, return `seq_conflict`.
3.  **Parse completely into memory**, with Oxigraph's `RdfParser`. Supported
    formats are N-Triples (recommended) and Turtle. Named-graph syntaxes are
    refused (`without_named_graphs`), because the job decides the graph.
    While parsing:
    *   stop at `max_batch_triples` + 1 → `limit_exceeded`;
    *   check each term against `max_iri_bytes` and `max_literal_bytes`;
    *   skolemize blank nodes: `_:x` becomes
        `<urn:sparql-mcp:genid:{job_id}:x>` (or reject them when
        `blank_nodes: reject`);
    *   on a syntax error, return `parse_error` with line, column and
        message, and write nothing.
4.  **Job total check.** Job triples so far plus this batch must stay under
    `max_job_triples` for the mode.
5.  **Write, under the write gate, in `spawn_blocking`.** One transaction
    holds the quads (graph = staging or target) plus the batch record and
    the updated manifest totals. New-versus-duplicate counts come from
    `contains` checks inside the same gate, so they are exact without
    `len()`.
6.  **Events (direct mode only).** Emit `Insert` events for the target graph,
    split into chunks of `event_max_triples`.
7.  **Reply:**

```json
{"job_id":"01JD…","seq":5,"parsed":4998,"inserted_new":4990,"already_present":8,
 "duplicate":false,"job":{"batches":5,"triples":24970,"remaining_triples":475030}}
```

### Commit

*   **Staged, append.** One SPARQL update request:
    `ADD <staging> TO <target> ; DROP GRAPH <staging>`, plus the manifest
    state change. Oxigraph runs one update request as one transaction, so
    the commit is atomic. This is why staged jobs have a lower
    `max_job_triples`: Oxigraph documents large transactions as memory-heavy.
*   **Staged, replace.** `MOVE <staging> TO <target>` in one transaction,
    which drops the target, copies, and drops the staging graph.
*   **Direct.** Commit only closes the job and records totals. The data is
    already in place.
*   **Events after commit.**
    *   If the committed triple count is at most `event_changed_threshold`,
        the core reads the staging graph before the move and emits `Insert`
        events, split by `event_max_triples`.
    *   Above the threshold, or for replace, it emits one `Changed` event,
        and the viewer resyncs from a snapshot (base design CUJ 3).
*   **Reply:** totals, target graph, the target graph's triple count (one
    `COUNT` over that graph only, never a whole-store scan), and the number
    of distinct new subjects, so the agent can follow up with
    `viewer_show`.

### Validation (`ingest_validate`, staged mode)

The agent sends named checks. Each check is a SPARQL `SELECT` or `ASK` in
which `{{staging}}` and `{{target}}` are replaced by the job's graph IRIs.
Each check states `expect: empty | non_empty | true | false`.

```json
{"job_id":"01JD…","checks":[
  {"name":"no-dangling-types",
   "query":"SELECT ?s WHERE { GRAPH {{staging}} { ?s a ?c } FILTER NOT EXISTS { GRAPH {{staging}} { ?c a ?k } } FILTER NOT EXISTS { GRAPH {{target}} { ?c a ?k } } } LIMIT 20",
   "expect":"empty"},
  {"name":"every-class-labelled","query":"…","expect":"empty"}]}
```

*   Checks are read-only. The parser rejects anything that is not a query.
*   Each check runs with `validate_timeout` and returns at most 20 offending
    rows.
*   The result is stored on the manifest. `ingest_commit` with
    `require_validation: true` refuses to commit unless the last validation
    passed and no batch arrived after it.

### File jobs (`ingest_file`)

*   **Path checks.** The path must canonicalize to a location inside one of
    `[ingest] file_roots`. Symlinks are resolved before the check, and the
    file size must be at most `max_file_bytes`.
*   **Background run.** The job runs on a blocking task. The parser reads
    from a `BufReader` (streaming, never `fs::read`), and the SHA-256 is
    computed while reading. Every `file_batch_triples` quads (default 10k),
    one batch transaction is written, with a record of the quads consumed so
    far.
*   **Resume.** On restart, a `running` file job is marked `interrupted`.
    `ingest_file` with `resume_job` re-parses the file, skips the recorded
    quad count, and continues. This is valid only if the SHA-256 of the file
    prefix still matches; otherwise it fails with `source_changed`.
*   **Modes.** Both modes are allowed. A staged file job above the staged
    `max_job_triples` is refused at begin, from the file size and a bytes per
    triple estimate, and again at the limit during the run.
*   **Fairness.** The writer releases the write gate between batches, so
    interactive writes interleave.
*   **Progress.** `ingest_status` reports bytes read out of the total,
    triples written, triples per second, and an ETA.
*   **Idempotence.** The existing SHA-256 idempotence of `load_ontology_file`
    applies: a completed file with the same hash is skipped unless
    `force: true`.

### Write gate

*   **One writer.** `WriteGate` is a Tokio mutex shared by every write tool
    and by file jobs. Holding it, the tool runs its store work in
    `spawn_blocking`.
*   **What this gives:** exact new-versus-duplicate counts without `len()`;
    no transaction conflicts between an agent's `update_sparql` and a
    running file job; and Tokio workers free for the MCP transport and the
    viewer channel.
*   **Reads take no gate.** `query_sparql` reads an Oxigraph snapshot, so it
    is not blocked by writes.
*   **Waiting.** A write that waits longer than `write_wait_timeout` (default
    30 s) returns `write_busy`, with the id of the job holding the gate.

### Limits (`[ingest]` in `sparql-mcp.toml`)

| Key | Default | Applies to | Rationale |
|---|---|---|---|
| `max_batch_bytes` | 1 MiB | `ingest_batch`, `load_ontology` | An agent tool call rarely exceeds a few hundred KB, and parse memory stays around 10× the input |
| `max_batch_triples` | 10,000 | `ingest_batch`, `load_ontology` | One transaction of ≤ 10k quads stays small, and so do viewer events |
| `max_update_bytes` | 1 MiB | `update_sparql` | Same as above |
| `max_iri_bytes` | 4 KiB | all writes | Rejects junk IRIs (base64 blobs) |
| `max_literal_bytes` | 64 KiB | all writes | Large documents belong in `write_doc`, not in a literal |
| `max_job_batches` | 10,000 | jobs | Bounds the ledger |
| `max_job_triples_staged` | 500,000 | staged jobs | Commit is one transaction |
| `max_job_triples_direct` | 10,000,000 | direct and file jobs | Per-batch transactions, so the limit is disk, not memory |
| `max_open_jobs` | 4 | jobs | Bounds staging graphs |
| `job_idle_ttl` | 1 h | jobs | Cleans up abandoned staging data |
| `max_file_bytes` | 2 GiB | `ingest_file`, `load_ontology_file` | — |
| `file_batch_triples` | 10,000 | `ingest_file` | — |
| `file_roots` | *(unset)* | `ingest_file`, `load_ontology_file` | See *Compatibility* |
| `event_max_triples` | 5,000 | events | Bounds one broadcast slot |
| `event_changed_threshold` | 50,000 | events | Above it, viewer resync is cheaper than replay |
| `validate_timeout` | 10 s | `ingest_validate` | — |
| `write_wait_timeout` | 30 s | write gate | — |
| `allow_destructive_updates` | `false` | `update_sparql` | See *Guards* |

`ingest_begin` returns the effective limits, so the agent sizes its batches
from data rather than guesses.

### Guards

*   **Protected graphs.** `urn:meta`, `urn:sparql-mcp:jobs`, and the prefixes
    `urn:hkb:meta:` and `urn:sparql-mcp:staging:` can only be written by the
    core's own code. Any tool write that targets them is refused with
    `graph_protected`, including `update_sparql`, whose parsed
    `spargebra::Update` is inspected for every graph it touches.
*   **Destructive updates.** `DROP` and `CLEAR` (any target, including
    `ALL`, `NAMED` and `DEFAULT`), and `MOVE`/`COPY` whose destination is a
    project graph, are refused with `destructive_update_forbidden` unless
    `allow_destructive_updates = true`. A staged `replace` commit is the
    sanctioned way to rewrite a graph.
*   **Target graphs.** A job target must be an existing project graph
    (`urn:project:*`, recorded in `urn:meta`) or the active graph by
    default. Arbitrary new named graphs are refused unless the agent creates
    a project first.
*   **Errors are data.** Refusals return `isError: true` with
    `{"code", "message", "limit"?, "value"?, "line"?, "column"?}`, so the
    agent can react without parsing prose.

### Event tagging fix (F4)

`ground_quads` and `load_rdf_impl` return quads with their graph. Event
emission groups quads by graph and emits one event per graph. The viewer
already ignores events whose `graph` is not the one it shows
(`event_for_another_graph_is_ignored`). This fix therefore removes the
mis-drawn triples without any viewer change.

### Compatibility

*   **Tool outputs.** `triples_before` and `triples_after` in existing tool
    outputs required full scans (F3). They are kept for one release with
    value `null` and are documented as deprecated. `delta` becomes
    `inserted_new`, which is exact and cheap. **This is a visible change**
    for any skill that reads `triples_after`.
*   **`file_roots` unset.**
    *   `ingest_file` is disabled, and its error explains how to enable it.
    *   `load_ontology_file` keeps today's behavior and logs a warning once.
    *   When `file_roots` is set, both tools enforce it.
*   **Limits apply to existing tools at once.** Defaults were chosen above
    any write the current skills produce. `kb-workbench` renders and
    ingests per note, which is far below 1 MiB.

--------------------------------------------------------------------------------

## Agent workflow (guidance for skills)

This text goes into the `sparql-first` and `kb-workbench` skill guidance.

1.  Call `ingest_begin` with `mode: staged` (the default). Use `direct`
    only above about 500k triples or for append-only logs. Read the returned
    limits.
2.  Emit **N-Triples**. It needs no prefix state across batches and is the
    fastest to parse. Aim for 2k–5k triples or at most about 512 KiB per
    batch.
3.  Number batches from 1. On any error or timeout, re-send **the same
    seq** with the same content. That is always safe.
4.  On `parse_error` or `limit_exceeded`, fix or split the batch and re-send
    it under the same seq. Splitting means seq n is re-sent smaller, and the
    remainder becomes new seqs at the end.
5.  Before commit, run `ingest_validate` with checks that match the
    ontology.
6.  Call `ingest_commit`, then `viewer_show` the new top-level subjects if a
    viewer is connected.
7.  Use `update_sparql` for small, targeted edits only. Never use it for
    bulk loads.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: Raise limits on `load_ontology` and keep calling it in a loop

*   **Pros:** No new tools.
*   **Cons:** It gives no idempotency, no resume, no atomic multi-call
    commit, and no validation step. It does not fix blank-node instability.
*   **Rejected:** Most of the problem would remain.

### Alternative B: Oxigraph `BulkLoader` for every batch

*   **Pros:** Fastest raw load.
*   **Cons:** It is non-transactional by design, so a failure leaves a
    partial write, and a ledger record cannot share an atomic write with
    the data. It is built for tens of millions of triples into a mostly
    idle store.
*   **Rejected for batches. Kept as a Future Improvement** for an offline
    `sparql-mcp ingest` CLI path.

### Alternative C: Agent writes one file, then calls `ingest_file`

*   **Pros:** It reuses the streaming path, with a single call.
*   **Cons:** The agent needs write access to a shared path, which is
    awkward across WSL and Windows. A late error is found only after
    everything was produced, and nothing is validated before landing.
*   **Partly adopted:** `ingest_file` exists for dumps. Agent-produced data
    uses batches.

### Alternative D: Staging in a separate in-memory store

*   **Pros:** It keeps the RocksDB store clean of staging data.
*   **Cons:** Staged data would be lost on restart (no resume). Validation
    checks against the target would need to span two stores. Commit would
    cross stores and not be atomic.
*   **Rejected.**

### Alternative E: Per-session job state kept in memory only

*   **Pros:** Simpler.
*   **Cons:** The core is spawned per Claude Code session, and sessions
    restart. Job state would vanish while staging data remained orphaned.
*   **Rejected:** The ledger lives in the store and shares a transaction
    with each batch.

--------------------------------------------------------------------------------

## Non-Functional Requirements

*   **Performance.**
    *   An `ingest_batch` of 5k triples takes ≤ 300 ms end to end.
    *   Sustained N-Triples throughput is ≥ 50k triples/s for file jobs.
    *   Write cost does not grow with store size, since nothing calls
        `len()` on the write path except pattern updates.
    *   All of this is measured with a new `examples/ingest_bench.rs`.
*   **Memory.**
    *   The peak per batch is about 10× `max_batch_bytes`.
    *   A staged commit is bounded by `max_job_triples_staged`.
    *   One broadcast slot holds at most `event_max_triples`.
    *   File jobs stream.
*   **Reliability.**
    *   Each batch is exactly-once (data and ledger in one transaction).
    *   A commit is all-or-nothing.
    *   A crash at any point leaves the job resumable or expirable, never
        half-committed.
*   **Security.**
    *   Inputs are bounded (see *Limits*).
    *   No network loads.
    *   File access is limited to `file_roots`, with canonicalized paths.
    *   Internal graphs are protected, and destructive updates are opt-in.
    *   Validation queries are read-only and time-limited.
    *   Error messages quote at most 200 bytes of input.
*   **Observability.**
    *   `tracing` spans per job and batch (job id, seq, counts, ms).
    *   `ingest_status` with no id lists open jobs.
    *   `stats` gains open jobs and staging triples.

### Future Improvements

*   **SHACL validation**, when a Rust SHACL engine fits the dependency
    budget.
*   **Delete and upsert batches**, for example "replace all triples about
    subject S".
*   **An offline bulk path**: `sparql-mcp ingest <file>` with `BulkLoader`,
    run outside a session.
*   **Job progress events** on the viewer channel: a `job` event kind
    showing batch n/N in the feed.
*   **Per-project quotas**, with triples per project graph.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Delivery phases

| Phase | Scope | Exit criterion |
|---|---|---|
| 0 | Hardening: `Limits` + `[ingest]` config; transactional `load_rdf_impl` (F2); no `len()` on ground writes (F3); per-graph events (F4); split events (F5); real `dry_run` (F6); guards (F7); `WriteGate` + `spawn_blocking` (F9); `file_roots` for `load_ontology_file` (F8) | Each finding has a regression test: a parse error leaves zero quads; a `GRAPH <g>` insert tags its event `g`; `DROP ALL` is refused; an oversized payload is refused. Existing tests pass. |
| 1 | Jobs: begin, batch, commit, abort, status; ledger; skolemization; direct and staged modes; expiry | Integration tests: CUJs 1–4 with an in-memory store; a duplicate seq is a no-op; a conflicting seq is refused; restart resume works on a temp RocksDB store. Bench: 5k-triple batch ≤ 300 ms. |
| 2 | `ingest_validate` + `require_validation` | Checks with each `expect` value; a non-query is rejected; a timeout is enforced. |
| 3 | `ingest_file`: streaming, background, resume, progress | CUJ 5 on a generated 1 GB file: memory stays flat and `query_sparql` stays responsive. Resume after kill. |
| 4 | Docs, skill guidance, viewer tie-in | README, CHANGELOG, skill text; manual CUJ 1 with the viewer and `viewer_show` |

### Target Files to Create/Modify

#### 1. `crates/sparql-mcp-core/src/config.rs` (Modify)

Add `IngestConfig` with the defaults in *Limits*, `Config.ingest`, and
deserialization tests (absent section, partial section, `file_roots`).

#### 2. `crates/sparql-mcp-core/src/application/limits.rs` (Create)

`Limits` built from `IngestConfig`. It has pure checks
(`check_bytes`, `check_term`, …) that return a typed `LimitError`, which is
rendered as the error JSON.

#### 3. `crates/sparql-mcp-core/src/application/write_gate.rs` (Create)

`WriteGate::run(label, closure)`: lock with `write_wait_timeout`, then
`spawn_blocking`, and expose the current holder for `write_busy`.

#### 4. `crates/sparql-mcp-core/src/application/guards.rs` (Create)

`classify_update(&spargebra::Update)` returns the graphs touched and whether
the update is destructive. Also `PROTECTED_GRAPHS` and `is_protected(iri)`.
Unit-tested against each `GraphUpdateOperation` variant.

#### 5. `crates/sparql-mcp-core/src/domain/sparql_store.rs` (Modify)

*   Quads carry their graph: `[String; 4]`, or a `StoredQuad` struct.
*   New methods:
    *   `write_batch(quads, extra_quads) -> BatchWrite { inserted_new, already_present }`;
    *   `graph_size(graph)`;
    *   `run_update_tx(sparql)`;
    *   `stream_file(path, opts, batch, on_batch)`.
*   Remove `len()` from ground paths.

#### 6. `crates/sparql-mcp-core/src/infrastructure/oxigraph.rs` (Modify)

*   Transactional `load_rdf_impl`: parse to a `Vec`, then
    `start_transaction` + `extend` + `commit`.
*   `ground_quads` keeps the graph.
*   Implement the new port methods.
*   Streaming parse from a `BufReader` for file jobs.

#### 7. `crates/sparql-mcp-core/src/application/ingest.rs` (Create)

`IngestService`: job state machine, ULIDs, manifest and batch records as
quads in `urn:sparql-mcp:jobs`, idempotency, skolemization, staging
names, commit SPARQL (`ADD`/`MOVE` + `DROP`), event splitting, expiry
sweeper, and file job runner.

#### 8. `crates/sparql-mcp-core/src/application/tools/ingest.rs` (Create)

Definitions and handlers for the 7 tools. Argument parsing and the error
JSON.

#### 9. `crates/sparql-mcp-core/src/application/tools/{sparql,ontology}.rs` (Modify)

Apply `Limits`, guards and `WriteGate`. Per-graph, split events. Real
`dry_run`. Deprecated fields set to `null`. `file_roots` check.

#### 10. `crates/sparql-mcp-core/src/application/event_sink.rs` (Modify)

`emit_split(graph, triples, limits)`: chunks of `event_max_triples`, or one
`Changed` above `event_changed_threshold`.

#### 11. `crates/sparql-mcp-core/src/mcp/server.rs` and `src/main.rs` (Modify)

Hold `Arc<IngestService>`, `Arc<WriteGate>` and `Limits`. Write tools and
ingest tools are dispatched asynchronously. Start the sweeper task.
`instructions` text gains the batching rule ("bulk data → `ingest_*`").

#### 12. Tests (Create)

`tests/ingest_jobs.rs` (CUJs 1–4, idempotency, limits, guards, resume on
a temp RocksDB store), `tests/write_path_hardening.rs` (F1–F9 regressions),
`tests/ingest_file.rs` (streaming, resume, `file_roots`, symlink escape).

#### 13. `crates/sparql-mcp-core/examples/ingest_bench.rs` (Create)

Generates N-Triples, and measures batch latency and file-job throughput
against a temporary RocksDB store.

#### 14. Docs and skills (Modify)

`README.md` (tools, `[ingest]` block), `sparql-mcp.toml` (commented
defaults), `CHANGELOG.md` (including the deprecated output fields), and skill
guidance under `plugins/` (see *Agent workflow*).

### Verification Plan

*   `cargo build/test/clippy -D warnings/fmt --check` on the workspace.
*   Every finding F1–F9 has a failing-then-passing regression test.
*   `ingest_bench` results recorded in the PR for 5k-triple batches and a
    1 GB file. Peak RSS is captured during the file job.
*   Manual: CUJ 1 with the viewer connected; CUJ 4 by killing the core
    mid-job; CUJ 6 from a Claude Code session.
