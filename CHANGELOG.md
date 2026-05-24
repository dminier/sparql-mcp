# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] — 2026-05-25

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

- Terminal viewer `sparql-mcp tui` — project list + 4-tab detail
  (Detail / Ontologies with `subClassOf` inheritance / Metrics / Backup).
- Schema migrations: `sparql-mcp migrate [--status|--dry-run]`
  (forward-only, embedded, version + checksum in `<urn:meta>`).
- KB container: `kb-export [--tag]`, `kb-import`, `kb-list` — portable,
  shareable zip of the whole store; daily `latest.zip` + auto-incremental
  `vN` versions; Backup Manager screen (F5/F6) in the TUI.
- Contextual `claude` launch from the viewer (`c`).
- Desktop launcher + semantic-graph logo created by `install` (Linux).
- Read-only store open for viewers (`tui`, `stats`, `kb-export`) — no lock
  contention with a running MCP server.

### Scaffolded (wiring in v0.2)
- `per_project_store` flag in `[core]` — will open one RocksDB store per
  project slug under `$SPARQL_MCP_HOME/projects/<slug>/` so multiple agents
  can work on different projects in parallel without lock contention.

### Removed
- SSE bridge script (`start_sparql_http.sh`) — STDIO-only is the supported
  transport. Parallel multi-project will be handled by `per_project_store`.
