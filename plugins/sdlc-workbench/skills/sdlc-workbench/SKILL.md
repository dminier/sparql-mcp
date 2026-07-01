---
name: sdlc-workbench
description: |
  Domain skill that tracks the whole software-delivery lifecycle (spec, plan,
  review, QA, ship, retro, learning) as graph entities in sparql-mcp, so a
  multi-agent team working on this repo has a shared, queryable memory of
  what was specified, reviewed, qualified, shipped, and learned. Built on top
  of kb-workbench's generic ingest/store/audit/render loop — this skill only
  adds the sdlc: ontology, its audit rules, its SPARQL catalog, and the
  persona skills that read/write it. Ships as a generic, self-contained
  skill (no external product dependency); dogfooded first against this
  repo's own graph `<urn:project:sparql-mcp-dev>`.

  Triggers (EN): "write a spec", "plan this feature", "review this spec/plan",
  "run QA on", "record a QA finding", "ship this", "record a release",
  "run a retro", "capture a learning", "sdlc dashboard", "sdlc audit",
  "who reviewed this plan", "what shipped last".
  Triggers (FR): "écris une spec", "planifie cette fonctionnalité", "revue de
  cette spec/plan", "lance la QA sur", "enregistre un défaut QA", "ship ça",
  "enregistre une release", "fais une rétro", "capture un apprentissage",
  "tableau de bord sdlc", "audit sdlc", "qui a revu ce plan", "qu'est-ce qui a
  été shippé".

  Load this skill whenever work touches the delivery lifecycle of *this*
  repo (or, generically, of any project using the same ontology) — writing a
  spec, planning, reviewing, qualifying, shipping, or running a retro. This
  skill defers all generic KB plumbing (SPARQL-first doctrine, ingestion,
  ontology stewardship, audit runner, Obsidian rendering) to `kb-workbench`.
---

# sdlc-workbench — SDLC lifecycle as a graph

Follows the "How domain skills use this skill" contract from
`plugins/kb-workbench/skills/kb-workbench/SKILL.md`. Read that skill first —
this file only states what is specific to the SDLC domain.

## 1. Ontology

- `ontology/2-sdlc.ttl` (L2) — `sdlc:Spec`, `sdlc:Plan`, `sdlc:Review`,
  `sdlc:QARun`, `sdlc:QAFinding`, `sdlc:ShipEvent`, `sdlc:Retro`,
  `sdlc:Learning`, `sdlc:AgentPersona`, and the typed relations
  `sdlc:derivesFrom`, `sdlc:reviewedBy`, `sdlc:qualifiedBy`, `sdlc:foundIn`,
  `sdlc:learnedFrom`, `sdlc:backedBySkill`.
- `ontology/3-sdlc-personas.ttl` (L3) — the concrete persona catalog for this
  workspace (`sdlc:CEOReviewerPersona`, `sdlc:EngReviewerPersona`,
  `sdlc:DesignReviewerPersona`, `sdlc:DevExReviewerPersona`,
  `sdlc:QALeadPersona`, `sdlc:SecurityOfficerPersona`,
  `sdlc:ReleaseEngineerPersona`), each carrying the 5 mandatory facets
  (`sourcing`, `sovereigntyTier`, `maturity`, `adoptionState`, `mcpNative`)
  per `references/ontology-design.md` in kb-workbench.

Load both files with `mcp__sparql-mcp__load_ontology_file` before the first
write in a fresh store.

## 2. Prefixes

`prefixes.yaml` declares `sdlc:` and `dct:` on top of kb-workbench's standard
registry.

## 3. Context bridge — no external preamble needed

sparql-mcp is already a first-class MCP tool, so no CLI wrapper or external
service is required to load context. Every persona skill under `skills/`
opens with a "contexte" step that calls `mcp__sparql-mcp__query_sparql`
directly, using a SELECT from `references/sparql-queries.md`, following the
visual protocol already in force in `sparql-first-code` (🔎 QUERY / ✏️ PUSH /
✅ OUTCOME banners, query text always shown verbatim).

Degradation: if `query_sparql` is unreachable, fall back read-only to
`scripts/sparql_boilerplate.py` (kb-workbench) and say so explicitly — see
`references/sparql-first.md` § Offline fallback.

## 4. Persona skills

Under `skills/<name>/SKILL.md`, one per lifecycle step. Each persona skill:

1. Runs its "contexte" SELECT(s) from `references/sparql-queries.md`.
2. Applies its own domain methodology (review checklist, QA criteria, retro
   questions — defined in that skill's own file, not here).
3. Persists its output as an `INSERT DATA` into `urn:project:sparql-mcp-dev`
   (or the target project graph), verified by a follow-up SELECT — "store
   first, verify, ask before TTL" per `references/sparql-first.md`.

Delivery order (pilot per cycle, then extended — both now landed):

| Cycle | Pilot | Extension |
|---|---|---|
| Plan | `/spec` → writes `sdlc:Spec` | `/review-ceo`, `/review-eng`, `/review-design`, `/review-devex` — persona-specific review lenses on top of `/review` |
| Qualité | `/review` → writes `sdlc:Review` | `/qa-run` (functional QA), `/qa-security` (`sdlc:SecurityOfficerPersona`) |
| Release + Rétro | `/retro` + `/learn` → write `sdlc:Retro`/`sdlc:Learning` | `/ship` (`sdlc:ReleaseEngineerPersona`) — the one skill that refuses to write without a passing `sdlc:QARun` |

Orchestrated planning (parallel review personas via the Agent tool),
deploy+health check, and session context save/restore remain future
extensions, not yet built.

## 5. Audit

`rules/audit.yaml` — P1-P6, run via kb-workbench's shared
`scripts/kb_audit.py --rules .claude/skills/sdlc-workbench/rules/audit.yaml
--project sparql-mcp-dev`. Re-run after every persona write and before every
vault sync, per the generic audit-framework doctrine.

## 6. Rendering

`render_spec.yaml` — project dashboard note (current phase, open Specs,
last ShipEvent, last Retro), a timeline canvas
(`Spec → Plan → Review → QARun → ShipEvent` per feature), and a decisions/retro
MOC with a per-persona breakdown (the agent-persona equivalent of
kb-workbench's per-person retro breakdown).

## 7. Multi-agent write concurrency (domain-specific constraint)

The Oxigraph/RocksDB store takes an **exclusive lock per directory**: only
one process may mutate it at a time. If an orchestrating skill spawns
several reviewer personas in parallel via the Agent tool (e.g. CEO + eng +
design + DevEx review), **every sub-agent must stay read-only**
(`query_sparql` only) and return its verdict as structured text to the
orchestrator. **Only the orchestrating agent performs `update_sparql`**,
sequentially, after collecting all verdicts. Do not parallelize writes —
this is the one rule specific enough to this domain that it must be
restated here rather than left implicit in kb-workbench's generic doctrine.

## File map

Packaged as the `sdlc-workbench` Claude Code plugin
(`plugins/sdlc-workbench/`), discoverable at `.claude/skills/sdlc-workbench`
via symlink, mirroring how `kb-workbench` is packaged:

```
plugins/sdlc-workbench/
├── .claude-plugin/plugin.json
├── README.md
└── skills/
    ├── sdlc-workbench/          ← this file + contract
    │   ├── SKILL.md
    │   ├── prefixes.yaml
    │   ├── references/
    │   │   ├── sparql-queries.md   ← SDLC-specific SELECT catalog
    │   │   └── personas.md         ← which persona backs which skill, and its facets
    │   ├── rules/audit.yaml        ← P1-P6
    │   └── render_spec.yaml
    ├── spec/SKILL.md
    ├── review/SKILL.md
    ├── review-ceo/SKILL.md
    ├── review-eng/SKILL.md
    ├── review-design/SKILL.md
    ├── review-devex/SKILL.md
    ├── qa-run/SKILL.md
    ├── qa-security/SKILL.md
    ├── ship/SKILL.md
    ├── retro/SKILL.md
    └── learn/SKILL.md
```

## Related skills

- **`kb-workbench`** — owns the generic ingest → store → steward → project
  loop, the ontology 3-layer pattern + 5 facets, the audit runner, and the
  Obsidian rendering primitives. This skill only adds the SDLC vocabulary
  and personas on top.
- **`sparql-first-code`** — the visual protocol (🔎/✏️/✅ banners) reused by
  every persona skill's context and persistence steps.
