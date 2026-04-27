---
name: sparql-first-code
description: Use this skill whenever the conversation involves reading, exploring, searching, tracing, or reasoning about source code in any indexed repository. The skill enforces a SPARQL-first workflow — code is ingested into sparql-mcp as a dated named graph (with git or alternative provenance metadata) and then queried via SPARQL instead of grep/Read. Trigger this skill on any code-discovery phrasing ("find function X", "what calls Y", "where is Z handled", "trace the auth flow", "is there dead code", "show me callers of…"), and on explicit user phrases like "IMPORT PLAYWRIGHT", "IMPORT HAR", or "purge le code de plus de N jours". Even when the user asks casually ("just grep for it"), prefer this skill — grep on indexed code wastes context. Only fall back to Grep/Read for non-code files (configs, markdown, JSON dashboards).
---

# sparql-first-code

## Why

Grep over a large codebase floods the context with low-signal text matches. A code knowledge graph (CALLS, DEFINES, IMPLEMENTS, USAGE) compresses the same information ~100×. We push everything we ever read into `sparql-mcp` as **dated named graphs**, attach **provenance metadata** (git commit *if available*, otherwise a verbal description), and reason via SPARQL. That gives:

- traceability — every SPARQL answer is tied to a specific commit (or a specific Playwright capture, HAR export, etc.)
- bounded context — query results return only what was asked
- explicit lifecycle — old code only disappears when the user asks for it

## Visual protocol — every SPARQL operation MUST be narrated

Every single call to `query_sparql` / `update_sparql` / `load_ontology` / `DROP GRAPH` is a *visible* event. The user wants to *see the workflow happen*, not just the answer. Before each call, emit a block that describes phase, intent, ontologies targeted, and the exact query. After the call, emit a one-line outcome.

### Phase banners

Open each phase with a visual banner so the user can scan the timeline:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📥 IMPORT  ·  demo-app @ master/00311c6  ·  step 2/4 turtle build
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

Phase glyphs (use exactly these so the user's eye locks onto them):

| Glyph | Phase |
|---|---|
| 📥 | IMPORT — index_repository / Turtle build / load_ontology |
| 🔎 | QUERY — read-only `query_sparql` (data **GET**) |
| ✏️ | PUSH  — `update_sparql` INSERT / DELETE / WHERE-modify (data **PUSH**) |
| 🧹 | PURGE — DROP GRAPH + meta annotation |
| ✅ | OUTCOME — one-line result (rows, triples added/removed, errors) |

### Per-request block — MANDATORY for every SPARQL call

Print this block **before** the tool call. No SPARQL call without it.

```
🔎 QUERY  ·  freshness check on /home/alice/projects/demo-app
**description** : Récupérer la dernière ingestion non purgée pour comparer le SHA courant et décider si une ré-ingestion est nécessaire.
**Domaines :**
   - <http://codebase-memory.dev/ontology#> : vocabulaire interne du graphe codebase (CodeIngestion, gitCommit, provenance, edge_type properties).
   - <urn:cbm:meta> : graphe-index dédié à l'historique des ingestions (jamais purgé).
**QUERY** :
```sparql
PREFIX cbm: <http://codebase-memory.dev/ontology#>
SELECT ?g ?prov ?sha ?ingestedAt WHERE {
  GRAPH <urn:cbm:meta> {
    ?g a cbm:CodeIngestion ;
       cbm:repoPath "/home/alice/projects/demo-app" ;
       cbm:provenance ?prov ;
       cbm:ingestedAt ?ingestedAt .
    OPTIONAL { ?g cbm:gitCommit ?sha }
    FILTER NOT EXISTS { ?g cbm:purgedAt ?_ }
  }
} ORDER BY DESC(?ingestedAt) LIMIT 1
```
```

Then call the tool. Then emit:

```
✅ 1 row · g=urn:cbm:demo-app:2026-04-27:00311c6 · sha=00311c6… · ingestedAt=2026-04-27T14:32:00Z
```

### Field rules

- **description** — *functional*, one or two sentences in French (the user's working language). Explain *why this query exists in the workflow*, not what SPARQL syntax does.
- **Domaines** — every IRI prefix that appears in the query, plus the named graphs read or written. One line of functional description per IRI. If a query touches both `cbm:` and `bbp:`, list both. Never list a prefix the query doesn't use.
- **QUERY** — verbatim SPARQL inside a fenced ` ```sparql ` block, fully formatted (uppercase keywords, indented WHERE, one triple per line). No truncation, no ellipsis — even a 60-line update goes in full.

### Update / push variant

Same block, swap glyph and add a *change preview* line:

```
✏️ PUSH  ·  insert ingestion meta for demo-app:2026-04-27:00311c6
**description** : Enregistrer dans `urn:cbm:meta` la traçabilité de l'ingestion courante : branche git, SHA, date du commit, état dirty, compteurs nodes/edges.
**Domaines :**
   - <http://codebase-memory.dev/ontology#> : ontologie d'ingestion (CodeIngestion + git*).
   - <urn:cbm:meta> : graphe-index, écriture additive — aucun triple existant supprimé.
**QUERY** :
```sparql
PREFIX cbm: <http://codebase-memory.dev/ontology#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
INSERT DATA { GRAPH <urn:cbm:meta> {
  <urn:cbm:demo-app:2026-04-27:00311c6> a cbm:CodeIngestion ;
    cbm:repo "demo-app" ;
    cbm:repoPath "/home/alice/projects/demo-app" ;
    cbm:ingestedAt "2026-04-27T14:32:00Z"^^xsd:dateTime ;
    cbm:provenance "git" ;
    cbm:gitBranch "master" ;
    cbm:gitCommit "00311c636cf70bbe7e89482a48f53fbcc31d7690" ;
    cbm:gitDirty "true"^^xsd:boolean ;
    cbm:nodeCount "277"^^xsd:integer ;
    cbm:edgeCount "276"^^xsd:integer .
} }
```
```
Outcome: `✅ +13 triples in urn:cbm:meta`.

### Purge variant — extra safety theatre

Purge is destructive, so the visual block carries one more line: `**Réversibilité :** non — DROP GRAPH supprime les triples du snapshot daté. Le triple méta est conservé avec cbm:purgedAt.` Always emit a `🧹 PURGE` banner and require explicit user confirmation **between the block and the call**.

### Why this matters

Two reasons. First, when the user pairs SPARQL skills with code work she needs to *audit what touched the store* — the block is the audit trail. Second, the per-query description forces *you* to articulate the functional intent before issuing the call; if you can't write the description in one sentence, the query is probably the wrong one.

This protocol overrides the default "be terse" tendency for SPARQL operations only. Keep prose elsewhere brief.

## Tools you must use (and not use)

- **Discovery / reasoning**: `mcp__sparql-mcp__query_sparql` only.
- **Ingestion**: `mcp__codebase-memory-mcp__index_repository` → `mcp__sparql-mcp__cbm_load_graph` (single MCP call: reads the cbm SQLite cache, converts to Turtle via the canonical `cbm/turtle.rs` emitter, loads into the target named graph) → `mcp__sparql-mcp__update_sparql` to attach metadata in `urn:cbm:meta`. The old JSON→Python→Turtle path is retired — `cbm_load_graph` is the supported bridge.
- **Forbidden for code exploration**: `Grep`, `Glob` over `.py/.ts/.tsx/.js/.jsx/.rs/.java/.kt/.go/.rb/.php/.cs/.cpp/.c/.swift/.m/.mm` files when an ingestion exists. `Read` is allowed for **specific** known paths to inspect a single file's exact contents — never to "look around".
- **Allowed without ingestion**: configs (`*.yml`, `*.json`, `*.toml`, `*.ini`, `*.env*`), docs (`*.md`, `*.rst`, `*.txt`), dashboards.

If the user explicitly says "grep for X" on indexed code, push back once and offer the SPARQL equivalent. If they insist, comply.

## Decision flow

```
user mentions code in repo R at path P
      │
      ▼
SPARQL: latest ingestion for repoPath = P ?
      │
   ┌──┴──┐
  yes    no  ────────────────────────────► ingest (see below)
   │
   ▼
provenance = git ?
   │
  yes ──► current HEAD == cbm:gitCommit ?
   │              │
   │           ┌──┴──┐
   │          yes    no ─► re-ingest (new named graph)
   │           │
   │           ▼
   │       query via SPARQL
   │
  no  ──► provenance is manual (PLAYWRIGHT/HAR/…) → re-ingestion only on explicit user request
            ▼
        query via SPARQL
```

## Named graph & metadata model

- **Dated graph IRI** (where code triples live):
  - git case:    `urn:cbm:<repo-slug>:<YYYY-MM-DD>:<short-sha>`
  - manual case: `urn:cbm:<repo-slug>:<YYYY-MM-DD>:<provenance-tag>` (e.g. `playwright-20260427T1432`, `har-2026-04-27`)
- **Meta graph**: `urn:cbm:meta` — never DROP'd by purge, only annotated.

Vocabulary (`@prefix cbm: <http://codebase-memory.dev/ontology#>`):

| Property | Required? | Notes |
|---|---|---|
| `cbm:repo` | yes | slug |
| `cbm:repoPath` | yes | absolute path on disk |
| `cbm:ingestedAt` | yes | xsd:dateTime, ISO-8601 |
| `cbm:provenance` | yes | one of `git`, `playwright-import`, `har-import`, `manual-archive`, `decompiled-bundle`, … |
| `cbm:provenanceNote` | recommended | free text — **always fill this when no git** ("Playwright capture of app.example.com session 2026-04-27 14:32, network tab + DOM dump") |
| `cbm:gitBranch` | optional | only when `provenance=git` |
| `cbm:gitCommit` | optional | full SHA |
| `cbm:gitCommitShort` | optional | |
| `cbm:gitCommitDate` | optional | xsd:dateTime |
| `cbm:gitCommitMsg` | optional | first line of subject |
| `cbm:gitDirty` | optional | xsd:boolean |
| `cbm:gitRemote` | optional | |
| `cbm:nodeCount`, `cbm:edgeCount` | yes | |
| `cbm:purgedAt` | added on purge | xsd:dateTime |

Rule: **never fabricate a SHA**. If git is unavailable or the user says "IMPORT PLAYWRIGHT" / "IMPORT HAR", set `provenance` to the matching tag and write a clear `provenanceNote` describing source, capture timestamp, and scope.

## Ingestion procedure

1. Resolve absolute repo path `P`.
2. Check freshness with SPARQL on `urn:cbm:meta`:
   ```sparql
   PREFIX cbm: <http://codebase-memory.dev/ontology#>
   SELECT ?g ?prov ?sha ?ingestedAt WHERE {
     GRAPH <urn:cbm:meta> {
       ?g a cbm:CodeIngestion ;
          cbm:repoPath "P" ;
          cbm:provenance ?prov ;
          cbm:ingestedAt ?ingestedAt .
       OPTIONAL { ?g cbm:gitCommit ?sha }
     }
   } ORDER BY DESC(?ingestedAt) LIMIT 1
   ```
3. If `prov=git`, run `scripts/git_meta.sh P` and compare `gitCommit` to `?sha`. Match → done. Otherwise, ingest.
4. `mcp__codebase-memory-mcp__index_repository(repo_path=P, mode="full")` — **always `full`, never `moderate`/`fast` before a SPARQL ingestion**. The `fast`/`moderate` modes apply `FAST_SKIP_DIRS` (defined in cbm `src/discover/discover.c`) which silently drops `scripts/`, `tools/`, `bin/`, `build/`, `docs/`, `examples/`, `migrations/`, `assets/`, `static/`, `public/`, `e2e/`, `__tests__/`, etc. Those directories almost always contain **operational code we want to reason about** — data scripts, migrations, test fixtures referenced by code under test. Indexing without them produces a partial graph that *looks* complete (no error, no warning) but answers wrong to "who calls X" queries. Only `full` walks every directory. Cost is acceptable: cbm-mcp is incremental, the second run on an unchanged tree is near-instant.
5. `mcp__sparql-mcp__cbm_load_graph(repo_path=P, graph_iri=<urn:cbm:repo:date:sha>)` — one call. The tool reads the cbm SQLite cache (`~/.cache/codebase-memory-mcp/<slug>.db`), serialises with the canonical Turtle emitter (`cbm/turtle.rs`), and loads into the named graph. Returns `{ nodes, edges, triples_before, triples_after, delta }`. Use those numbers for the meta insert below.
6. Build the metadata triples (see `scripts/meta_insert.py`) — git case takes the JSON from `git_meta.sh`, manual case takes a `--provenance <tag> --note "<text>"` pair you provide. Pass `--node-count` and `--edge-count` from the previous step's response.
7. `mcp__sparql-mcp__update_sparql("INSERT DATA { GRAPH <urn:cbm:meta> { ... } }")`.

The `scripts/nodes_to_turtle.py` helper is kept only as a fallback for when `cbm_load_graph` is not available (older sparql-mcp build). Prefer the MCP tool — it's batched, fast, and never drops edge types.

## Querying — recipes

See `references/sparql-recipes.md`. Core patterns to memorize:

- **callers of `X`**: `?caller cbm:calls ?x . ?x cbm:name "X"`
- **callees of `X`**: invert the triple
- **dead code**: function with no incoming `cbm:calls` and no `cbm:definesRoute` parent
- **fan-out > N**: `GROUP BY ?fn HAVING (COUNT(?callee) > N)`
- **cross-graph diff** (audit): `MINUS` between two dated graphs of the same repo

Always scope by `GRAPH <urn:cbm:repo:date:sha>` when reasoning about a single snapshot. Use `GRAPH ?g` only for diff/history queries.

## Purge — strict manual protocol

Trigger phrase must include an explicit duration: *"purge le code de plus de 30 jours"*, *"drop ingestions older than 3 months"*. Vague phrasings ("clean up", "fais le ménage") → refuse and ask for a duration.

Procedure: `scripts/purge_older_than.py` — see `references/purge-protocol.md` for full flow. Always **dry-run first**, show the list (repo, provenance, age, triples), wait for explicit "ok", then `DROP GRAPH` each + annotate `cbm:purgedAt` on the meta entry. Never delete the meta entry — keeping it preserves the historical fact that the snapshot existed.

## Verbal-provenance examples

When git isn't available, the `provenanceNote` is the only thing future-you will have to know what this graph contains. Be specific.

**Example 1 — Playwright capture:**
```
provenance:     "playwright-import"
provenanceNote: "Playwright headed run on https://app.example.com/dashboard
                 2026-04-27T14:32Z. Captured: 47 XHR responses (JSON bodies kept),
                 full DOM after auth as user alice, no service-worker scripts.
                 Source files under /home/alice/.../recordings/20260427T1432Z/."
```

**Example 2 — HAR archive import:**
```
provenance:     "har-import"
provenanceNote: "HAR export from Chrome DevTools 2026-04-27, app.example.com
                 staging environment, 1284 requests captured during E2E
                 walkthrough. JS bundles pretty-printed before storage; no
                 decompilation."
```

**Example 3 — decompiled bundle:**
```
provenance:     "decompiled-bundle"
provenanceNote: "iOS .ipa for ExampleApp 8.4.1 build 2026-04-20, decompiled
                 with class-dump + Hopper. Only Objective-C/Swift symbol surface,
                 no string xrefs."
```

These notes go into `cbm:provenanceNote` literally — they are how SPARQL queries against `urn:cbm:meta` will surface ingestion context to future sessions.
