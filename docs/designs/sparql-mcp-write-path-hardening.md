# sparql-mcp: Write-Path Hardening

*   **Author:** dminier
*   **Status:** Draft
*   **Date:** 2026-11-18

--------------------------------------------------------------------------------

## Overview

`sparql-mcp-core` is the MCP server that agents use to read and write an
Oxigraph SPARQL store. Every agent write goes through MCP tools
(`update_sparql`, `load_ontology`, `load_ontology_file`, `cbm_load_graph`, …)
or through plugins and CLI subcommands that call the same `SparqlStore` port.

A review of that write path on 2026-11-18 found ten defects (F1–F10 below).
They include unbounded inputs, non-atomic loads, full-store scans on every
write, events tagged with the wrong graph, silently merged blank nodes, and
no protection against `DROP ALL`.

This design fixes them, and only them. It ships first. The batch-ingestion
design ([`sparql-mcp-batch-ingestion.md`](./sparql-mcp-batch-ingestion.md))
builds its jobs on the gate, limits and guards defined here. This split was
decided by the author after the EGM review of the batch-ingestion design
(finding M4).

| Concern | Owned by |
|---|---|
| Side-channel, `StoreEvent` wire format | [`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md) |
| Write gate, limits, guards, event tagging, F1–F10 | this document |
| Ingestion jobs (`ingest_*`) | [`sparql-mcp-batch-ingestion.md`](./sparql-mcp-batch-ingestion.md) |

### Terms

*   **Ground update**: `INSERT DATA`, `DELETE DATA`, and RDF loads. The exact
    quads are known before anything is written.
*   **Pattern update**: `INSERT/DELETE … WHERE`, `CLEAR`, `DROP`, `ADD`,
    `MOVE`, `COPY`. Which quads change is only known by running it.
*   **Write gate**: one process-wide async mutex that serializes every store
    write (see [Write gate](#write-gate)).
*   **Project graph**: a named graph `<urn:project:ID>` registered in
    `<urn:meta>`, or the active graph given on the command line.
*   **Load ledger**: the existing graph `urn:hkb:meta:loaded-files`, which
    records loaded files by SHA-256.
*   Inputs, counts and limits are counted in **triples**. *Quads* only
    appears for store-level writes.

## The Problem

Findings verified in the code and in the Oxigraph 0.5.7 sources on 2026-11-18:

| # | Finding | Where | Risk |
|---|---|---|---|
| F1 | No size limit on `update`, inline `turtle`, or file size. `load_rdf_file` uses `fs::read`. | `tools/sparql.rs`, `tools/ontology.rs`, `oxigraph.rs::load_rdf_file` | Memory exhaustion of the core and the viewer |
| F2 | `load_rdf_impl` calls `Store::insert` per quad, which is one transaction per quad | `oxigraph.rs::load_rdf_impl` | A parse error at quad N leaves quads 1..N-1 written; slow |
| F3 | `Store::len()`, documented as "executes a full scan", runs 2–4 times per write | `update_with_outcome`, `load_rdf_impl`, `load_rdf_file`, `load_ontology`, `sparql.rs::with_delta` (used by `cbm_load_graph` and the recording imports), `project_create`, `export.rs::stats` | Write cost grows with store size |
| F4 | Events carry the **active** graph, whatever graph the data went to. This includes the `graph_iri` argument, `INSERT DATA { GRAPH <g> {…} }`, and the default graph (no `graph_iri`, or `INSERT DATA` without `GRAPH`). `ground_quads` and `quad_to_triple` drop the graph. | `ontology.rs::emit_insert`, `sparql.rs::update_sparql`, `oxigraph.rs::ground_quads` | The viewer draws triples that are not in its graph |
| F5 | One `StoreEvent` carries every triple of a load, through a 1024-slot broadcast | `emit_insert` | Unbounded memory held for a slow viewer |
| F6 | `dry_run` returns before parsing anything | `sparql.rs::update_sparql` | Agents trust a validation that does not happen |
| F7 | No guard on `DROP`/`CLEAR` (including `ALL`), or on writes to `urn:meta` and `urn:hkb:meta:*` | `update_sparql`, `cbm_load_graph` (`graph_iri` is free) | Project registry or load ledger wiped by one call |
| F8 | `load_ontology_file` reads any path the process can read. `export_graph` writes to any path. `cbm_load_graph` opens any DB path. | `ontology.rs`, `export.rs`, `cbm.rs` | Files outside the workspace exposed or overwritten |
| F9 | Store calls are synchronous and run on Tokio worker threads | `server.rs::call_tool` → `dispatch_core` | A long load stalls the MCP transport and the viewer channel |
| F10 | `load_rdf_impl` parses without renaming blank nodes, so `_:b0` in two separate calls becomes one node. Turtle `[]` nodes get fresh random ids, so a retry duplicates them. | `oxigraph.rs::load_rdf_impl` | Silent data corruption |

Oxigraph facts this design relies on (0.5.7, verified 2026-11-18):

*   One SPARQL update request runs as one transaction (`sparql/update.rs`).
*   `Transaction::contains` exists and sees the transaction's own
    uncommitted writes (`store.rs:1279`).
*   A transaction keeps all its changes in memory until `commit`.
*   **RocksDB transactions have no conflict detection.** Two concurrent
    commits are both written, and the last writer wins with no error.
*   `oxhttp` is not in `Cargo.lock`, so `LOAD <http://…>` cannot fetch
    anything.
*   spargebra rewrites `ADD`, `COPY` and `MOVE` into `Drop` and
    `DeleteInsert`. There is no `Add`/`Move`/`Copy` variant to match on.

## Goals

*   **Every store write is serialized and bounded.** One write gate and one
    set of limits cover every writer: tools, plugins and CLI.
*   **Atomic loads.** An RDF load is parsed completely, then written in one
    transaction. A parse error writes nothing.
*   **No whole-store scan on ground writes.** Their cost does not grow with
    store size.
*   **Correct events.** Each event names the graph its triples went to, and no
    event exceeds `event_max_triples`.
*   **Protected internals.** `urn:meta`, the load ledger, and the jobs and
    staging graphs (reserved for batch ingestion) can only be written by core
    code.
*   **Destructive updates are opt-in.** `DROP`/`CLEAR` and their rewritten
    forms are refused unless `[ingest] allow_destructive_updates = true`.
*   **Confined file access** to `file_roots` for reads and writes, with
    Windows-correct path checks.
*   **Stable blank nodes per call.** Two separate calls never share a blank
    node by accident.
*   **The async runtime is never blocked** by store work.

## Non-Goals

*   **Ingestion jobs, staging, validation, file streaming.** These are covered
    by the batch-ingestion design.
*   **Read-path limits.** `query_sparql` keeps its current behavior. Query
    timeouts are a Future Improvement.
*   **Multi-process access.** One core process owns the RocksDB lock, as
    today.

## Critical User Journeys (CUJs)

### CUJ 1: A malformed load leaves nothing behind

*   **When**: An agent calls `load_ontology` with 8,000 triples of Turtle and
    a syntax error on line 5,001.
*   **Then**: The call fails with `parse_error` and a 1-based line and column,
    when the parser provides them. Zero quads were written.

### CUJ 2: Oversized input is refused before parsing

*   **When**: An agent sends a 3 MiB `update_sparql`.
*   **Then**: The call fails with `limit_exceeded`
    (`limit_key: "max_update_bytes"`, `limit: 1048576`, `value: 3145728`).
    Nothing is parsed or written.

### CUJ 3: Destructive and protected writes are refused

*   **When**: An agent calls `update_sparql` with `DROP ALL`, then with
    `DELETE { GRAPH ?g { ?s ?p ?o } } WHERE { GRAPH ?g { ?s ?p ?o } }`, then
    `cbm_load_graph` with `graph_iri: "urn:meta"`.
*   **Then**:
    *   The first two are refused with `destructive_update_forbidden`, and the
        message names `[ingest] allow_destructive_updates`.
    *   The third is refused with `graph_protected`, whatever the setting.

### CUJ 4: The viewer only draws what is in its graph

*   **Given**: The viewer shows `urn:project:P`.
*   **When**: An agent runs `INSERT DATA { GRAPH <urn:project:Q> { … } }`, then
    `load_ontology` without `graph_iri`.
*   **Then**: The first event is tagged `urn:project:Q`, and the viewer
    ignores it. The second load goes to the active graph `urn:project:P` (see
    [Compatibility](#compatibility)) and is drawn.

### CUJ 5: Long writes do not freeze the server

*   **When**: `cbm_load_graph` loads 300,000 triples.
*   **Then**: `query_sparql` calls made meanwhile answer normally, since reads
    take no gate. A second write waits for the gate, and returns `write_busy`
    after `write_wait_timeout`.

--------------------------------------------------------------------------------

## Technical Plan (Architecture)

### Gated store

The gate and the protected-graph check are store-wide invariants. A check
inside one tool cannot stop a plugin or another tool. So they are enforced in
a decorator around the port, not per tool.

```text
tools / plugins / CLI ──▶ GatedStore (impl SparqlStore) ──▶ OxigraphAdapter
                              │  write methods: WriteGate + protected-graph check
                              │  read methods: pass through
core internals (registry, load ledger, ingestion jobs) ──▶ PrivilegedStore (gate only)
```

*   `GatedStore` wraps `Arc<dyn SparqlStore>`. Every write method takes the
    gate. In Phase 0a it provides the gate only; the protected-graph and
    destructive checks are added in Phase 0c, once the update classifier
    exists. The methods are classified as follows:

    | Port method | Class in `GatedStore` |
    |---|---|
    | `update`, `update_with_outcome`, `load_rdf`, `load_rdf_with_triples`, `load_rdf_file`, `load_ontology_dir` | gated write |
    | `query`, `export_graph` (a store read; the file check is in the tool), `list_graphs`, `triple_count`, `graph_size` (added in 0b) | pass-through |

    Methods added later (the batch-ingestion `write_batch`, `commit_job` and
    `stream_file`) are added to this table by the change that adds them.
*   `PrivilegedStore` takes the gate but skips the protected-graph and
    destructive checks. Only core modules that own those graphs hold it.
*   **Checks by handle.** From Phase 0c, `GatedStore` applies the
    protected-graph and destructive checks for every caller, plugins
    included. `PrivilegedStore` skips both. `reserved_iri` is not checked in
    either handle: it is checked when an agent payload is parsed
    (`limits::check_term`, and the batch-ingestion `parse_batch`, before
    skolemization), so job-generated skolem IRIs are never refused.
*   **Wiring.**
    *   `pub struct GatedStore { inner: Arc<dyn SparqlStore>, gate: WriteGate, guards: GuardConfig }`
        and `pub(crate) struct PrivilegedStore { inner: Arc<dyn SparqlStore>, gate: WriteGate }`
        share one `WriteGate`. `main.rs` builds both from one
        `OxigraphAdapter`.
    *   `SparqlMcpServer` gains `privileged: Arc<dyn SparqlStore>`. The
        `project_create`, `import_recording_*` and `materialize_recording`
        arms, and `mcp/importer.rs`, receive the privileged handle. Every other
        arm and `plugin_ctx()` receive the gated handle. `PluginContext.store`
        keeps its type and never holds the privileged handle.
*   **Multi-call sections.** Some tools make several store calls that must
    not be split: recording imports run `CLEAR` then two loads, and
    `with_delta` counts, writes and counts again.
    `PrivilegedStore::exclusive<T>(&self, label, f: impl FnOnce(&dyn SparqlStore) -> anyhow::Result<T>)`
    holds the gate once and passes the ungated inner store. Recording imports
    and `with_delta` (until 0b removes its counts) use it.
*   Limits are checked at the tool boundary, before any parse, because only
    the tool knows which input is the payload.

| Writer | Gated | Limits | Protected-graph check |
|---|---|---|---|
| `update_sparql`, `load_ontology`, `load_ontology_file` | yes | `[ingest]` | yes |
| `cbm_load_graph` | yes | `max_cbm_triples`; chunks of `file_batch_triples` (see [Atomic, scan-free loads](#atomic-scan-free-loads-f2-f3-f10)) | yes (`graph_iri`) |
| recording imports, `project_create`, `mcp/importer.rs` | yes | none (core-generated data) | privileged |
| `ToolPlugin`s | yes | the plugin's own | yes |
| CLI `load-file`, `code-import`, `import` | yes (for uniformity; the RocksDB lock already excludes a concurrent `serve`) | none | yes |
| startup `load_ontology_dir`, CLI `reload-ontology` | yes | none | privileged |
| `ingest_*` (batch-ingestion design) | yes | `[ingest]` | yes; jobs code is privileged for its own graphs |

### Write gate

*   **What it protects.** Oxigraph transactions on RocksDB are not isolated
    from one another. Concurrent commits are written as-is, and the last
    writer wins with no error. The gate is therefore the only thing that
    keeps new-versus-duplicate counts exact and stops ledger updates from
    being lost.
*   **Shape.** `WriteGate` is `Clone` and wraps
    `Arc<tokio::sync::Mutex<GateHolder>>`, with
    `GateHolder { label: &'static str, since: Instant }`. It has two entry
    points:
    *   `fn run_blocking<T>(&self, label: &'static str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T>`
        takes the lock with
        `Handle::current().block_on(tokio::time::timeout(wait, mutex.clone().lock_owned()))`
        and runs `f` on the current thread. The write methods of
        `GatedStore` and `PrivilegedStore` use it.
    *   `async fn run<T: Send + 'static>(&self, label: &'static str, f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T>`
        takes the lock, then moves the owned guard into `spawn_blocking`.
        It is for callers that hold no store handle.

    If the waiting time runs out, both return `ToolError::WriteBusy` with the
    label of the current holder.
*   **Calling rule.** Gated and privileged store methods are only called from
    a blocking thread, because `block_on` panics on an async worker.
    `dispatch_core`, plugin dispatch and every CLI write wrap their call in
    `tokio::task::spawn_blocking`. The gate is not reentrant, so
    `WriteGate::run` and `run_blocking` are never wrapped around a
    `GatedStore` or `PrivilegedStore` call: that would deadlock.
*   **Cancellation.** The guard lives on the blocking thread that runs the
    store work, so dropping the request future does not release the gate. It
    is released only when the store work ends.
*   **Reads take no gate.** `query_sparql` reads an Oxigraph snapshot, so it is
    never blocked by writes. It still runs in `spawn_blocking` (see F9).
*   **Tests.** Gate and cancellation tests run on a temporary RocksDB store.
    The in-memory store serializes writers on its own and would hide a
    missing gate.

### Off the async runtime (F9)

Every store-touching tool runs in `spawn_blocking`, reads included. Writes
also take the gate, on the blocking thread (see the calling rule above).

*   **Signature.** `pub(crate) async fn dispatch_core(&self, name: &str, args: &JsonObject) -> Option<Result<CallToolResult, McpError>>`.
*   **Arms.** Each store-touching arm clones the `Arc`s and `args`, then
    awaits `spawn_blocking(move || tool(&store, &args, &active_graph, sink.as_deref()))`.
    This satisfies the `'static` requirement of `spawn_blocking`. A
    `JoinError` maps to `McpError::internal_error`. `project_switch` and
    `get_gdrive_config` stay inline.
*   **Plugins.** Plugin dispatch runs in one `spawn_blocking`, with
    `plugins: Arc<Vec<…>>` and the `PluginContext` moved in.
*   **No semaphore.** The blocking pool is not bounded further: one stdio
    client sends few concurrent calls, and writes are serialized by the gate.
*   **Test.** `every_declared_tool_has_a_dispatch_arm` becomes a
    `#[tokio::test]`.

### Atomic, scan-free loads (F2, F3, F10)

*   **Loads.** `load_rdf_impl` parses the whole payload into a `Vec` of quads,
    applying term limits (see [Limits](#limits)) and
    `RdfParser::rename_blank_nodes()` (F10). Only then does it open one
    transaction. In that transaction, for each quad, `contains` decides
    between `inserted_new` and `already_present`, then `insert`. Finally it
    commits.
*   **Ground updates.** `update_with_outcome` keeps parsing with `spargebra`
    to classify the request. For a ground request it runs the update inside a
    `Transaction` and counts with `contains` before the update, so no `len()`
    is needed.
*   **Pattern updates** keep their two `len()` calls. Their effect is unknown
    without them, and they are rare and small by nature.
*   **Other `len()` sites.** `with_delta` gets the same per-quad counting as
    loads. `project_create` drops its count. `stats` keeps `len()`, which is
    expected for a statistics tool.
*   **`load_ontology_file`** reads through a `BufReader` capped at
    `max_load_file_bytes`. It is one transaction, so larger files must go
    through `ingest_file` (batch-ingestion design).
*   **Load ledger.** The subject is
    `<urn:hkb:meta:file:{sha256}:{sha256 of the graph IRI}>`, with the existing
    predicates plus `meta:graph <g>`. It is written **in the data
    transaction**; today it is written in a separate one.
    *   Old hash-only entries are ignored, so each file is reloaded once,
        which is idempotent.
    *   When an allowed `DROP` or `CLEAR` of `<g>` runs, `update_with_outcome`
        appends
        `DELETE WHERE { GRAPH <urn:hkb:meta:loaded-files> { ?f meta:graph <g> ; ?p ?o } }`
        to the same request. `DROP ALL` and `CLEAR ALL` clear the whole
        ledger.
*   **`cbm_load_graph`** (Phase 0b). It parses the generated Turtle into
    `Vec<StoredQuad>`, then writes chunks of `file_batch_triples`.
    *   Each chunk is its own gated transaction, and the gate is released
        between chunks.
    *   It receives the event sink, and emits through `emit_split`. Today it
        emits nothing, which breaks the `AGENTS.md` invariant that every write
        to the active graph emits a `StoreEvent`.
    *   The cap is `max_cbm_triples` (default 1,000,000).

### Correct, bounded events (F4, F5)

*   `ground_quads` and `quad_to_triple` keep each quad's graph (`StoredQuad
    { s, p, o, g: Option<String> }`).
*   Event emission groups quads by graph and emits one event per graph. Events
    for the default graph use `graph = ""`, which the viewer never shows.
*   Each graph's triples are split into chunks of `event_max_triples`. Above
    `event_changed_threshold` triples, one `Changed` event is emitted instead.
*   `Changed.triples_before` and `triples_after` stay `u64`, as the viewer
    reads them. They are filled with a `COUNT` of the target graph only,
    before and after the write. That count is computed only on the `Changed`
    path.
*   **Pattern updates.** One `Changed` per constant graph from
    `classify_update`, with `graph_size` before and after. For an all-graphs
    update, one `Changed { graph: <active graph> }` with the store-wide `len()`
    values that pattern updates already compute, so the viewer resyncs.

### Guards (F7)

*   **Protected graphs.** `urn:meta`, `urn:sparql-mcp:jobs`, and the prefixes
    `urn:hkb:meta:` and `urn:sparql-mcp:staging:` are refused to every
    non-privileged writer with `graph_protected`. Payload IRIs starting with
    `urn:sparql-mcp:` are refused with `reserved_iri`.
*   **Classifying an update.** `guards::classify_update(&spargebra::Update)`
    returns the graphs touched and whether the update is destructive. It works
    on the operations as spargebra rewrites them:

| Operation (after rewriting) | Graphs touched | Destructive | Refused |
|---|---|---|---|
| `InsertData` / `DeleteData` | each quad's graph | no | protected graph: always |
| `DeleteInsert` with constant template graphs | the template graphs | only if a delete template is fully variable (`?s ?p ?o`) | protected: always; destructive: unless allowed |
| `DeleteInsert` with any `GRAPH ?g` template, or a `WITH`/`USING` that leaves the graph variable | **all graphs** | yes | unless allowed; always if the WHERE clause could bind a protected graph |
| `Drop` / `Clear` (graph, `DEFAULT`, `NAMED`, `ALL`); also `ADD`/`MOVE`/`COPY` after rewriting | the target | yes | unless allowed; protected: always |
| `Create` | the target | no | protected: always |
| `Load` (also `SILENT`) | the target | n/a | always (`load_forbidden`) |

*   **Rules.**
    *   A delete template is *fully variable* when its subject, predicate and
        object are all variables. `DELETE { ?s ?p ?o } WHERE { ?s a :X }` is
        destructive, whatever its `WHERE` clause.
    *   A template graph is *variable* when it is `GRAPH ?g`, or when it has no
        `GRAPH`, no `WITH` fixes it, and `USING NAMED` is present.
    *   A variable graph counts as touching every graph, protected ones
        included. It is refused unless `allow_destructive_updates` is set.
        Even then, it is refused when the update has a delete template, unless
        `?g` is bound to constants by a top-level `VALUES` or by
        `FILTER(?g = <iri>)`.
    *   The classifier never tries to decide statically whether a `WHERE`
        clause could bind a protected graph. A variable graph is refused or
        allowed as a whole.
*   **Default graph.** The default graph counts as a project graph for this
    check.
*   **Test coverage.** Unit tests cover each row, including `ADD`, `COPY` and
    `MOVE` in their rewritten form, `WITH`, `GRAPH ?g` and `VALUES`-bound
    graphs.

### File access (F8)

*   **Config.** `file_roots` is a `Vec<PathBuf>`, for example
    `file_roots = ['C:\data\rdf', 'D:/dumps']` (TOML literal strings, or
    forward slashes). Each root is canonicalized at startup, and a missing root
    is a startup error.
*   **Checks, Windows-first.**
    *   UNC (`\\host\…`), device (`\\.\…`, `\\?\` with a device), and
        alternate-data-stream paths (a `:` after the drive) are rejected
        before anything is opened.
    *   The file is then opened, and its final path is read from the handle
        (`GetFinalPathNameByHandleW`). That path is checked against the
        roots component-wise and case-insensitively.
    *   Reading or writing uses the same handle, so there is no gap between
        the check and the use.
    *   The symlink-escape test uses a directory junction, which needs no
        elevation.
*   **Scope.** `load_ontology_file` and `export_graph` (output path) are
    checked, and so is the `cbm_db_path` of `cbm_load_graph`.
*   **Unset `file_roots`.** In this release the old behavior stays, with a
    warning logged once per tool. The next release defaults `file_roots` to
    the workspace directory and enforces it.

### Real `dry_run` (F6)

`dry_run: true` parses the update with `spargebra`, runs the guard, and returns
`{"dry_run": true, "operations": ["insert_data", "drop", …], "graphs": [...],
"destructive": bool, "refused": null | {"code", "message"}}`. Invalid SPARQL
returns `parse_error`.

### Limits

`[ingest]` in `sparql-mcp.toml`. The section is optional, and absent keys take
the defaults below. Sizes are integer bytes. Durations are integer seconds,
with a `_secs` suffix. This design uses the keys below. The batch-ingestion
design adds its own job keys to the same section.

| Key | Default | Applies to | Rationale |
|---|---|---|---|
| `max_batch_bytes` | 1,048,576 | `load_ontology` (and `ingest_batch`) | An agent tool call rarely exceeds a few hundred KiB. Parse memory is estimated at 10× the input, confirmed by a bench in Phase 0. |
| `max_batch_triples` | 10,000 | `load_ontology` (and `ingest_batch`) | One transaction of at most 10,000 triples stays small, and so do events |
| `max_update_bytes` | 1,048,576 | `update_sparql` | Same bound as `max_batch_bytes` |
| `max_iri_bytes` | 4,096 | every ground write | Rejects junk IRIs (base64 blobs) |
| `max_literal_bytes` | 65,536 | every ground write | Large documents belong in `write_doc` |
| `max_load_file_bytes` | 67,108,864 | `load_ontology_file` | One transaction; larger files go through `ingest_file` |
| `file_batch_triples` | 10,000 | `cbm_load_graph` chunks | Matches `max_batch_triples` |
| `file_roots` | unset | file reads and writes | See [File access](#file-access-f8) |
| `event_max_triples` | 5,000 | events | Bounds one broadcast slot |
| `event_changed_threshold` | 50,000 | events | Above it, a viewer resync costs less than replaying the events |
| `write_wait_timeout_secs` | 30 | write gate | Longer than one 10,000-triple transaction |
| `allow_destructive_updates` | `false` | `GatedStore` (all non-privileged writers) | See [Guards](#guards-f7) |
| `max_cbm_triples` | 1,000,000 | `cbm_load_graph` | A generated code graph is far smaller; bounds one tool call |

*   **Term size.** Term size is the UTF-8 byte length of the IRI or of the
    literal's lexical form. Datatype IRIs and language tags are checked as
    IRIs. `update_sparql` checks terms in `INSERT DATA`/`DELETE DATA` only.
*   **Errors are data.** Every refusal returns `isError: true` with
    `{"code", "message", "limit_key"?, "limit"?, "value"?, "line"?,
    "column"?, "holder"?}`. Messages quote at most 200 bytes of input, cut on
    a char boundary by `limits::quote_input`.
*   **Error codes:** `limit_exceeded`, `parse_error`, `graph_protected`,
    `reserved_iri`, `destructive_update_forbidden`, `load_forbidden`,
    `path_outside_roots`, `path_rejected`, `write_busy`.
*   **Error channel.** Tools turn store errors into `McpError` today, so a
    refusal raised below the tool would surface as a JSON-RPC error string.
    `limits.rs` therefore defines
    `pub enum ToolError { LimitExceeded{..}, ParseError{..}, GraphProtected{graph}, ReservedIri{iri}, DestructiveUpdateForbidden, LoadForbidden, PathOutsideRoots{path}, PathRejected{path}, WriteBusy{holder: String, waited_secs: u64} }`.
    *   Store-level refusals are returned as `anyhow::Error::new(ToolError::…)`.
    *   Each tool's error mapping first tries `e.downcast_ref::<ToolError>()`.
        On a match, it returns `Ok(CallToolResult { is_error: Some(true), content: [json] })`.
    *   Missing or ill-typed arguments stay `McpError::invalid_params`.
    *   `holder` is the `GateHolder.label`, which is the tool or method name.
    *   `ToolError` exists from Phase 0a, because `write_busy` needs it.

### Compatibility

*   **Tool outputs.**
    *   For ground writes, `triples_before` and `triples_after` become
        `null`. They are deprecated for one release, then removed.
    *   `delta` is kept, and equals `inserted_new - removed_existing`. Those
        two fields are new; both are exact and cheap.
    *   Pattern updates keep real before and after values.
    *   **This is a visible change** for skills. `kb-workbench` reads
        `triples_before`, `triples_after` and `delta` after
        `cbm_load_graph`, so its `SKILL.md` is updated in the same change.
*   **`load_ontology` without `graph_iri`.** It now loads into the active
    graph, the same as the CLI `load-file`. Before, it loaded into the default
    graph, which no viewer showed.
*   **Limits apply to existing tools at once.** The defaults are above any
    write the current skills produce. Code import goes through
    `cbm_load_graph`, which keeps its own cap and chunked transactions.
*   **Errors move from JSON-RPC to results.** Refusals and SPARQL parse
    errors now return `isError: true` results instead of JSON-RPC errors.
    Argument errors stay JSON-RPC errors.
*   **`dry_run`** keeps `"status": "ok"`.
*   **Oversized `load_ontology_file`** returns `limit_exceeded` with
    `limit_key: "max_load_file_bytes"`.
*   **Tests updated.** `tests/tools_sparql.rs` (lines 122-123 and 145-146) and
    `tests/store.rs` (lines 225-231) assert the old counts.

--------------------------------------------------------------------------------

## Alternatives Considered

### Alternative A: Per-tool checks instead of a gated store

*   **Pros:** Smaller change.
*   **Cons:** Plugins and the CLI would still write ungated. Every new tool
    would have to remember the checks.
*   **Rejected:** The gate and the guards are store-wide invariants.

### Alternative B: Rely on Oxigraph transactions for isolation

*   **Cons:** RocksDB transactions in Oxigraph do not detect conflicts (see
    [The Problem](#the-problem)).
*   **Rejected.**

### Alternative C: Per-graph locks

*   **Pros:** Writes to different graphs could run in parallel.
*   **Cons:** It needs lock ordering, and `GRAPH ?g` updates touch every
    graph. RocksDB serializes the final writes anyway.
*   **Rejected:** Not worth it for one local process.

--------------------------------------------------------------------------------

## Risks and Open Questions

| Risk / question | Impact | Mitigation |
|---|---|---|
| A long write holds the gate | Other writes get `write_busy` | Chunked transactions for `cbm_load_graph` and `ingest_*`; `write_wait_timeout` |
| A skill reads the deprecated fields | Wrong numbers in a skill | One-release deprecation; `kb-workbench` updated in the same change |
| `GetFinalPathNameByHandleW` edge cases (network drives, subst) | False refusal | Refusal names the resolved path; roots are canonicalized the same way |
| Resolved: one update request is one transaction | — | Verified in `sparql/update.rs` on 2026-11-18 |

## Future Improvements

*   **Query timeouts** for `query_sparql`, with `SparqlEvaluator`'s
    cancellation token.
*   **Enforce `file_roots` by default.** Next release, defaulting to the
    workspace directory.

--------------------------------------------------------------------------------

## Detailed Implementation Plan

### Required changes to the companion designs

*   **[`interface-agentic-rust-stylee.md`](./interface-agentic-rust-stylee.md).**
    *   Events carry the graph the data went to. `graph = ""` stands for the
        default graph.
    *   A write splits into chunks of `event_max_triples`, or becomes one
        `Changed` above `event_changed_threshold`.
    *   `Changed` counts come from a per-graph `COUNT`.
*   **[`sparql-mcp-batch-ingestion.md`](./sparql-mcp-batch-ingestion.md).**
    *   Job writes call `PrivilegedStore` methods or
        `PrivilegedStore::exclusive`. They never wrap `WriteGate::run` around a
        store call, because the gate is not reentrant.
    *   Skolem IRIs are produced after the `reserved_iri` check (see
        [Guards](#guards-f7)).
    *   The oversized-file error uses `limit_exceeded` with
        `limit_key: "max_file_bytes"`, not `file_too_large`.

### Delivery phases

Each phase starts only when the previous phase's exit criterion passes, and
each phase leaves the workspace building with all tests green. The test files
use temporary RocksDB stores where concurrency matters, because the in-memory
store serializes writers by itself.

| Phase | Scope | Files | Exit criterion |
|---|---|---|---|
| 0a | `WriteGate` with `run` and `run_blocking`; `GatedStore` (gate only) and `PrivilegedStore`, with their wiring; `ToolError` and the error channel; `spawn_blocking` everywhere; async `dispatch_core` | 1, 2 (`write_wait_timeout_secs`), 3 (`ToolError`), 4, 5, 9, 10, 12 | In `tests/write_path_hardening.rs`, on `#[tokio::test(flavor = "current_thread")]`, with a `SlowStore` double wrapping `OxigraphAdapter::open(tempdir)` whose `update` sleeps 2 s: (a) `dispatch_core("query_sparql")` completes within 200 ms while `dispatch_core("update_sparql")` is running; (b) two concurrent writes never overlap, with an `AtomicUsize` of concurrent entries that peaks at 1; (c) the first write's future is dropped by `timeout(50 ms)`, then a second write with `write_wait_timeout_secs = 1` gets `write_busy` with holder `update_sparql`, and succeeds after the first write ends. All existing tests pass. |
| 0b | Atomic scan-free loads, `StoredQuad` and the new outcome types, per-graph split events, blank-node renaming, load-ledger key and cleanup, chunked `cbm_load_graph` with events | 6, 7, 10, 11, 12 (`tools_sparql.rs`, `store.rs`, bench) | CUJs 1 and 4. F3: a `#[cfg(test)]` counter behind a private `OxigraphAdapter::store_len()`, which replaces every `self.store.len()`, stays at 0 across `load_rdf`, `INSERT DATA` and `DELETE DATA`. F10: `_:b0` in two calls gives two nodes. CUJ 5: a `query_sparql` and a second write both complete while a 300,000-triple `cbm_load_graph` runs. The tests listed under [Compatibility](#compatibility) are updated in this phase. |
| 0c | Limits, `GatedStore` guard checks, `classify_update`, real `dry_run`, `file_roots` | 1, 2, 3, 5 (guards), 7, 8, 9, 10, 12 | CUJs 2 and 3. Each row of the guard table, and each of the *Rules*. F6: `dry_run` on invalid SPARQL returns `parse_error`. F8: a path outside `file_roots` and a junction escape are refused. |
| 0d | Docs, skills, CHANGELOG | 12 (docs) | `kb-workbench` SKILL.md uses the new fields. Every existing test passes. |

### Target Files to Create/Modify

Bullets are tagged with the phase that makes the change.

#### 1. `crates/sparql-mcp-core/Cargo.toml` (Modify)

*   [0a] Add `"time"` to the tokio features in `[dependencies]`.
*   [0c] Add `windows-sys` (feature `Win32_Storage_FileSystem`), for
    `GetFinalPathNameByHandleW`, under `[target.'cfg(windows)'.dependencies]`.

#### 2. `crates/sparql-mcp-core/src/config.rs` (Modify)

*   [0a] `IngestConfig` with `write_wait_timeout_secs`, and
    `#[serde(default)] pub ingest: IngestConfig`.
*   [0c] The remaining keys and defaults of [Limits](#limits).
*   Tests: section absent, section partial, `file_roots` with a Windows path.

#### 3. `crates/sparql-mcp-core/src/application/limits.rs` (Create)

*   [0a] `ToolError`, and its rendering into the error JSON.
*   [0c] `Limits::from(&IngestConfig)`, with `Limits: Copy`. Pure checks
    (`check_bytes`, `check_term`, `check_path`) returning `ToolError`, and
    `quote_input`.

#### 4. `crates/sparql-mcp-core/src/application/write_gate.rs` (Create)

[0a] `WriteGate` with `run`, `run_blocking`, the timeout, and the current
holder for `write_busy`.

#### 5. `crates/sparql-mcp-core/src/application/gated_store.rs` and `application/guards.rs` (Create)

*   [0a] `GatedStore` (gate only), `PrivilegedStore` with `exclusive`, both
    implementing `SparqlStore` and classified as in the table under
    [Gated store](#gated-store).
*   [0c] `guards.rs`: `classify_update`, `PROTECTED_GRAPHS`, `is_protected`,
    `is_reserved_iri`, and the unit tests from [Guards](#guards-f7).
    `GatedStore.update*` parses with spargebra and calls `classify_update`.
    The load methods check `opts.graph_iri`, or the default graph when it is
    `None`.

#### 6. `crates/sparql-mcp-core/src/domain/sparql_store.rs` (Modify)

[0b] These changes:

*   `#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub struct StoredQuad { s: String, p: String, o: String, g: Option<String> }`.
*   `UpdateOutcome::Ground { inserted: Vec<StoredQuad>, deleted: Vec<StoredQuad>, inserted_new: u64, removed_existing: u64 }`.
*   `triples_before()` and `triples_after()` return `Option<u64>`, `None` for
    ground updates.
*   `FileLoadResult` gains `inserted_new`, and `delta()` returns
    `inserted_new as i64`.
*   `LoadOpts` gains `limits: Option<Limits>`, with `Limits: Copy`, so
    `LoadOpts` stays `Clone + Default` with no lifetime.
*   `fn graph_size(&self, graph: &str) -> anyhow::Result<u64>`.

#### 7. `crates/sparql-mcp-core/src/infrastructure/oxigraph.rs` (Modify)

*   [0b] Parse first, then one transaction with `contains` counting.
*   [0b] `rename_blank_nodes()`.
*   [0b] `ground_quads` keeps the graph.
*   [0b] The ground update runs in a `Transaction`.
*   [0b] A private `store_len()` replaces every `self.store.len()`, with a
    `#[cfg(test)]` call counter.
*   [0b] `BufReader` with a cap.
*   [0b] The load ledger follows [Atomic, scan-free loads](#atomic-scan-free-loads-f2-f3-f10),
    including the cleanup appended to `DROP`/`CLEAR`.
*   [0b] `graph_size` through a `COUNT` query.
*   [0c] `update_with_outcome` calls the guards' classification for the
    cleanup and the events.

#### 8. Path checks (Create)

[0c] `crates/sparql-mcp-core/src/application/path_guard.rs` holds the
Windows-first checks of [File access](#file-access-f8).

#### 9. `crates/sparql-mcp-core/src/mcp/server.rs`, `src/plugin/mod.rs`, `src/main.rs` (Modify)

*   [0a] `SparqlMcpServer::new(gated, privileged, doc_store, ontology_dir, active_graph, gdrive_config, store_path)`.
    The test builder builds both handles from one in-memory adapter.
*   [0a] `PluginContext` holds the gated handle.
*   [0a] `spawn_viewer_channel` keeps the raw adapter, because it only reads.
*   [0a] The CLI builds `WriteGate::new(Duration::from_secs(cfg.ingest.write_wait_timeout_secs))`.
    `load-file`, `code-import` and `import` use `GatedStore`.
    `load_ontology_dir` and `reload-ontology` use `PrivilegedStore`. Each call
    is made inside `spawn_blocking`.
*   [0a] `async fn dispatch_core`, as in [Off the async runtime](#off-the-async-runtime-f9).
*   [0c] The `instructions` text gains one sentence on limits.

#### 10. `crates/sparql-mcp-core/src/application/tools/{sparql,ontology,cbm,export,project,recording}.rs` and `src/mcp/importer.rs` (Modify)

*   [0a] The `ToolError` mapping in every tool, and the handle each tool
    receives (see *Wiring*). `project.rs`, `recording.rs` and
    `mcp/importer.rs` use the privileged handle, and recording imports use
    `PrivilegedStore::exclusive`.
*   [0b] Per-graph split events through `event_sink::emit_split`. Deprecated
    fields set to `null`. `with_delta` without `len()`.
*   [0b] `cbm_load_graph` parses the generated Turtle into `Vec<StoredQuad>`,
    then writes chunks of `file_batch_triples`, each its own gated
    transaction, with events (see [Atomic, scan-free loads](#atomic-scan-free-loads-f2-f3-f10)).
*   [0c] Limits at the tool boundary, a real `dry_run`, and the `file_roots`
    checks in `ontology.rs`, `export.rs` and `cbm.rs`.

#### 11. `crates/sparql-mcp-core/src/application/event_sink.rs` (Modify)

[0b] `emit_split(graph, triples, limits, count_fn)`: chunks of
`event_max_triples`, or one `Changed` above `event_changed_threshold`, with
per-graph counts.

#### 12. Tests, bench and docs

*   [0a] `crates/sparql-mcp-core/tests/write_path_hardening.rs` (Create): the
    `SlowStore` double and the 0a exit tests. Later phases add one regression
    test per finding F1–F10, and CUJs 1–5.
*   [0b] `crates/sparql-mcp-core/tests/tools_sparql.rs` and `tests/store.rs`
    (Modify), as listed under [Compatibility](#compatibility).
*   [0b] `crates/sparql-mcp-core/examples/ingest_bench.rs` (Create): parse
    memory per input byte, and the cost of a ground write on a 1M-triple store
    versus an empty one.
*   [0d] `README.md`, `sparql-mcp.toml` (commented `[ingest]` defaults),
    `CHANGELOG.md`, `plugins/kb-workbench/skills/kb-workbench/SKILL.md`, and
    `examples/hello-plugin/src/lib.rs` (docs).

### Verification Plan

*   Workspace checks: `cargo build --workspace`, `cargo test --workspace`,
    `cargo clippy --workspace --all-features -- -D warnings`,
    `cargo fmt --check`. The `recording` feature gates tools that this design
    touches, so CI builds with `--all-features`.
*   One failing-then-passing regression test per finding.
*   Bench (`examples/ingest_bench.rs`, shared with the batch-ingestion
    design): the cost of a ground write must not grow with store size.
*   Manual: CUJ 3 from a Claude Code session, and CUJ 4 with the viewer
    connected.
*   **Citation and carry-over checks.** The review of this design is partial:
    the claim-by-claim check of cited paths, lines and APIs did not run, and
    neither did the carry-over audit against the batch-ingestion review. The
    author chose to make these checks while implementing. Each phase's author
    verifies the code references in the files it touches, and corrects the
    design in the same commit when one is wrong.
