# sparql-mcp

[![CI](https://github.com/dminier/sparql-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/dminier/sparql-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**A personal semantic knowledge base for you and your AI agents — one small binary.**

`sparql-mcp` wraps an [Oxigraph](https://github.com/oxigraph/oxigraph) SPARQL 1.1
store behind the [Model Context Protocol](https://modelcontextprotocol.io), so any
MCP-aware agent (Claude Code, Codex CLI, Gemini CLI…) can read and write a real
knowledge graph. It also ships a terminal UI to browse your projects, and a
one-file **KB container** format to back up, version, and share your knowledge.

> **SPARQL is the source of truth.** Everything else — the terminal UI, the
> Obsidian vault, the backups — is a view or an export of it.

---

# User guide

## 1. Install (from a release)

**macOS / Linux — one line:**

```bash
curl -fsSL https://raw.githubusercontent.com/dminier/sparql-mcp/main/install.sh | bash
```

This downloads the right static binary for your platform, verifies its SHA-256,
drops it in `~/.local/bin/sparql-mcp`, and runs `sparql-mcp install -y` which:

- registers `sparql-mcp` in every detected agent's MCP config, and
- adds a **desktop launcher** that opens the terminal viewer (Linux).

Pin a version with `SPARQL_MCP_VERSION=v0.1.0`, change the location with
`--dir=<path>`, or skip agent config with `--skip-config`.

**Windows:** download the archive from the
[latest release](https://github.com/dminier/sparql-mcp/releases/latest), put
`sparql-mcp.exe` on your `PATH`, then run `sparql-mcp.exe install -y`. This
registers the MCP entry and drops a **launcher** (`sparql-mcp.cmd` + a Desktop
shortcut) that opens the viewer.

Restart your agent afterwards — `sparql-mcp` is a STDIO server the agent starts
on demand. Nothing keeps running in the background.

## 2. Browse your knowledge base — `sparql-mcp tui`

Launch the viewer from the desktop icon or the terminal:

```bash
sparql-mcp tui
```

**Project list** — global stats on top, your projects below:

```text
┌ Store ─────────────────────────────────────────────────────┐
│ sparql-mcp  —  146 triples · 2 graphs · 35 nodes            │
└─────────────────────────────────────────────────────────────┘
┌ Projects ───────────────────────────────────────────────────┐
│ project              description                triples nodes│
│› matrix_speedrunner  BreizhCamp CLI game (Rust)    105    32 │
│                                                              │
└─────────────────────────────────────────────────────────────┘
 ↑/↓ move   Enter open   q quit
```

Press **Enter** on a project to open its detail screen — four tabs, switch with
`Tab` / `←` `→` or `1`·`2`·`3`·`4`:

**① Detail**

```text
┌ matrix_speedrunner ─────────────────────────────────────────┐
│  Detail │ Ontologies │ Metrics │ Backup                      │
└─────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────┐
│ Name        Matrix Speedrunner                               │
│ Project id  matrix_speedrunner                               │
│                                                              │
│ Description                                                  │
│ BreizhCamp CLI game written in Rust — a terminal "speedrun"  │
│ through falling Matrix glyphs. Modules, screens, assets and  │
│ tech notes are tracked here as a knowledge graph.            │
└─────────────────────────────────────────────────────────────┘
 Tab/←/→ or 1·2·3·4 switch   ↑/↓ scroll   Esc back   q quit
```

**② Ontologies** — classes used, instance counts, and `subClassOf` inheritance:

```text
┌─────────────────────────────────────────────────────────────┐
│ ● Module        (8 instances)                                │
│ ● Dependency    (5 instances)                                │
│ ● TechNote      (3 instances)                                │
│ ● Screen        (3 instances)                                │
│    ⊂ Asset                                                   │
│ ● GameDesign    (1 instances)                                │
│    a design decision for the game loop                       │
└─────────────────────────────────────────────────────────────┘
```

**③ Metrics** — each with an explanation and an interpretation of *this* value:

```text
┌─────────────────────────────────────────────────────────────┐
│ Triples       105                                            │
│    Total RDF statements in the project graph.                │
│    → Raw size of the knowledge graph.                        │
│ Density       3.28                                           │
│    Average statements per node (triples / nodes).            │
│    → rich — entities are densely described                   │
│ Typed ratio   100%                                           │
│    Share of subjects carrying an rdf:type.                   │
│    → high — almost everything is typed                       │
└─────────────────────────────────────────────────────────────┘
```

**④ Backup** — see §4.

## 3. Use it from your agent (MCP)

Once installed, your agent can call these tools:

| Tool | Purpose |
|---|---|
| `query_sparql` | SELECT / ASK / CONSTRUCT / DESCRIBE |
| `update_sparql` | INSERT / DELETE / LOAD / CLEAR (SPARQL 1.1 Update) |
| `load_ontology`, `load_ontology_file` | Push TTL into a named graph (idempotent) |
| `export_graph` | Dump a named graph to Turtle |
| `project_create`, `project_list`, `project_switch` | Manage `smc:Project` isolation |
| `write_doc` | Persist agent-written markdown |
| `stats`, `list_graphs` | Introspection |

Projects live in named graphs: `<urn:project:<slug>>`.

The **kb-workbench** Claude Code skill teaches the full ingest → audit → render
workflow on top of these tools:

```text
/plugin marketplace add dminier/sparql-mcp
/plugin install kb-workbench@sparql-mcp
```

## 4. Back up, version & share — the KB container

Your whole knowledge base travels as a single portable zip (a `manifest.json`
plus one Turtle file per graph). Share it, archive it, re-import it anywhere.

```bash
sparql-mcp kb-export                 # daily snapshot → ~/.local/share/sparql-mcp/backups/latest.zip
sparql-mcp kb-export --tag release   # a tagged, timestamped version
sparql-mcp kb-list                   # list available archives
sparql-mcp kb-import --path kb.zip   # reload / import a shared archive
```

```text
$ sparql-mcp kb-export --tag breizhcamp
exported 2 graph(s) -> ~/.local/share/sparql-mcp/backups/kb-breizhcamp-20260525T101500Z.zip (4213 bytes)

$ sparql-mcp kb-list
2026-05-25T09:00:00Z  [latest]      2 graphs  ~/.local/share/sparql-mcp/backups/latest.zip
2026-05-25T10:15:00Z  [breizhcamp]  2 graphs  ~/.local/share/sparql-mcp/backups/kb-breizhcamp-20260525T101500Z.zip
```

The **Backup** tab in the viewer shows the same thing and refreshes `latest.zip`
with a single key:

```text
┌─────────────────────────────────────────────────────────────┐
│ Backups dir  ~/.local/share/sparql-mcp/backups               │
│ latest.zip   updated 3h ago                                  │
│ ✓ saved latest.zip (2 graphs)                                │
│                                                              │
│ Archives (2)                                                 │
│   2026-05-25T09:00:00Z  [latest]      2 graphs               │
│   2026-05-25T10:15:00Z  [breizhcamp]  2 graphs               │
│                                                              │
│ A KB archive is a portable container: share it and re-import.│
│   b                       refresh latest.zip now             │
│   sparql-mcp kb-export --tag <name>    tagged version        │
│   sparql-mcp kb-import --path <zip>    reload a shared archive│
└─────────────────────────────────────────────────────────────┘
```

Tip: keep a daily `latest.zip` and cut a tagged version at milestones. For
multi-machine sync and a private data repo, see
[`data-versioning.md`](plugins/kb-workbench/skills/kb-workbench/references/data-versioning.md).

## 5. Schema migrations

When the schema/ontology evolves, forward-only migrations are embedded in the
binary and applied on demand (your data stays put):

```bash
sparql-mcp migrate --status     # current vs available version, pending list
sparql-mcp migrate --dry-run    # preview
sparql-mcp migrate              # apply (idempotent)
```

---

# For developers

Short version — the deep docs live in [`docs/`](docs/).

## Build & test

```bash
cargo build --release
cargo test                                   # 80+ tests, in-memory store
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Layout

```
crates/sparql-mcp-core/
  src/domain/           SparqlStore port (query/update/export…)
  src/infrastructure/   OxigraphAdapter (RocksDB; open_read_only for viewers)
  src/application/      stats, detail, migrations, archive, MCP tool handlers
  src/tui/              ratatui viewer (list + 4-tab detail)
  src/main.rs           CLI (serve, tui, migrate, kb-export/import/list, install…)
  migrations/*.ru       embedded schema migrations (SPARQL Update)
plugins/kb-workbench/   Claude Code skill + references
ontology/1-smc.ttl      core smc: vocabulary
```

## Conventions

- **Workflow:** spec → plan → TDD (see `docs/superpowers/` and the `tdd` skill).
  Each change writes a failing test first; CI gates on fmt + clippy + tests.
- **Read-only viewers:** `tui`, `stats` and `kb-export` open the store with
  `open_read_only`, so they never take the RocksDB write lock and run fine while
  an MCP server is live.
- **Data separation:** personal data is versioned in a *private* repo, never
  here; a `.githooks/pre-commit` guard rejects `urn:project:` TTL in this repo.
  Enable hooks with `git config core.hooksPath .githooks`.

## Contributing

Issues and PRs welcome. Run the full gate above before opening a PR.

## License

MIT — see [LICENSE](./LICENSE).
