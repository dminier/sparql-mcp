# AGENTS.md

Guide for coding agents working **on** this repository. For using sparql-mcp
as a knowledge base, see the `kb-workbench` plugin instead.

## What this is

A single-binary MCP server (STDIO) wrapping an Oxigraph SPARQL 1.1 store, plus
an optional live 3D viewer. Rust workspace, edition 2021, MSRV 1.78.

| Path | Role |
|---|---|
| `crates/sparql-mcp-core/` | The server binary `sparql-mcp` (crate name `sparql-mcp`) |
| `crates/sparql-mcp-viewer/` | Bevy 0.20 viewer, reads the server's loopback side-channel |
| `examples/hello-plugin/` | Minimal `ToolPlugin` implementation |
| `plugins/kb-workbench/` | **Shipped product**: the Claude Code plugin end users install. Not dev tooling. |
| `.claude/` | Dev skills and agents for this repo — see the table at the end |
| `ontology/1-smc.ttl` | Core `smc:` vocabulary (`https://sparql-mcp.dev/ns#`) |
| `docs/designs/` | Design docs and their reviews — read the relevant one before changing the viewer |

## Core architecture (`crates/sparql-mcp-core/src/`)

Layered, dependencies point inward:

- `domain/` — ports: `SparqlStore`, `DocStore` traits. No I/O.
- `infrastructure/` — adapters: `OxigraphAdapter` (RocksDB or in-memory),
  `FsDocStore`, `viewer_channel` (TCP side-channel).
- `application/tools/` — one module per tool group; each tool is a pair
  `tool_<name>_def() -> Tool` + `fn <name>(store, args, …) -> Result<CallToolResult, McpError>`.
  `application/event_sink.rs` carries write events to the viewer.
- `mcp/server.rs` — thin dispatcher only: `core_tool_defs()` + `dispatch_core()`.
  No business logic here.
- `plugin/` — `ToolPlugin` trait for out-of-tree tools; core tools win on name clash.
- `cbm/` — import of codebase-memory-mcp SQLite graphs into RDF.

Named graphs: `<urn:project:<slug>>` per project, `<urn:meta>` for project metadata.

## Invariants — breaking these causes silent failures

- **Stdout belongs to JSON-RPC.** Never `println!` in server code paths; log
  with `tracing` (configured to stderr). A stray print corrupts the MCP stream.
- **Every write to the active graph emits a `StoreEvent`** through the
  `EventSink` it receives. Skip it and the viewer silently drifts from the store.
- **RocksDB takes an exclusive lock.** CLI commands touching `./store`
  (`stats`, `load-file`, …) fail while a `serve` process holds it. Tests use
  `OxigraphAdapter::open_in_memory()`.
- **Paths embedded in Turtle must be escaped** (`\` is a Turtle escape char —
  this broke every Windows path once).
- The `recording` feature gates three tools; CI builds with `--all-features`,
  so feature-gated code must compile and pass clippy.

## Commands

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings   # what CI runs
cargo test --all --all-features
cargo run -p sparql-mcp -- serve --config sparql-mcp.toml
cargo run --release -p sparql-mcp --example viewer_load -- --count 500   # feed the viewer without an agent
cargo run --release -p sparql-mcp-viewer -- --connect 127.0.0.1:7878
```

Run fmt, clippy and tests before declaring work done: CI fails on any warning.

Platform notes: on Windows, RocksDB needs `libclang` (`winget install LLVM.LLVM`).
Dependencies are built with `opt-level = 3` even in dev (Bevy is unusable
otherwise), so the first build is slow and later ones are not.

## Tests

Integration tests live in `crates/*/tests/`, one file per area
(`tools_sparql.rs`, `tools_project.rs`, `viewer_channel.rs`, …). They call the
tool functions directly against an in-memory store — follow that pattern
rather than spawning the binary. `mcp_handshake.rs` covers the STDIO protocol
end to end; `headless_e2e.rs` covers the viewer without a window.

## Conventions

- Commits: Conventional Commits with a scope — `feat(viewer): …`,
  `fix(core): …`, `docs(kb-workbench): …`.
- User-visible change → entry in `CHANGELOG.md` under the unreleased section.
- `sparql-mcp.toml` at the root is a sample; never commit real credentials
  or personal IDs into it.

## Project skills and agents

Local to this repo. Claude Code loads them natively; other agents should read
the file when the task matches.

| When | Read |
|---|---|
| Adding or changing an MCP tool | `.claude/skills/add-mcp-tool/SKILL.md` |
| Tools, CLI flags, config keys or versions changed; before a release | `.claude/skills/sync-docs/SKILL.md` |
| Before a commit or PR: check the diff against the invariants above | `.claude/agents/invariant-reviewer.md` |
