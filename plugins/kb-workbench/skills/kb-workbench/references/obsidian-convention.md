# Obsidian convention — tags, frontmatter, MOCs

The Obsidian graph is a *projection* of the KB. To make it **actually
useful** (not just a pretty noise cloud), three things must be consistent
across every note the skill emits:

1. A **tag taxonomy** shared by all notes in the vault.
2. A **frontmatter schema** with the same facets the ontology declares.
3. **Maps of Content (MOCs)** — one hub note per ontology concept.

When these three are consistent, the Obsidian graph becomes a *tool*:
clusters emerge by domain, MOCs are the visible hubs, ADRs bridge
clusters, risks appear as a separate colored satellite.

## 1. Tag taxonomy (hierarchical, one axis per prefix)

Always hierarchical `parent/child`. One tag = one axis. A note carries
4-8 tags, one per applicable axis.

| Axis | Prefix | Examples |
|---|---|---|
| Artifact kind | `kind/` | `brick`, `moc`, `adr`, `note`, `watch`, `canvas`, `pattern`, `risk`, `capability`, `vendor` |
| ArchiMate layer | `layer/` | `strategy`, `business`, `application`, `technology`, `motivation`, `implementation` |
| Functional domain (L3) | `domain/` | `orchestration`, `mcp-gateway`, `mcp-server`, `llm`, `llm-gateway`, `rag`, `vector`, `embedding`, `memory`, `guardrails`, `eval`, `observability`, `governance`, `policy`, `idp` |
| Business domain (L2) | `<industry>/` | domain-specific categories (e.g. `operations`, `platform`, `compliance`) |
| Sourcing | `sourcing/` | `oss`, `vendor`, `hybrid` |
| Sovereignty | `sovereignty/` | `secnumcloud`, `bleu`, `s3ns`, `eu-hyperscaler`, `us-hyperscaler`, `on-prem`, `saas-global` |
| Maturity | `maturity/` | `poc`, `alpha`, `beta`, `rc`, `ga`, `deprecated` |
| Adoption | `adoption/` | `candidate`, `evaluated`, `selected`, `adopted`, `deprecated` |
| Vendor | `vendor/` | `ibm`, `microsoft`, `aws`, `google`, `salesforce`, `snowflake`, `databricks`, `anthropic`, `openai`, `oss-foundation` |
| OWASP / CWE risk | `risk/` | `llm01`, …, `llm10`, `cwe-XX` |
| Compliance | `compliance/` | `eu-ai-act`, `nist-ai-rmf`, `iso-42001`, `dora`, `acpr`, `solvabilite2`, `rgpd` |
| Population / principal | `population/` | depends on domain |
| Status | `status/` | `draft`, `reviewed`, `published`, `deprecated` |

**Minimum tag set for a `kind/brick` note**: `kind/brick` + `layer/*` +
`domain/*` + `sourcing/*` + `sovereignty/*` + `maturity/*` + `vendor/*`.
Others are optional but encouraged.

Why hierarchical? Obsidian's graph colorer can target prefixes, and
dataview can query `FROM #domain/*` to aggregate.

## 2. Frontmatter schema (standard, copy-paste)

```yaml
---
title: <human readable>
kind: brick | adr | moc | note | watch | canvas | pattern | risk | capability | vendor
iri: urn:ea:<project>:brick:<slug>       # backlink to KB
kb_graph: urn:project:<slug>
layer: application | technology | motivation | ...
domain: <L3 axis>                         # for agent-stack bricks
<industry>_domain: [list]                 # e.g. finance_domain: [trading]
sourcing: oss | vendor | hybrid
sovereignty: on-prem | secnumcloud | bleu | s3ns | eu-hyperscaler | us-hyperscaler | saas-global
maturity: poc | alpha | beta | rc | ga | deprecated
adoption: candidate | evaluated | selected | adopted | deprecated
vendor: <name>                            # free text + tagged
mcp_native: true | false | partial
realizes: [<capability>, ...]             # wikilink targets
mitigates: [<risk-id>, ...]
compliance: [eu-ai-act, ...]
populations: [<principal>, ...]
related_moc: [MOC — RAG, MOC — Souveraineté]     # wikilink list
related_adr: [ADR-001, ADR-002]
tags: [kind/brick, layer/application, domain/rag, sourcing/vendor, ...]
status: draft
date: 2026-04-23
---
```

All frontmatter keys are queryable through **Dataview**. Keep values as
lists, not comma-separated strings, for reliable filtering.

## 3. Maps of Content (MOCs) — the hubs

A MOC is a note with **no original content**. Its job:
- expose the ontology definition of a concept,
- auto-list notes tagged with it (via Dataview),
- link horizontally to sibling MOCs.

Convention:
- Path: `_Ontology/MOC/MOC — <Concept>.md`
- Frontmatter: `kind: moc`, tags `kind/moc` + one main axis tag.
- Body: brief definition → Dataview query → related MOC wikilinks.

**Canonical MOC set** (adapt to the domain):

```
MOC — Orchestration        MOC — MCP Gateway          MOC — MCP Server
MOC — LLM                  MOC — LLM Gateway          MOC — RAG
MOC — Vector Store         MOC — Embedding            MOC — Memory
MOC — Guardrails           MOC — Eval                 MOC — Observability
MOC — Governance           MOC — Policy Engine        MOC — IdP
MOC — Souveraineté         MOC — OSS vs Vendor        MOC — OWASP LLM
MOC — Populations          MOC — Domain B   MOC — ADR
MOC — Veille
```

## 4. Dataview queries that a MOC should embed

Full table of all bricks in a category:

````
```dataview
TABLE vendor, sovereignty, maturity, adoption, mcp_native AS "MCP"
FROM #kind/brick AND #domain/mcp-gateway
SORT adoption DESC, vendor ASC
```
````

Risk coverage map:

````
```dataview
LIST FROM #kind/brick AND #risk/llm06
```
````

ADR linked to a domain:

````
```dataview
TABLE status, date FROM #kind/adr WHERE contains(related_moc, "MOC — MCP Gateway")
```
````

## 5. Graph View — recommended config

*Graph view → Groups* (create these with distinct colors):

| Group query | Color | Role |
|---|---|---|
| `tag:#kind/moc` | red, large node | **Hubs** — pull everything together |
| `tag:#kind/adr` | yellow | Decisions — bridge domains |
| `tag:#domain/rag` | blue | RAG cluster |
| `tag:#domain/mcp-gateway` | purple | Gateway cluster |
| `tag:#domain/llm` | cyan | LLM cluster |
| `tag:#risk/*` | orange | Risks — satellite |
| `tag:#sovereignty/secnumcloud` OR `tag:#sovereignty/bleu` | dark green | French sovereign |
| `tag:#sourcing/oss` | light green | OSS halo |

*Graph view → Filters*: Depth 2 for local views; tune repulsion ~25 and
link force ~0.4 so hubs don't collapse onto each other.

## 6. Minimum viable refactor of an existing note

When retrofitting an old note that doesn't follow this convention:

1. Replace frontmatter with the schema in §2. Fill the 7 mandatory fields.
2. Append a **`## Relations`** section at the end listing 3-6 MOC wikilinks.
3. Keep the hand-written content between markers so it survives re-renders:
   ```
   <!-- kb:auto-start -->
   (auto sections — frontmatter tables, dataview blocks, link lists)
   <!-- kb:auto-end -->
   <!-- hand-written notes below — preserved on sync -->
   ```

## 7. Why this works (theory)

Two effects compound:

- **Tags create homogeneity** — Obsidian's graph colorer treats tags as
  first-class groups; a consistent prefix scheme yields visible clusters
  without any manual layout.
- **MOCs create attractors** — a note linked to 3-6 MOCs becomes
  multi-connected. Multi-connected nodes sit in the middle of their cluster
  naturally (force-directed layout). You didn't place them — you made them
  matter.

The cost is pure discipline: no new note without frontmatter, no new
concept without MOC. Enforce in code review. In practice: when the skill
emits a vault, it **generates the frontmatter and the MOC wikilinks from
the graph**, so the author doesn't have to remember.

## 8. Regen flow — the skill's responsibility

For every render-to-Obsidian operation, the skill should:

1. SELECT bricks from the project graph (with all facets).
2. Emit each brick note with the full frontmatter (from facets) + a
   `## Relations` section containing wikilinks to MOCs derived from
   `domain`, `sovereignty`, `sourcing`, `risk`, `compliance`.
3. Emit / refresh the MOC files — each MOC body contains a Dataview block
   (never hand-maintain the list of members).
4. Emit a snapshot `<project>.ttl` at the root of the vault.
5. Run the generic audit rules (see `audit-framework.md` and
   `ontology-design.md`) and surface violations as a note under
   `_Ontology/Meta/audit-<ts>.md`.
