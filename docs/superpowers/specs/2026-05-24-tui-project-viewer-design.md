# TUI Project Viewer — Design Spec

**Date:** 2026-05-24
**Status:** approved
**Scope:** A terminal UI subcommand that very roughly visualizes the projects in
the store with a short description plus global statistics (triple count, node count).

---

## Context

`sparql-mcp` already has a `stats` subcommand that prints `triples: N` and nothing
else. There is no quick way to see, at a glance, what projects exist in the store
and how big each one is. The user wants a lightweight "launch button" — a terminal
UI (no browser) — to eyeball projects and global volume.

Projects are `smc:Project` instances in the `<urn:meta>` named graph
(`smc:` = `https://sparql-mcp.dev/ns#`), each owning a named graph
`<urn:project:ID>`. Predicates: `smc:projectId`, `smc:label`, `smc:namedGraph`,
`smc:created`. There is **no description predicate today** — the TUI reads an
OPTIONAL `smc:description` and falls back to the label.

## Goals

- New `sparql-mcp tui` subcommand opening a ratatui full-screen view.
- Header line: global stats — total triples, total named graphs, total distinct
  IRI nodes.
- Scrollable list of projects: `id` · label · short description · per-project
  triple count · per-project node count.
- Keyboard: `↑/↓` or `j/k` to move, `q`/`Esc` to quit.
- The stats-gathering layer is **pure and fully unit-tested** against an in-memory
  `OxigraphAdapter`; the ratatui render/event loop is thin and not unit-tested.

## Non-goals

- Editing, querying, or mutating anything from the TUI (read-only).
- Mouse support, themes, or per-graph drill-down (future).
- Running while a `serve` process holds the store lock (see Constraints).

## Constraints

- Oxigraph/RocksDB takes an **exclusive lock per store directory**. `tui` opens
  the store read/write like every other subcommand, so it cannot run while a
  `serve` process is holding the same store. Documented; not worked around here.
- No personal data leaves the machine — the TUI only renders counts and the
  project label/description already in the store.

## Data model

Two pure types in a new module `application/stats.rs`:

```rust
pub struct StoreStats {
    pub triples: u64,
    pub graphs: u64,
    pub nodes: u64,   // distinct IRIs appearing as subject or object, any graph
}

pub struct ProjectStat {
    pub id: String,
    pub label: String,
    pub description: String, // smc:description, fallback to label
    pub graph_iri: String,
    pub triples: u64,
    pub nodes: u64,
}
```

Two pure functions, each taking `&dyn SparqlStore`:

- `collect_store_stats(store) -> Result<StoreStats>`
- `collect_project_stats(store) -> Result<Vec<ProjectStat>>` (sorted by id)

### SPARQL

Global triples: `store.triple_count()`.
Global graphs: `store.list_graphs()?.len()`.
Global nodes:
```sparql
SELECT (COUNT(DISTINCT ?n) AS ?c) WHERE {
  GRAPH ?g { { ?n ?p ?o } UNION { ?s ?p ?n } }
  FILTER(isIri(?n))
}
```
Projects: the existing `project_list` SELECT, plus `OPTIONAL { ?p smc:description ?desc }`.
Per-project triples:
```sparql
SELECT (COUNT(*) AS ?c) WHERE { GRAPH <IRI> { ?s ?p ?o } }
```
Per-project nodes:
```sparql
SELECT (COUNT(DISTINCT ?n) AS ?c) WHERE {
  GRAPH <IRI> { { ?n ?p ?o } UNION { ?s ?p ?n } }
  FILTER(isIri(?n))
}
```

## TUI layer

Module `tui/mod.rs` (`pub fn run(store: Arc<dyn SparqlStore>) -> Result<()>`):
- Enter alternate screen + raw mode via crossterm; restore on exit (and on panic
  via a guard).
- Build `StoreStats` + `Vec<ProjectStat>` once at startup.
- Render: top `Paragraph` with global stats; main `Table`/`List` of projects with
  a `ListState` for selection.
- Event loop: `crossterm::event::read()`; handle Up/Down/j/k/q/Esc.

## Dependencies

Add to `crates/sparql-mcp-core/Cargo.toml`: `ratatui` and `crossterm`
(ratatui re-exports a compatible crossterm; pin to ratatui's version to avoid
a duplicate crossterm). TUI code gated behind nothing — it's a normal subcommand.

## Testing strategy

- `tests/stats.rs`: build an in-memory `OxigraphAdapter` (temp dir), load a couple
  of named graphs with known triples, assert `collect_store_stats` and
  `collect_project_stats` return exact counts, description fallback, and sort order.
- The ratatui loop is exercised manually (`cargo run -p sparql-mcp-core -- tui`);
  no unit test for terminal rendering.
