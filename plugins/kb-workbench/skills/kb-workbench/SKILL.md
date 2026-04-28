---
name: kb-workbench
description: |
  Semantic knowledge workbench backed by sparql-mcp (SPARQL + ontology) and
  Obsidian for human projection. Owns the generic "KB-first" workflow shared
  by every domain skill in this workspace (e.g. enterprise-architecture, research-notebook,
  incident-response, and any future domain): ingest from code / docs / web / recordings into a
  project-scoped named graph, maintain the ontology as a living artefact
  (detect unknown terms, propose candidate classes/predicates), audit graph
  integrity with rule-driven SPARQL, and render the result as Obsidian
  notes + canvas. SPARQL is the source of truth; Obsidian is a projection.

  Triggers (EN): "query the KB", "load this into the graph", "ingest this
  doc", "render the vault", "audit the graph", "propose ontology updates",
  "what's in the KB about X", "check for unknown predicates", "snapshot the
  graph", "canvas for domain X", "store first then project", "start the
  sparql bridge".
  Triggers (FR): "interroge la base", "charge dans le graphe", "ingère ce
  doc", "génère le vault", "audit du graphe", "propose des classes
  candidates", "qu'est-ce qu'il y a dans la KB sur X", "repère les
  prédicats inconnus", "snapshot du graphe".

  Load this skill whenever a task touches a SPARQL graph, an ontology, the
  Obsidian vault, or needs to ingest external information into the
  knowledge base — even when the user invokes a domain skill (domain plugins,
  research-notebook) that layers on top. Domain skills defer the KB
  plumbing to this one.

---

## GDrive sync

Triggered by: `kb sync`, `kb sync push`, `kb sync pull`, `kb sync status`,
`synchronise la kb`, `push to drive`, `sauvegarde`, `pull from drive`,
`restaure la kb`, `nouvelle machine`, `restore kb`, `état de la sync`, `dernière sync`.

→ Load `references/gdrive-sync.md` and follow the matching operation (push / pull / status / bootstrap).

# kb-workbench — the shared semantic backbone

Every domain skill in this workspace (enterprise-architecture, research-notebook, incident-response,
and any future domain) shares the same core loop:

```
ingest  →  store in SPARQL  →  steward the ontology  →  project to Obsidian
```

This skill owns that loop. Domain skills declare *what* belongs in their
ontology, *which* templates render their vault, and *which* audit rules
guard their invariants — this skill handles *how*.

## 1. SPARQL is the source of truth, Obsidian is a projection

Non-negotiable across every domain:

- **Query the graph first.** Before reading a file, running a shell
  command, or answering a question about any entity the graph might know,
  issue a SPARQL SELECT against `<urn:project:<slug>>`. The filesystem
  (issues dirs, capture output, architecture notes, vault markdown) is a
  projection of the graph, not an authority.
- **Display every SPARQL payload verbatim** in the response. The user
  wants to see the query text and the write payload; don't paraphrase.
- **Store first, verify with SELECT, then ask before writing versioned TTL.**
  Push triples via `mcp__sparql-mcp__update_sparql` or
  `mcp__sparql-mcp__load_ontology`, re-query to confirm the write landed,
  and only then offer to persist a snapshot under `output/snapshots/` or a
  domain-specific directory.
- **TTL is the canonical export.** Every vault sync exports the project
  graph via `mcp__sparql-mcp__export_graph(graph_iri="urn:project:<slug>")`
  and drops the result into the vault as a sibling of the Markdown notes.
  The Markdown is derived; the TTL is ground truth.

See `references/sparql-first.md` for the full doctrine and offline fallback.

## 2. Multi-source ingestion

New knowledge arrives from four kinds of source; each has an adapter:

| Source      | Tooling                                                        | Emits triples in         |
|-------------|----------------------------------------------------------------|--------------------------|
| `code`      | codebase-memory-mcp (`search_graph`, `get_code_snippet`, `trace_path`) | `<urn:staging:code:<ts>>`     |
| `doc`       | local PDF/MD/HTML → entity extraction                          | `<urn:staging:doc:<ts>>`      |
| `web`       | WebFetch / WebSearch / microsoft-docs / context7               | `<urn:staging:web:<ts>>`      |
| `recording` | Playwright session capture                                     | `<urn:staging:recording:<ts>>` |

Every adapter lands in a **staging graph** first. The agent reviews, then
promotes into `<urn:project:<slug>>` via an explicit `MOVE`/`INSERT` step.
This is the generalisation of the "store first, then ask to persist" rule.

See `references/ingestion.md` for the adapter contracts and
`scripts/kb_ingest.py` for the dispatcher.

## 3. The ontology is a living artefact

Every ingest introduces terms. Some fit the current ontology; some don't.
The skill treats unknown terms as **candidates to surface**, not errors to
swallow:

1. After every ingest, run `scripts/ontology_propose.py --project <slug>`.
2. The script enumerates predicates + classes used in the project graph,
   compares against the loaded ontology files (`ontology/*.ttl`), and
   emits `output/ontology/candidates-<ts>.ttl`:
   - unknown predicates, clustered by `(domain_class, range_type)`
   - unknown classes, with sample instances
   - structurally redundant pairs (two predicates with identical
     domain/range that could be merged)
3. Present candidates to the human: **accept** (promote to
   `ontology/<layer>.ttl`), **alias** (map to an existing term), or
   **defer** (keep in staging).

This makes the ontology evolve **with** the KB instead of lagging behind.
See `references/ontology-stewardship.md` for the promotion flow, and
`references/ontology-design.md` for the **shape** a good ontology should
take (3-layer split L1 Core EA / L2 Domain / L3 Specialized, the 5
cross-cutting facets every brick must carry — `sourcing`,
`sovereigntyTier`, `maturity`, `adoptionState`, `mcpNative` —, canonical
audit rules, and anti-patterns). Any new domain skill should start by
declaring its L2+L3 classes against that template.

## 4. Graph-integrity audits

Domain skills declare their invariants as YAML rules:

```yaml
- id: A1
  name: Orphan ApplicationComponent
  severity: blocker
  query: |
    SELECT ?c WHERE {
      ?c a archimate:ApplicationComponent .
      FILTER NOT EXISTS { ?c kb:role ?_ }
    }
```

`scripts/kb_audit.py --rules <file.yaml> --project <slug>` runs each query
and formats violations by severity. The agent re-runs audits after every
ingest and before every vault sync. See `references/audit-framework.md`.

## 5. Obsidian rendering (note + canvas)

The vault is the human face of the KB. Generic rendering primitives:

- **Folder-note convention** — for each entity, `<name>.md` (card) beside
  a `<name>/` directory (contents). Obsidian treats them as one.
- **Marker-based preservation** — auto-generated regions sit between
  `<!-- kb:auto-start -->` / `<!-- kb:auto-end -->`. Hand-written notes
  below a `<!-- kb-vault: hand-written notes below — preserved on sync -->`
  marker survive re-renders.
- **Canvas patterns** — `grid`, `layered` (ArchiMate bands), `vendor-vs-oss`
  (2 columns + top card), `trajectory` (ArchiMate plateaus). Pure geometry
  in `scripts/canvas_layout.py`.
- **TTL snapshot** — every sync copies the project graph into the vault
  as `<slug>.ttl`.

Domain skills supply a `render_spec.yaml` listing SELECTs → templates →
canvas patterns. The generic renderer handles the rest. See
`references/obsidian-rendering.md`, and see `references/obsidian-convention.md`
for the **tags taxonomy + frontmatter schema + MOC (Map of Content) pattern**
that makes the Obsidian graph view actually useful — without that
convention, the graph degenerates into an unreadable cloud. Every
render-to-Obsidian operation must emit frontmatter derived from facets,
MOC wikilinks, and refresh MOC files with Dataview blocks.

## 6. Transport

`sparql-mcp` is a STDIO MCP server: every Claude session spawns its own
child process. No daemon, no ports. If the server can't be spawned (e.g.
broken install), fall back to the on-disk store via
`scripts/sparql_boilerplate.py` (see `references/sparql-first.md` §
"Offline fallback").

The oxigraph store takes an exclusive rocksdb lock per directory, so only
one agent at a time can mutate the same store. For parallel multi-project
work, either give each project a distinct `[core] store` path in its
`sparql-mcp.toml`, or enable `per_project_store = true` (roadmap v0.2)
to auto-open a store per project slug under `$SPARQL_MCP_HOME`.

## File map

```
.claude/skills/kb-workbench/
├── SKILL.md                          ← this file
├── references/
│   ├── sparql-first.md               ← KB-first doctrine + offline fallback
│   ├── ingestion.md                  ← 4 adapters, staging-graph pattern
│   ├── ontology-design.md            ← 3-layer ontology pattern + 5 facets + audit rules template
│   ├── ontology-stewardship.md       ← candidate detection, promotion flow
│   ├── obsidian-rendering.md         ← note + canvas patterns
│   ├── obsidian-convention.md        ← tags taxonomy + frontmatter schema + MOC pattern (dense graph)
│   ├── audit-framework.md            ← rules-YAML spec, severity taxonomy
│   └── sparql-patterns.md            ← PrefixRegistry, canonical SELECTs
└── scripts/
    ├── sparql_boilerplate.py         ← execute_query, prefix map, TTL fallback
    ├── kb_ingest.py                  ← unified ingestion dispatcher
    ├── kb_audit.py                   ← rules-driven integrity checker
    ├── ontology_propose.py           ← unknown-term detector → candidates.ttl
    ├── obsidian_render.py            ← generic note + canvas renderer
    └── canvas_layout.py              ← grid / layered / vendor-vs-oss layouts
```

## How domain skills use this skill

A domain skill (e.g. `enterprise-architecture`, `research-notebook`) should:

1. Declare its ontology files under `ontology/*.ttl` (workspace root),
   ordered by dependency depth (1-foundation, 2-domain, 3-bridge).
   Structure the classes along the 3-layer template from
   `references/ontology-design.md` — don't flatten everything at one
   level.
2. Ensure every L2/L3 product-class brick carries the 5 mandatory facets
   (`sourcing`, `sovereigntyTier`, `maturity`, `adoptionState`,
   protocol-native flag). Without these, canvases and ADRs degenerate
   into hand-waving.
3. List the prefixes it uses in a small `prefixes.yaml` at its root.
4. Keep domain-specific SPARQL queries in `references/sparql-queries.md`
   (catalog) — not generic plumbing queries.
5. Provide a `render_spec.yaml` mapping entity types to templates + canvas
   patterns. Apply the Obsidian convention from
   `references/obsidian-convention.md`: every emitted note gets the full
   frontmatter schema (facets → tags), 3-6 MOC wikilinks in a
   `## Relations` section, and the skill refreshes the MOC hub notes.
6. Supply a `rules/audit.yaml` for its invariants. Start from the 10
   canonical rules in `references/ontology-design.md` §"Canonical audit
   rules", then layer domain-specific rules on top.
7. Restate only the rules truly specific to the domain (e.g.
   rate budgets, dry-run defaults, domain-specific safety constraints) — defer the KB-first
   doctrine, the ingestion contract, the ontology stewardship + design,
   the audit framework, the Obsidian convention, and the rendering
   primitives to this skill.
