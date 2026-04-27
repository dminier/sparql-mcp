# kb-workbench

**Semantic knowledge workbench for AI agents.** One skill, one contract:
**SPARQL is the source of truth. Obsidian is its human face.**

`kb-workbench` is a Claude Code plugin that bundles the agent-facing
**skill** for the [sparql-mcp](https://github.com/dminier/sparql-mcp) server.
The MCP server itself is installed separately as a single static binary
(see the root README for the one-line installer).

Once both are installed, this skill lets any agent:

- **ingest** from code (via codebase-memory-mcp), local docs, the web,
  or browser recordings,
- **steward** — detect predicates/classes that don't fit the current
  ontology and propose candidate declarations,
- **audit** with rule-driven SPARQL (YAML → SELECT → violations),
- **project** to Obsidian as notes + canvas, with hand-written regions
  preserved across re-syncs.

Domain plugins (enterprise-architecture, research-notebook,
incident-response, …) defer the KB plumbing to this one and concentrate on their
vocabulary and workflow.

---

## Why this exists

Every team that builds a semantic memory for agents rediscovers the
same plumbing: prefix registries, TTL-disk fallback, grid/layered
canvas layouts, folder-note conventions, staging-graph promotions,
unknown-term detection, integrity audits. `kb-workbench` packages that
plumbing once, correctly, so domain skills stay thin.

## Install

### 1. Install the MCP server binary (one-time, workstation-wide)

```bash
curl -fsSL https://raw.githubusercontent.com/dminier/sparql-mcp/main/install.sh | bash
```

This drops `sparql-mcp` into `~/.local/bin` and registers it as a STDIO
MCP server in every detected agent config (Claude Code, Codex, Gemini).

### 2. Install this plugin (Claude Code)

```text
/plugin marketplace add dminier/sparql-mcp
/plugin install kb-workbench@sparql-mcp
```

Restart Claude Code. The skill auto-triggers on phrases like *"query
the KB"*, *"ingest this doc"*, *"audit the graph"*, *"interroge la
base"*, etc.

## Multi-project on one workstation

v0.1 keeps one RocksDB store per working directory (set via
`[core] store` in `sparql-mcp.toml`). If you work on several projects
in parallel, give each one its own config file pointing at a distinct
store directory, or wait for v0.2 which opens stores lazily per
project under `$SPARQL_MCP_HOME/projects/<slug>/`.

## What the skill teaches the agent

See [`skills/kb-workbench/SKILL.md`](skills/kb-workbench/SKILL.md) for
the full workflow. In short:

1. SPARQL is the source of truth. Obsidian is regenerated from it.
2. Every ingestion pass lands in a staging graph, is audited, then
   promoted to the project graph.
3. Unknown predicates / classes are flagged, not silently accepted —
   the skill proposes candidate TTL declarations the user reviews.
4. The canvas layout is derived deterministically from the graph so
   re-renders don't shuffle nodes.

## Layout

```
kb-workbench/
├── .claude-plugin/plugin.json
├── README.md
└── skills/kb-workbench/
    ├── SKILL.md
    └── references/
        ├── audit-framework.md
        ├── ingestion.md
        ├── obsidian-convention.md
        ├── obsidian-rendering.md
        ├── ontology-design.md
        ├── ontology-stewardship.md
        ├── sparql-first.md
        └── sparql-patterns.md
```

## License

MIT.
