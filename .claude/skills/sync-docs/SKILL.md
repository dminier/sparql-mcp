---
name: sync-docs
description: Detect and fix drift between sparql-mcp's code and its docs (README.md, AGENTS.md, CHANGELOG.md, docs/getting-started.md, crate READMEs, version numbers). Use after adding/renaming an MCP tool, a CLI subcommand or flag, a config key, a crate or a module; before a release or a PR; or when the user says "update the docs", "check the README", "is the doc up to date", "mets à jour la doc", "la doc est à jour ?", "prépare la release".
---

# Sync docs with code

The code is the source of truth; docs are projections of it. Compare each doc
against the place that defines the fact, fix what drifted, and keep docs
short — remove a stale line rather than explaining it.

## What to check, and against what

| Doc fact | Source of truth |
|---|---|
| README "MCP tools" table | `core_tool_defs()` in `crates/sparql-mcp-core/src/mcp/server.rs` (feature-gated tools: mention the feature) |
| README "CLI" block | `enum Cmd` in `crates/sparql-mcp-core/src/main.rs` (one line per subcommand, doc comment as the description) |
| Config keys in README / getting-started / `sparql-mcp.toml` | structs in `crates/sparql-mcp-core/src/config.rs` |
| Viewer flags and controls | `crates/sparql-mcp-viewer/src/main.rs` (clap) and `camera.rs` / `camera_input.rs` / `interact.rs` → `crates/sparql-mcp-viewer/README.md` |
| AGENTS.md layout, architecture, commands, invariants | the tree, `lib.rs` modules, `Makefile`, `.github/workflows/ci.yml` |
| Versions | `[workspace.package] version` in `Cargo.toml`, crate `Cargo.toml`s, `server.json`, plugin manifests in `plugins/`, latest `CHANGELOG.md` heading |

Get the current lists mechanically instead of from memory, e.g.:

```bash
grep -n "tool_.*_def()" crates/sparql-mcp-core/src/mcp/server.rs
grep -rn '"version"\|^version' Cargo.toml crates/*/Cargo.toml server.json plugins/**/*.json
```

## CHANGELOG

Use `git log <last-release-tag>..HEAD --oneline` to find user-visible changes
missing from the unreleased section. Keep a Changelog headings (`Added`,
`Changed`, `Fixed`, `Removed`); one line per change, written for a user, not
a commit message copy.

## Rules

- Fix factual drift directly. Report, without changing, anything that needs a
  decision — mismatched versions, a documented feature that no longer exists.
- Don't add sections. If a doc grows, link to the source instead of copying it.
- `plugins/kb-workbench/` is end-user product documentation: update it only
  when a tool it calls changed name or arguments.
- Finish with a short list: what was fixed, what needs a decision.
