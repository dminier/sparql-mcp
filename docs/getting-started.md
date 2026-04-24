# Getting started

From zero to first SPARQL query executed by an AI agent.

## 1. Install the binary

### One-liner (macOS / Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/dminier/sparql-mcp/main/install.sh | bash
```

### From source

```bash
git clone https://github.com/dminier/sparql-mcp
cd sparql-mcp
cargo build --release
cp target/release/sparql-mcp ~/.local/bin/
```

## 2. Register the MCP server in your agent

The binary ships a self-install subcommand that patches the config of
every detected agent (Claude Code, Codex CLI, Gemini CLI):

```bash
sparql-mcp install          # interactive
sparql-mcp install -y       # non-interactive
sparql-mcp install --dry-run
```

It writes a STDIO entry like:

```json
{
  "mcpServers": {
    "sparql-mcp": {
      "type": "stdio",
      "command": "/home/you/.local/bin/sparql-mcp",
      "args": ["serve"]
    }
  }
}
```

Each session your agent opens spawns its own `sparql-mcp serve` child —
there is no daemon to keep running.

## 3. Pick a data directory

By default the server opens `./store/` (RocksDB) and `./ontology/` in the
process's current working directory. Override per-project:

```toml
# sparql-mcp.toml (placed at the project root)
[core]
store    = "./store"
ontology = "./ontology"
docs     = "./front/docs"
```

Or globally via CLI flags: `sparql-mcp serve --store ~/kb/store --ontology ~/kb/ontology`.

## 4. Load the core ontology

Once the server starts, ask the agent: *"load the core ontology"*. The
`kb-workbench` skill calls `load_ontology_file` on `ontology/1-smc.ttl`.

Or from the CLI:

```bash
sparql-mcp reload-ontology
```

## 5. First query

From the agent: *"show me the 25 first triples of the meta graph"*.

Or directly against the store (requires the server not to be running, due
to the RocksDB exclusive lock):

```sparql
PREFIX smc: <https://sparql-mcp.dev/ns#>
SELECT ?s ?p ?o
WHERE { GRAPH <urn:meta> { ?s ?p ?o } }
LIMIT 25
```

## 6. Writing a domain plugin

Implement the `ToolPlugin` trait from the `sparql-mcp` crate in your own
crate and register it in `sparql-mcp.toml`. See
`crates/sparql-mcp-core/src/plugin/mod.rs` for the contract.
