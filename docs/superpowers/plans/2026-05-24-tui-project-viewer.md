# TUI Project Viewer — Implementation Plan

> Follow the `tdd` skill: per task → failing test → confirm fail → implement →
> confirm green → commit. Spec: `../specs/2026-05-24-tui-project-viewer-design.md`.

## File Map

| Action | File | Responsibility |
|---|---|---|
| Create | `crates/sparql-mcp-core/src/application/stats.rs` | `StoreStats`, `ProjectStat`, `collect_store_stats`, `collect_project_stats` (pure, tested) |
| Modify | `crates/sparql-mcp-core/src/application/mod.rs` | `pub mod stats;` |
| Create | `crates/sparql-mcp-core/tests/stats.rs` | unit tests for the stats layer |
| Create | `crates/sparql-mcp-core/src/tui/mod.rs` | ratatui render + event loop (`run(store)`) |
| Modify | `crates/sparql-mcp-core/src/lib.rs` | `pub mod tui;` (and ensure `application::stats` reachable) |
| Modify | `crates/sparql-mcp-core/Cargo.toml` | add `ratatui`, `crossterm` |
| Modify | `crates/sparql-mcp-core/src/main.rs` | add `Cmd::Tui`, dispatch to `tui::run` |

## Task 1: stats data layer (TDD)

- [ ] Write `tests/stats.rs`: in-memory store, load `<urn:meta>` with two
  `smc:Project` records (one with `smc:description`, one without) + load
  `<urn:project:a>` and `<urn:project:b>` with known triples. Assert:
  - `collect_store_stats` → exact `triples`, `graphs`, `nodes`.
  - `collect_project_stats` → 2 entries sorted by id, description fallback to
    label when `smc:description` absent, correct per-project `triples`/`nodes`.
- [ ] `cargo test -p sparql-mcp --test stats 2>&1 | head -30` → fails to compile (module absent).
- [ ] Implement `application/stats.rs` + `pub mod stats;`. Extract counts from
  `QueryResult::Solutions` via `RdfTerm::as_value_str().parse::<u64>()`.
- [ ] `cargo test -p sparql-mcp --test stats` → green.
- [ ] `cargo clippy -p sparql-mcp -- -D warnings`.
- [ ] Commit: `feat(stats): add project + store stats collectors`.

## Task 2: TUI module + deps

- [ ] Add `ratatui` + `crossterm` to Cargo.toml; `cargo build -p sparql-mcp`.
- [ ] Implement `src/tui/mod.rs`: terminal guard (alt screen + raw mode, restored
  on drop), build stats once, render header `Paragraph` + project `Table` with
  `TableState`, event loop (Up/Down/j/k/q/Esc). `pub mod tui;` in lib.rs.
- [ ] `cargo build` clean + `cargo clippy -- -D warnings`.
- [ ] Commit: `feat(tui): ratatui project viewer with global stats header`.

## Task 3: wire `sparql-mcp tui`

- [ ] Add `/// Launch the terminal project viewer.  Tui,` variant to `Cmd`.
- [ ] In `run()`, after the store is opened, add `Cmd::Tui => tui::run(store)?;`.
- [ ] `cargo test -p sparql-mcp` (full suite) → green.
- [ ] Manual smoke: `cargo run -p sparql-mcp -- --store <path> tui`.
- [ ] Commit: `feat(cli): add tui subcommand`.

## Verification checklist

- [ ] `cargo test -p sparql-mcp` all green
- [ ] `cargo clippy -p sparql-mcp -- -D warnings` clean
- [ ] `sparql-mcp tui` renders projects + global triple/node counts; q quits;
      terminal restored cleanly afterwards
