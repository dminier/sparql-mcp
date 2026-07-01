# sdlc-workbench

**The project's own software-delivery lifecycle, tracked as a graph.**
Spec → Plan → Review → QA → Ship → Retro → Learning, all as RDF entities
in [sparql-mcp](https://github.com/dminier/sparql-mcp), so a multi-agent
team working on this repo shares one queryable memory of what was
specified, reviewed, qualified, shipped, and learned.

`sdlc-workbench` is a Claude Code plugin built on top of `kb-workbench`:
it defers all generic KB plumbing (SPARQL-first doctrine, ontology
stewardship, audit runner, Obsidian rendering) to that plugin and adds
only the SDLC vocabulary and the persona skills that read/write it.

## Personas

Each lifecycle step is backed by an `sdlc:AgentPersona` in the L3
catalog (`ontology/3-sdlc-personas.ttl`) and a matching skill under
`skills/`:

| Cycle | Persona skills |
|---|---|
| Plan | `spec` |
| Quality | `review` (generic), `review-ceo`, `review-eng`, `review-design`, `review-devex`, `qa-run`, `qa-security` |
| Release + Retro | `ship`, `retro`, `learn` |

See [`skills/sdlc-workbench/SKILL.md`](skills/sdlc-workbench/SKILL.md)
for the full contract, ontology summary, and the multi-agent write-
concurrency rule (RocksDB store = single writer per directory).

## Install

```text
/plugin marketplace add dminier/sparql-mcp
/plugin install sdlc-workbench@sparql-mcp
```

Requires `kb-workbench` (same marketplace) and the `sparql-mcp` MCP
server to already be installed.

## Layout

```
sdlc-workbench/
├── .claude-plugin/plugin.json
├── README.md
└── skills/
    ├── sdlc-workbench/          ← contract, ontology summary, audit, rendering
    │   ├── SKILL.md
    │   ├── prefixes.yaml
    │   ├── references/{sparql-queries,personas}.md
    │   ├── rules/audit.yaml
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

## License

MIT.
