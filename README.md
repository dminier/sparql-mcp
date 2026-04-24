# sparql-mcp

[![CI](https://github.com/dminier/sparql-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/dminier/sparql-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**A generic semantic knowledge base for AI coding agents.**
`sparql-mcp` is a single-binary MCP server wrapping an
[Oxigraph](https://github.com/oxigraph/oxigraph) SPARQL 1.1 store, plus a
Claude Code plugin and skill that teach agents the single contract:

> **SPARQL is the source of truth. Obsidian is its human face.**

Install once on your workstation, use it across every project you touch.

## Quick start

### One-line install (macOS / Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/dminier/sparql-mcp/main/install.sh | bash
```

The installer downloads the right static binary for your platform, drops it
into `~/.local/bin/sparql-mcp`, verifies its SHA-256 checksum, then runs
`sparql-mcp install -y` which auto-patches each detected agent's MCP config
(Claude Code, Codex CLI, Gemini CLI).

Flags: `--dir=<path>` (install location), `--skip-config` (don't patch
agents). Environment: `SPARQL_MCP_VERSION=v0.1.0` to pin a release.

### Windows (PowerShell)

```powershell
# 1. Download the archive for your platform from the latest release:
#    https://github.com/dminier/sparql-mcp/releases/latest
# 2. Extract sparql-mcp.exe somewhere on PATH
# 3. Register MCP entries:
sparql-mcp.exe install -y
```

### Manual / from source

```bash
git clone https://github.com/dminier/sparql-mcp
cd sparql-mcp
cargo build --release
./target/release/sparql-mcp install -y     # patch agent configs
```

After install, restart your coding agent. `sparql-mcp` is now a STDIO MCP
server the agent spawns on demand — nothing to keep running.

## Why a single-binary STDIO server

- **Zero dependencies** — no Docker, no daemon, no background process.
- **One install, every project** — the binary lives in `~/.local/bin`; every
  agent session spawns its own child process and talks to it over stdin/stdout.
- **Multi-project** (roadmap v0.2) — flip `per_project_store = true` in the
  config and each project opens its own RocksDB store, so several agents can
  work on different projects in parallel without contending for the same
  database lock.

## What's in this repo

```
sparql-mcp/
├── crates/sparql-mcp-core/        # Rust: MCP server + Oxigraph core
├── plugins/kb-workbench/          # Claude Code plugin (skill only)
├── .claude/skills/kb-workbench/   # Symlink to the skill
├── ontology/1-smc.ttl             # Core RDF vocabulary (smc:)
├── sparql-mcp.toml                # Sample config
├── server.json                    # MCP registry manifest
├── install.sh                     # One-line installer
└── docs/getting-started.md
```

## MCP tools exposed

| Tool | Purpose |
|---|---|
| `query_sparql` | SELECT / ASK / CONSTRUCT / DESCRIBE |
| `update_sparql` | INSERT DATA / DELETE / LOAD / CLEAR (SPARQL 1.1 Update) |
| `load_ontology`, `load_ontology_file` | Push TTL into a named graph (SHA-256 idempotent) |
| `export_graph` | Dump a named graph to Turtle on disk |
| `project_create`, `project_list`, `project_switch` | Manage `smc:Project` isolation |
| `write_doc` | Persist agent-generated markdown to the configured docs root |
| `stats`, `list_graphs` | Introspection |

Named graph convention: `<urn:project:<slug>>`.

## CLI

```
sparql-mcp serve                # run as MCP STDIO server (default for agents)
sparql-mcp install              # register MCP entries in detected agent configs
sparql-mcp stats                # triple / graph counts
sparql-mcp reload-ontology      # re-parse ontology/ into the store
sparql-mcp load-file --path …   # load Turtle / NTriples / RDF-XML
sparql-mcp code-import --db …   # ingest a codebase-memory-mcp SQLite graph
```

## Install the Claude Code plugin (skill)

The MCP server itself is installed as a binary (above). The `kb-workbench`
plugin adds the matching Claude Code **skill** — the workflow that teaches
an agent how to ingest / audit / render a knowledge base:

```text
/plugin marketplace add dminier/sparql-mcp
/plugin install kb-workbench@sparql-mcp
```

## Development

```bash
make build         # cargo build --release
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## License

MIT — see [LICENSE](./LICENSE).
