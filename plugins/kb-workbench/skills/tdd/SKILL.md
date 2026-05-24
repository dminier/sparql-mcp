---
name: tdd
description: |
  Default development workflow for this repo: spec -> plan -> test-driven
  development. Load this skill at the start of ANY feature, bug fix, or
  refactor in sparql-mcp. Enforces the five-step loop (write failing test ->
  confirm it fails -> implement -> confirm green -> commit), spec files under
  docs/superpowers/specs/, and checkbox plans under docs/superpowers/plans/.
  Triggers (EN): "add a feature", "implement", "fix this bug", "refactor",
  "write tests", "start the plan", "TDD".
  Triggers (FR): "ajoute une feature", "implémente", "corrige ce bug",
  "refactor", "écris les tests", "démarre le plan", "on commence une feature".
  The agent is its own tester. CI enforces fmt + clippy(-D warnings) + tests.
---

# tdd — spec → plan → test-driven development

This is the **default** workflow for sparql-mcp. Every feature, fix, or
refactor goes through it. The agent is its own tester.

## The loop

1. **Spec** — `docs/superpowers/specs/<YYYY-MM-DD>-<name>.md`: context, goals,
   non-goals, design, data model. Get it approved before planning anything risky.
2. **Plan** — `docs/superpowers/plans/<YYYY-MM-DD>-<name>.md`: a file map, then
   one task per logical unit. Each task is a checkbox list of the 5 steps below.
3. **Per task (TDD):**
   - [ ] write the **failing test**;
   - [ ] run it, confirm it **fails** for the right reason
     (`cargo test -p sparql-mcp-core <filter> 2>&1 | head -30`);
   - [ ] implement the **minimum** to pass;
   - [ ] run tests, confirm **green** (`cargo test -p sparql-mcp-core`);
   - [ ] **commit** — one commit per task, `feat(scope): ...` / `fix(scope): ...`.

## Rules

- Never mark a task done with failing tests or partial implementation.
- Don't skip clippy: `cargo clippy -p sparql-mcp-core -- -D warnings`.
- Don't write code before the test that demands it.
- Personal data never enters the public repo — see root `CLAUDE.md` §"Data separation".
- SPARQL is the source of truth — see `kb-workbench` skill.

## Related

- `kb-workbench` — KB-first doctrine, ontology, Obsidian rendering, GDrive sync.
- `docs/superpowers/` — worked examples of specs + plans (e.g. gdrive-sync).
