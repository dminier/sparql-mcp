# sparql-mcp

[![CI](https://github.com/OWNER/sparql-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/OWNER/sparql-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**A generic semantic knowledge base for AI agents.** `sparql-mcp` is a small,
well-packaged MCP server wrapping an [Oxigraph](https://github.com/oxigraph/oxigraph)
SPARQL 1.1 store, shipped together with a Claude Code plugin and a skill that
teach agents the single contract:

> **SPARQL is the source of truth. Obsidian is its human face.**

Point any number of agents at the same store, let them ingest code, documents,
browser recordings, and web pages into project-scoped named graphs, then
project the graph to Obsidian notes + canvases for humans.

## What's in this repo

```
sparql-mcp/
├── crates/sparql-mcp-core/        # Rust: the MCP server + Oxigraph core
├── plugins/kb-workbench/          # Claude Code plugin (marketplace-installable)
├── .claude/skills/kb-workbench/   # The skill itself (symlinked from the plugin)
├── ontology/1-smc.ttl             # Core RDF vocabulary (smc:)
├── sparql-mcp.toml                # Sample server config
├── Dockerfile / docker-compose.yml
└── Makefile
```

Three artefacts, one workspace:

| Artefact | Role |
|---|---|
| **Cargo crate** `sparql-mcp` (`crates/sparql-mcp-core`) | The server binary, usable standalone over STDIO or SSE |
| **Claude Code plugin** `kb-workbench` | Installs the skill + SSE bridge script |
| **Skill** `kb-workbench` | Teaches agents how to ingest / audit / render the KB |

Domain-specific layers (bug-bounty, enterprise-architecture, research
notebooks, …) live in **downstream** repos and depend on this one.

## Quick start

```bash
# 1. Build
cargo build --release

# 2. Serve (STDIO by default — Claude Code spawns it automatically via .mcp.json)
./target/release/sparql-mcp serve --config sparql-mcp.toml

# 3. Or: shared SSE bridge (one process, many agents — recommended on Linux)
bash plugins/kb-workbench/skills/kb-workbench/scripts/start_sparql_http.sh --bg
# → http://127.0.0.1:7733/sse
```

Minimal `.mcp.json` for Claude Code:

```json
{
  "mcpServers": {
    "sparql-mcp": {
      "type": "sse",
      "url": "http://127.0.0.1:7733/sse"
    }
  }
}
```

## Install the plugin

```text
/plugin marketplace add OWNER/sparql-mcp
/plugin install kb-workbench@sparql-mcp
```

Replace `OWNER` with the GitHub org / user hosting the fork.

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

Named graph convention: `<urn:project:<slug>>`, with metadata on `<urn:meta>`.

## Core SPARQL prefixes

```turtle
PREFIX smc: <https://sparql-mcp.dev/ns#>
```

Downstream domain plugins declare their own prefixes on top.

## Development

```bash
make build         # cargo build --release
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Contributing

Issues and PRs welcome. Please:

- Keep the core generic — domain logic belongs in a separate plugin crate.
- English only in code, comments, identifiers, and commits.
- Add tests next to the code they exercise.

## License

MIT — see [LICENSE](./LICENSE).
