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
  Not yet available: `ingest_validate`, `ingest_file`, and the expiry of idle jobs.
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
