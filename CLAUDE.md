# CLAUDE.md — sparql-mcp

Resume rules for any agent picking up work in this repo. Read this first.

## What this is

`sparql-mcp` is an Oxigraph-backed SPARQL 1.1 MCP server (STDIO transport).
Cargo workspace: `crates/sparql-mcp-core` (the server) + `examples/hello-plugin`
(plugin SDK example). Layered/hexagonal: `domain/` (SPARQL logic),
`infrastructure/` (Oxigraph + FS adapters), `mcp/` (MCP transport), `plugin/`.

## Working methodology — TDD by default (non-negotiable)

Every feature follows spec → plan → TDD, per `docs/superpowers/`:

1. Write a design **spec** in `docs/superpowers/specs/<date>-<name>.md`.
2. Write a checkbox **plan** in `docs/superpowers/plans/<date>-<name>.md`
   (one task per logical unit; each task = the 5-step loop below).
3. Per task, in order:
   - write the **failing test**,
   - run it and confirm it **fails** (`cargo test -p sparql-mcp-core <filter>`),
   - implement the minimum to pass,
   - run tests and confirm **green**,
   - **commit** (one commit per task, `feat(scope): ...`).

The agent is its own tester. CI enforces `cargo fmt`, `cargo clippy -D warnings`,
and `cargo test`. Don't skip clippy.

## KB-first doctrine (source of truth)

SPARQL is the source of truth; the filesystem/Obsidian vault is a projection.

- Query the graph (`mcp__sparql-mcp__query_sparql`) before reading files.
- Display SPARQL payloads verbatim in responses.
- Store first → verify with SELECT → only then persist TTL.
- Dev/meta work for this repo is tracked in graph `<urn:project:sparql-mcp-dev>`.

Full doctrine: `plugins/kb-workbench/skills/kb-workbench/SKILL.md`.

Meta-work on this repo's own SDLC (spec → plan → review → QA → ship → retro)
is itself tracked as a domain skill on top of the same KB: see
`.claude/skills/sdlc-workbench/SKILL.md`.

## Data separation (critical)

The Oxigraph store contains **personal data**.

- **Schema/ontology migrations** (structure only, no personal data) are
  versioned **here**, in this public repo.
- The **data itself** (project triples, TTL/RocksDB snapshots) is versioned
  **separately in a private repo** (e.g. `github.com/mcazerty/mcazerty-data`)
  and/or synced via GDrive (`docs/superpowers/specs/2026-04-28-gdrive-sync-design.md`).
- **Never** commit personal triples into this public repo.

## Store facts

- Oxigraph store = RocksDB, **exclusive lock per directory**: only one process
  mutates a given store at a time.
- STDIO MCP only (no daemon, no ports). Each session spawns a child process.
- Parallel multi-project: distinct `[core] store` paths, or `per_project_store`
  (roadmap v0.2, scaffolded — see CHANGELOG).

## Roadmap / known gaps

- No `TODO`/`ROADMAP` file: de-facto roadmap = `CHANGELOG.md` + `docs/superpowers/`.
- **No migration system yet** — being designed (feature 2).
- TUI project viewer — being built (feature 1).
