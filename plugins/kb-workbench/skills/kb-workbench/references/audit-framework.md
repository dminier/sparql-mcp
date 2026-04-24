# Graph-integrity audit framework

A rules-YAML driven SPARQL linter. Domain skills declare invariants; the
framework runs them.

## Rule file schema

```yaml
# .claude/skills/<domain>/rules/audit.yaml
- id: A1                              # short ID, referenced in reports
  name: Orphan ApplicationComponent   # human-readable
  severity: blocker                   # blocker | warning | info
  description: >-
    ApplicationComponent without hkb:role is unusable — roles drive the
    triage pipeline.
  query: |
    SELECT ?c WHERE {
      ?c a archimate:ApplicationComponent .
      FILTER NOT EXISTS { ?c hkb:role ?_ }
    }
  remediation: >-
    Set hkb:role to one of: frontend | api | worker | storage.
```

## Severity taxonomy

| Severity  | Meaning                                                                |
|-----------|------------------------------------------------------------------------|
| `blocker` | graph is inconsistent or unusable; downstream renders will misbehave.  |
| `warning` | missing metadata that the workflow depends on; fix before next phase.  |
| `info`    | cosmetic or best-practice; fix when convenient.                        |

## Running audits

```bash
uv run python .claude/skills/kb-workbench/scripts/kb_audit.py \
    --rules .claude/skills/<domain>/rules/audit.yaml \
    --project example-project
```

Output formats: `text` (default), `json`, `markdown` (suitable for
pasting into Obsidian).

The script returns:
- exit code `2` if any `blocker` fires
- exit code `1` if any `warning` fires (no blockers)
- exit code `0` if only `info` (or nothing) fires

## When to run

- After every ingest promotion.
- Before every vault sync.
- On CI, if the project graph is versioned (`backups/<ts>/*.ttl`).

## Writing good rules

- **One invariant per rule.** Combining checks into a single SELECT
  makes reports unhelpful.
- **Name the variable `?violation`** when possible — the default
  formatter looks for it to pick the canonical IRI to display.
- **Keep queries fast.** Rules run often; avoid cross-graph joins
  unless necessary.
- **Write remediation.** A rule that fires without telling you how to
  fix it adds noise, not signal.
