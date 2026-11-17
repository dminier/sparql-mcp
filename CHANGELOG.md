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

### Scaffolded (wiring in v0.2)
- `per_project_store` flag in `[core]` — will open one RocksDB store per
  project slug under `$SPARQL_MCP_HOME/projects/<slug>/` so multiple agents
  can work on different projects in parallel without lock contention.

### Removed
- SSE bridge script (`start_sparql_http.sh`) — STDIO-only is the supported
  transport. Parallel multi-project will be handled by `per_project_store`.
