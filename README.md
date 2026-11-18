# sparql-mcp

[![CI](https://github.com/dminier/sparql-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/dminier/sparql-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

A semantic knowledge base for AI coding agents: a single-binary MCP server
over an [Oxigraph](https://github.com/oxigraph/oxigraph) SPARQL 1.1 store.
Agents store what they learn as RDF in per-project named graphs and query it
back with SPARQL instead of re-reading files.

![The live 3D viewer while an agent writes 400 instances](docs/assets/viewer.png)

## Install

macOS / Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/dminier/sparql-mcp/main/install.sh | bash
```

Windows: download the archive from the
[latest release](https://github.com/dminier/sparql-mcp/releases/latest), put
`sparql-mcp.exe` on your `PATH`, then run `sparql-mcp.exe install -y`.

From source: `cargo build --release && ./target/release/sparql-mcp install -y`.

`install` registers the server in every detected agent (Claude Code, Codex CLI,
Gemini CLI). Restart the agent: it spawns `sparql-mcp serve` over STDIO when
needed — there is no daemon. See [getting started](docs/getting-started.md).

## MCP tools

| Tool | Purpose |
|---|---|
| `query_sparql` | SELECT / ASK / CONSTRUCT / DESCRIBE |
| `update_sparql` | SPARQL 1.1 Update |
| `load_ontology`, `load_ontology_file` | Load Turtle into a named graph (idempotent, SHA-256) |
| `export_graph`, `list_graphs`, `stats` | Export to Turtle, introspection |
| `project_create`, `project_list`, `project_switch` | One named graph per project: `<urn:project:<slug>>` |
| `cbm_load_graph` | Import a codebase-memory-mcp code graph |
| `write_doc` | Write markdown under the configured docs root |
| `get_gdrive_config` | Expose the `[gdrive]` sync settings to the agent |

The `recording` feature adds `import_recording_navigations`,
`import_recording_network` and `materialize_recording` (Playwright captures).

## CLI

```text
sparql-mcp serve              run as an MCP STDIO server
sparql-mcp install            register the server in detected agent configs
sparql-mcp stats              triple and graph counts
sparql-mcp reload-ontology    re-load ontology/ into the store
sparql-mcp load-file --path   load a Turtle / N-Triples / RDF-XML file
sparql-mcp code-import --db   import a codebase-memory-mcp SQLite graph
```

Configuration: [`sparql-mcp.toml`](sparql-mcp.toml).

## Live 3D viewer

`sparql-mcp-viewer` shows the active project graph in 3D and grows it as the
agent writes. Enable `[viewer]` in `sparql-mcp.toml`, then see
[its README](crates/sparql-mcp-viewer/README.md).

## Claude Code plugin

The `kb-workbench` plugin adds the skills that teach an agent to ingest,
audit and render a knowledge base with this server:

```text
/plugin marketplace add dminier/sparql-mcp
/plugin install kb-workbench@sparql-mcp
```

## Contributing

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
```

Architecture and conventions: [AGENTS.md](AGENTS.md).

## License

MIT — see [LICENSE](LICENSE).
