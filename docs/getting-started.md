# Getting started

This guide walks you from `git clone` to your first SPARQL query executed by
an AI agent.

## 1. Prerequisites

- Rust toolchain (stable, 1.78+): <https://rustup.rs>
- Claude Code (or any MCP-compatible client)
- Optional: Obsidian (for the human-facing projection of the graph)

## 2. Build

```bash
cargo build --release
```

Binary lands at `./target/release/sparql-mcp`.

## 3. Run

Two transports are supported:

### STDIO (one client per process)

```bash
./target/release/sparql-mcp serve --config sparql-mcp.toml
```

Suitable for a single Claude Code session — add an entry to the project's
`.mcp.json`:

```json
{
  "mcpServers": {
    "sparql-mcp": {
      "type": "stdio",
      "command": "./target/release/sparql-mcp",
      "args": ["serve", "--config", "sparql-mcp.toml"]
    }
  }
}
```

### Shared SSE bridge (recommended for multi-client setups)

RocksDB (the Oxigraph backend) holds an exclusive lock on the store, so two
STDIO clients cannot share the same store directory. The plugin ships a
one-shot bridge that runs a single `sparql-mcp` process and re-exposes it
over SSE so every client can connect:

```bash
bash plugins/kb-workbench/skills/kb-workbench/scripts/start_sparql_http.sh --bg
```

Then in `.mcp.json`:

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

## 4. Load the core ontology

Once the server is up, an agent running the `kb-workbench` skill will load
`ontology/1-smc.ttl` automatically on first use. Or you can do it manually
from any SPARQL client by calling the `load_ontology_file` tool.

## 5. First query

```sparql
PREFIX smc: <https://sparql-mcp.dev/ns#>
SELECT ?s ?p ?o
WHERE { GRAPH <urn:meta> { ?s ?p ?o } }
LIMIT 25
```

## 6. Writing a domain plugin

To extend `sparql-mcp` with domain-specific MCP tools, implement the
`ToolPlugin` trait from the `sparql-mcp` crate in your own crate and register
it in `sparql-mcp.toml`. See `crates/sparql-mcp-core/src/plugin/mod.rs` for
the contract.
