# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] — Unreleased

Initial public cut. Extracted from a larger private monorepo and scrubbed
of domain-specific content.

### Added
- `sparql-mcp` server crate (Oxigraph-backed SPARQL 1.1 MCP server, STDIO).
- `sparql-mcp install` subcommand — auto-patches MCP entries in detected
  agent configs (Claude Code, Codex CLI, Gemini CLI).
- One-line installer script (`install.sh`) — downloads the right static
  binary, verifies SHA-256, drops into `~/.local/bin`, runs `install`.
- GitHub Actions `release.yml` — matrix build for 5 targets
  (darwin/linux arm64+amd64, windows amd64), attaches `SHA256SUMS`.
- `server.json` — MCP registry manifest.
- Core `smc:` ontology (`ontology/1-smc.ttl`).
- `kb-workbench` Claude Code plugin and skill.
- CI: fmt, clippy (-D warnings), tests.
- Viewer side-channel in the server (`[viewer]` config section, off by
  default): a loopback TCP listener that sends a snapshot of the active
  project graph on connect, then one event per committed write
  (`EventSink` / `StoreEvent`). `project_switch` now changes the active
  graph for the running server.
- `sparql-mcp-viewer` crate — optional native Windows app that renders the
  active graph in 3D (Bevy) and grows it live: class/instance styling,
  Barnes-Hut layout, last-batch highlight, activity feed, node inspection,
  class filter, and a `--headless` smoke-test mode.
- `viewer_load` example — load generator for the viewer, no agent needed.

### Fixed
- **Turtle `[]` nodes in an ingestion job.** Oxigraph writes the random id of a generated blank node
  without leading zeros, so about 6 in 100 have 31 digits, and the job took them for labels the agent
  had written (so `blank_nodes: reject` could refuse a valid Turtle batch). Generated labels are now
  recognised at 28 to 32 digits.
- Idempotent file loading (`load_ontology_file`) failed on any Windows path:
  the path was embedded unescaped in a Turtle literal, and `\` is a Turtle
  escape character.

### Added
- **Batch ingestion jobs** (`ingest_begin`, `ingest_batch`, `ingest_commit`,
  `ingest_abort`, `ingest_status`). A job groups the numbered batches of one
  load: a batch is parsed whole and written, with its record, in one
  transaction that re-checks the job is open and the batch is new, so a retry
  (even one that overlaps the original) is stored once and answered with the
  original counts. Staged jobs are invisible until `ingest_commit` moves them into the
  target in one transaction (`replace` swaps the target's contents; an empty
  replace needs `allow_empty_replace`); direct jobs write the target as they go.
  Labelled blank nodes are the same node across a job's batches. Job state lives
  in the protected graph `urn:sparql-mcp:jobs`, so it survives a restart. New
  `[ingest]` keys: `max_open_jobs`, `max_job_batches`, `max_job_triples_staged`,
  `max_job_triples_direct`, `max_replace_triples`, `max_batch_triples`,
  `max_iri_bytes`, `max_literal_bytes`, `event_max_triples`,
  `event_changed_threshold`, and others (see `sparql-mcp.toml`).
  The ceilings come from a benchmark (`docs/benchmarks/ingest.md`): a staged job holds
  at most 100,000 triples (its commit is one in-memory transaction that holds the write
  gate), a replace 200,000; larger loads use several jobs or `mode: "direct"`.
  Idle jobs expire (`job_idle_ttl_secs`, staging graph dropped), finished ones are purged
  after `job_retention_secs`, and time with the core off does not count as idle
  (`[ingest] sweep_interval_secs`); `ingest_status` shows `idle_secs` and `expires_in_secs`.
  `ingest_validate` runs named read-only `SELECT`/`ASK` checks (`{{staging}}`, `{{target}}`;
  cancelled after `validate_timeout_secs`; at most 16 checks, 20 rows each) and records the
  outcome for `ingest_commit` with `require_validation`.
  `ingest_file` streams a server-side N-Triples file (inside `file_roots`) in the background,
  with a checkpoint per batch, `resume_job` after a restart, `already_loaded` / `force`, and
  progress in `ingest_status`; `ingest_abort` stops a running one; the store is flushed when the
  job ends, so the memory of the load is given back. Measured at 26-39k triples/s
  (`docs/benchmarks/ingest.md`). `[ingest] file_batch_triples` (5000). Turtle files are not
  streamed.
- **Viewer control, phase 1.** With `[viewer]` enabled the core lists `viewer_status`, which asks
  the connected viewer (also a headless one) for its graph and node and edge counts, and answers
  `{"viewer":"not_connected","applied":false}` when none is open. Underneath: a `ViewerHub` that
  tracks controllable viewers (newest instance controlled, a reconnect keeps its place), request and
  response lines on the side-channel, separate reader and writer tasks, an 8 MiB line cap, and a
  resync on the same connection after `changed` or a lag. A viewer without `control` in its first
  line (an older one) behaves as before.
- **Viewer control, phase 2.** `viewer_show` (highlight or isolate nodes, select one), `viewer_clear`
  (with `restore` to give the user's view back) and `viewer_filter` (the class filter the user sees).
  The agent writes the same selection and filter the user clicks on, so the user can undo it in the
  same panels; `Esc` now clears the selection, then restores the user's view, then clears the
  highlight, one step per press; an *Agent view · Restore my view* chip shows while the agent has
  something up; the activity feed lists what the agent did; the response's `user` block says
  whether the user is active and how many times they changed the view, and `if_user_epoch` refuses a
  stale command (`user_changed`). A headless viewer answers `unsupported_headless`.
- Store port: `write_batch` (conditional write in one transaction) and
  `parse_rdf_raw`.

### Security
- All store writes now go through one **write gate** (a FIFO lock): writes
  are serialized, a slow write no longer blocks reads or the MCP transport,
  and a write that waits longer than `[ingest] write_wait_timeout_secs`
  (default 30) fails with `write_busy`. Tools run on blocking threads.
- **Write guards** for every non-privileged writer, plugins included:
  - writes to the internal graphs (`urn:meta`, `urn:hkb:meta:*`,
    `urn:sparql-mcp:jobs`, `urn:sparql-mcp:staging:*`) are refused; `project_create` and the recording
    imports still write them through the core's own handle;
  - `LOAD`, `DROP ALL`/`CLEAR ALL`/`NAMED`, and a `DELETE` over a variable
    graph (`GRAPH ?g`) are always refused;
  - `DROP`, `CLEAR`, `MOVE`/`COPY` onto a graph, `DELETE { ?s ?p ?o }` and an
    `INSERT` over a variable graph are refused unless
    `[ingest] allow_destructive_updates = true`.
- **Size limits**: `max_update_bytes`, `max_batch_bytes` (inline RDF) and
  `max_load_file_bytes`, all 1 MiB / 1 MiB / 64 MiB by default.
- **`[ingest] file_roots`** confines `load_ontology_file` and `export_graph`
  to the listed directories (symlinks, junctions and `..` are resolved; UNC,
  device and alternate-data-stream paths are refused on Windows). Unset keeps
  the old behaviour and logs a warning.
- Refusals reach the agent as `isError` results with a JSON payload
  (`code`, `message`, `limit_key`, `limit`, `value`, `holder`).

### Changed
- **Loads are atomic**: a syntax error at triple N no longer leaves the first
  N-1 triples in the store. Blank node labels are renamed per call, so `_:b0`
  in two calls is two nodes (it used to be merged silently).
- **No whole-store scan on ground writes**: `INSERT DATA`, `DELETE DATA` and
  loads count with `contains`. Their `triples_before` and `triples_after` are
  now `null`; use `delta`, `inserted_new` and `removed_existing`. Pattern
  updates keep real values.
- `load_ontology`, `load_ontology_file` and `cbm_load_graph` load into the
  **active project graph** when `graph_iri` is omitted (they used to load into
  the default graph, which no viewer shows).
- Viewer events name the graph the data actually went to (the default graph is
  `""`), are split at 5,000 triples, and become one `changed` event above
  50,000. `cbm_load_graph` now emits events (it emitted none) and writes in
  chunks of 10,000 triples (at most 1,000,000 per call).
- `update_sparql` with `dry_run` now parses and classifies the request and
  reports `operations`, `graphs`, `destructive` and `refused`, without running
  it. SPARQL syntax errors are `parse_error` results.
- `SparqlMcpServer::new` takes a gated and a privileged store
  (`application::build_stores`).
- The loaded-file ledger is keyed by (file hash, target graph) and written in
  the same transaction as the data. Older entries are ignored, so each file is
  reloaded once.

### Scaffolded (wiring in v0.2)
- `per_project_store` flag in `[core]` — will open one RocksDB store per
  project slug under `$SPARQL_MCP_HOME/projects/<slug>/` so multiple agents
  can work on different projects in parallel without lock contention.

### Removed
- SSE bridge script (`start_sparql_http.sh`) — STDIO-only is the supported
  transport. Parallel multi-project will be handled by `per_project_store`.
