# kb-workbench

**Semantic knowledge workbench for AI agents.** One skill, one contract:
**SPARQL is the source of truth. Obsidian is its human face.**

`kb-workbench` is a Claude Code / GitHub Copilot plugin that turns any
SPARQL store (oxigraph via [sparql-mcp](https://github.com/OWNER/sparql-mcp),
Jena, Blazegraph, or any MCP-compatible SPARQL server) into a shared,
ontology-aware knowledge base that agents can:

- **ingest** from code (via codebase-memory-mcp), local docs, the web,
  or browser recordings,
- **steward** — detect predicates/classes that don't fit the current
  ontology and propose candidate declarations,
- **audit** with rule-driven SPARQL (YAML → SELECT → violations),
- **project** to Obsidian as notes + canvas, with hand-written regions
  preserved across re-syncs.

Domain skills (bug-bounty, enterprise-architecture, research notebook,
…) defer the KB plumbing to this plugin and concentrate on their
vocabulary and their workflow.

---

## Why this exists

Every team that builds a semantic memory for agents rediscovers the
same plumbing: prefix registries, TTL-disk fallback, grid/layered
canvas layouts, folder-note conventions, staging-graph promotions,
unknown-term detection, integrity audits. `kb-workbench` packages that
plumbing once, correctly, so domain skills stay thin.

It also solves a concrete pain for Linux users running sparql-mcp:
**rocksdb holds an exclusive lock on the store**, so two STDIO-spawned
MCP clients can't share it. `kb-workbench` ships an SSE bridge script
so a single server serves every agent on the machine.

---

## Install

### Claude Code (recommended)

**1. Add the marketplace** (once):

```bash
# If hosted on GitHub:
/plugin marketplace add OWNER/sparql-mcp

# Or point at the local checkout during development:
/plugin marketplace add /path/to/sparql-mcp/plugins
```

**2. Install the plugin:**

```bash
/plugin install kb-workbench@sparql-mcp
```

Restart Claude Code. The skill auto-triggers on phrases like *"query
the KB"*, *"ingest this doc"*, *"audit the graph"*, *"interroge la
base"*, etc.

**3. Configure the MCP server** in your project's `.mcp.json`:

```json
{
  "mcpServers": {
    "sparql-mcp": {
      "type": "sse",
      "url": "http://127.0.0.1:7733/sse"
    }
  }
}
```

**4. Start the shared SSE bridge** (once per machine, survives shell exits):

```bash
bash ~/.claude/plugins/cache/kb-workbench/skills/kb-workbench/scripts/start_sparql_http.sh --bg
```

Multiple Claude Code sessions can now share the same `sparql-mcp` store
without rocksdb-lock conflicts.

### GitHub Copilot (VS Code)

Copilot doesn't have a native "skill" concept yet, but it speaks MCP and
supports repo-level custom instructions.

**1. Add the MCP server** to `.vscode/mcp.json`:

```json
{
  "servers": {
    "sparql-mcp": {
      "type": "sse",
      "url": "http://127.0.0.1:7733/sse"
    }
  }
}
```

**2. Register the skill content as Copilot instructions** — create
`.github/copilot-instructions.md`:

```markdown
# Copilot instructions — kb-workbench

Follow the KB-first doctrine from
`plugins/kb-workbench/skills/kb-workbench/SKILL.md` when the task
touches a SPARQL graph, an ontology, the Obsidian vault, or ingestion
from code / docs / web / recordings.

Detailed references:
- SPARQL-first doctrine: `plugins/kb-workbench/skills/kb-workbench/references/sparql-first.md`
- Ingestion contract: `.../references/ingestion.md`
- Ontology stewardship: `.../references/ontology-stewardship.md`
- Audit framework: `.../references/audit-framework.md`
- Obsidian rendering: `.../references/obsidian-rendering.md`
- SPARQL patterns: `.../references/sparql-patterns.md`
```

Copilot Chat picks these up automatically when operating in the repo.

**3. Start the shared bridge** (same as Claude):

```bash
bash plugins/kb-workbench/skills/kb-workbench/scripts/start_sparql_http.sh --bg
```

---

## Requirements (Linux)

- **Python ≥ 3.10** with [uv](https://github.com/astral-sh/uv)
- **`mcp-proxy`** (for the shared SSE bridge):
  ```bash
  uv tool install mcp-proxy
  ```
- **A SPARQL MCP server.** Out of the box, `kb-workbench` targets
  [sparql-mcp](https://github.com/OWNER/sparql-mcp) (Rust, oxigraph,
  `cargo build`). Any other MCP-speaking SPARQL endpoint works if it
  exposes the same five tools (`query_sparql`, `update_sparql`,
  `load_ontology`, `export_graph`, `stats`).
- **Obsidian** (optional — only needed for the human-projection side)
- **codebase-memory-mcp** (optional — only needed for the `code` ingest adapter)

---

## What the plugin ships

```
plugins/kb-workbench/
├── .claude-plugin/plugin.json       ← plugin manifest
├── README.md                        ← this file
└── skills/kb-workbench/
    ├── SKILL.md                     ← the skill itself (loaded by Claude on trigger)
    ├── references/
    │   ├── sparql-first.md          ← KB-first doctrine + offline fallback
    │   ├── ingestion.md             ← 4 adapters (code / doc / web / recording)
    │   ├── ontology-stewardship.md  ← unknown-term detection, candidate flow
    │   ├── audit-framework.md       ← YAML rules → SPARQL → violations
    │   ├── obsidian-rendering.md    ← note + canvas patterns
    │   └── sparql-patterns.md       ← PrefixRegistry, canonical SELECTs
    └── scripts/
        ├── start_sparql_http.sh     ← launch shared SSE bridge (`mcp-proxy`)
        ├── stop_sparql_http.sh      ← stop the bridge
        ├── sparql_boilerplate.py    ← SPARQL execution + prefix map + TTL fallback  [wip]
        ├── kb_ingest.py             ← unified ingestion dispatcher                   [wip]
        ├── kb_audit.py              ← rules-driven integrity checker                 [wip]
        ├── ontology_propose.py      ← unknown-term detector                          [wip]
        ├── obsidian_render.py       ← generic note + canvas renderer                [wip]
        └── canvas_layout.py         ← grid / layered / vendor-vs-oss geometry       [wip]
```

Scripts marked `[wip]` are being extracted from downstream domain skills
(e.g. bug-bounty, enterprise-architecture, research-notebook). Until
they land, domain skills keep their local copies.

---

## The shared SSE bridge — why and how

`sparql-mcp` is a **STDIO** MCP server: every Claude client spawns its
own child process. The oxigraph backend uses **rocksdb**, which takes
an exclusive lock on the store directory. Two STDIO clients therefore
deadlock on the lock — one succeeds, the other fails at startup with
`IO error: LOCK: Resource temporarily unavailable`.

`kb-workbench` works around this by running **one** sparql-mcp process
behind [`mcp-proxy`](https://github.com/sparfenyuk/mcp-proxy), exposed
as Server-Sent Events on `127.0.0.1:7733`. Every Claude / Copilot
session connects over SSE to that single server:

```
┌───────────────┐  ┌───────────────┐  ┌───────────────┐
│ Claude term 1 │  │ Claude term 2 │  │ Copilot VSCode│
└──────┬────────┘  └──────┬────────┘  └──────┬────────┘
       │                  │                  │
       └─────── SSE ──────┼─────── SSE ──────┘
                          │
                 ┌────────▼────────┐
                 │   mcp-proxy     │
                 │   127.0.0.1:7733│
                 └────────┬────────┘
                          │ stdio
                 ┌────────▼────────┐
                 │   sparql-mcp    │   (single rocksdb lock holder)
                 └────────┬────────┘
                          │
                    ┌─────▼─────┐
                    │ ./store   │
                    └───────────┘
```

Under the hood `start_sparql_http.sh` just runs:

```bash
RUST_LOG=off mcp-proxy --host 127.0.0.1 --port 7733 --pass-environment \
  -- ./target/debug/sparql-mcp serve --config sparql-mcp.toml
```

`RUST_LOG=off` is important — sparql-mcp's tracing would otherwise leak
onto stdout and corrupt the JSONRPC stream. The script takes care of
this for you.

---

## Quick smoke test

After installing the plugin and starting the bridge:

```bash
# Bridge responds with an SSE session endpoint
curl -sSN --max-time 1 http://127.0.0.1:7733/sse | head -2
# → event: endpoint
# → data: /messages/?session_id=...
```

From Claude Code, in a fresh session:

> *"Query the KB: what projects are in the meta graph?"*

The `kb-workbench` skill triggers and issues:

```sparql
PREFIX smc: <https://sparql-mcp.dev/ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?project ?slug ?label WHERE {
  GRAPH <urn:meta> {
    ?project a smc:Project ;
             smc:slug ?slug ;
             rdfs:label ?label .
  }
} ORDER BY ?slug
```

…and shows the full query text + result in the response, per the
KB-first doctrine.

---

## License

MIT. See `LICENSE` in the parent repository.
