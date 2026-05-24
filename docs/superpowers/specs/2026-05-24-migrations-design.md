# Triplestore Migrations — Design Spec

**Date:** 2026-05-24
**Status:** DRAFT — awaiting review (personal-data sensitivity)
**Scope:** Versioned, forward-only **schema/ontology** migrations for the
Oxigraph store, plus the separation rule for **data** versioning.

---

## Context

The store is resilient (RocksDB) but the schema/ontology evolves: predicates get
renamed, classes added, facets backfilled. Today there is **no migration path** —
`grep migrat|schema_version|upgrade` over the crates returns nothing. Ontology
files are reloaded wholesale (`reload-ontology`), which adds triples but cannot
*transform* or *remove* existing data, and there is no record of what structural
changes have been applied to a given store.

**Critical constraint (see root `CLAUDE.md`):** the store contains **personal
data**. Two concerns must stay strictly separate:

1. **Schema migrations** — structural transforms (rename predicate, add class,
   backfill a facet from existing triples). Contain **no personal data** →
   versioned in *this public repo*.
2. **Data versioning** — the actual project triples. Versioned in a **private
   repo** (`github.com/mcazerty/mcazerty-data`) and/or via GDrive sync. **Never**
   committed to the public repo.

This spec covers (1) as a Rust feature and defines the protocol for (2).

## Goals

- Numbered, ordered, forward-only schema migrations applied deterministically.
- A schema version recorded in the store (`<urn:meta>`), so any machine knows
  which migrations have run.
- `sparql-mcp migrate` to apply pending migrations; `migrate --status` to report.
- Idempotent: re-running applies nothing if up to date.
- Migrations are **embedded in the binary** (static install via `install.sh`),
  so a distributed binary carries its own migration set.
- Each applied migration recorded with version, timestamp, and SHA-256 of the
  migration text (detects drift / tampering).

## Non-goals

- Down/rollback migrations (forward-only; restore from a snapshot instead).
- Migrating personal data content (only structure/schema).
- Automatic migration on `serve` startup in v1 (explicit `migrate` first; a
  `[core] auto_migrate` flag can be added later).

## Design

### Migration files

`crates/sparql-mcp-core/migrations/NNNN_snake_description.ru` — SPARQL 1.1 Update
text (INSERT/DELETE/DELETE+INSERT WHERE). Numbered from `0001`. Example:

```
# 0001_backfill_project_description.ru
PREFIX smc: <https://sparql-mcp.dev/ns#>
INSERT { GRAPH <urn:meta> { ?p smc:description ?label } }
WHERE  { GRAPH <urn:meta> { ?p a smc:Project ; smc:label ?label .
         FILTER NOT EXISTS { ?p smc:description ?_ } } }
```

Embedded at compile time (`include_dir!`), so order and content are fixed per
binary build. A migration file is **immutable once shipped** — fixes go in a new
file.

### Version tracking (in `<urn:meta>`)

```
<urn:meta:store> a smc:Store ; smc:schemaVersion 3 .
<urn:meta:migration:0001> a smc:Migration ;
    smc:version 1 ; smc:name "backfill_project_description" ;
    smc:checksum "sha256:…" ; smc:appliedAt "2026-05-24T…Z"^^xsd:dateTime .
```

### Runner (`domain`/`application` module `migrations.rs`)

- `pub struct Migration { version: u32, name: &str, sparql: &str }`
- `pub fn embedded() -> Vec<Migration>` — parsed from embedded dir, sorted, with
  a check that versions are contiguous from 1 and unique.
- `pub fn current_version(store) -> Result<u32>` — reads `smc:schemaVersion`
  (0 if unset).
- `pub fn pending(store) -> Result<Vec<&Migration>>` — versions `> current`.
- `pub fn apply_all(store) -> Result<Vec<u32>>` — for each pending in order:
  run the update, write the `smc:Migration` record + bump `smc:schemaVersion`,
  verifying the checksum of any already-recorded migration matches (drift guard).

### CLI

- `sparql-mcp migrate` → apply pending, print `applied: [N..M]` or `up to date`.
- `sparql-mcp migrate --status` → print current version, embedded max, pending list.
- `sparql-mcp migrate --dry-run` → list what would run without writing.

### Data versioning protocol (separate, private repo)

Documented in `plugins/kb-workbench/skills/kb-workbench/references/` (reuses the
gdrive-sync plumbing):

- `export_graph(urn:project:<slug>)` → `<slug>.ttl` committed to the **private**
  `mcazerty-data` repo (or pushed to GDrive). One commit per sync.
- The private repo is the version history of the *data*; this public repo is the
  version history of the *code + schema migrations*. They never mix.
- A `.gitignore` / pre-commit guard in the public repo should reject any
  `*.ttl` containing `urn:project:` data (guard against accidental commits).

## Open questions (for review)

1. Embed migrations (`include_dir`) vs read from a runtime `migrations/` dir?
   (Spec assumes embed — better for the static-binary install story.)
2. `migrate` auto-run on `serve` startup now, or stay explicit in v1?
   (Spec: explicit in v1.)
3. Private data repo: plain git (`mcazerty-data`) vs lean entirely on the
   existing GDrive sync? (Spec keeps both open; the guard is the important part.)

## Testing strategy (TDD, once approved)

- `embedded()` rejects non-contiguous/duplicate versions (unit).
- `current_version` = 0 on a fresh store; reflects the record after apply.
- `apply_all` on a fresh in-memory store runs all, bumps version, is idempotent
  on second call (returns empty).
- A migration that backfills `smc:description` produces the expected triples.
- checksum-drift guard: tampering with a recorded checksum is detected.
