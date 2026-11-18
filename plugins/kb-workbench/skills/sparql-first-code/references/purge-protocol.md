# Purge protocol — manual only

Code ingestions are never purged automatically. The user must explicitly say
*"purge le code de plus de N jours"* / *"drop ingestions older than N months"*
with a duration. Without a duration, ask for one — do not guess.

## Prerequisite — destructive updates are opt-in

`DROP GRAPH` is refused unless the server runs with
`[ingest] allow_destructive_updates = true` in `sparql-mcp.toml`. Check first
without running anything: `mcp__sparql-mcp__update_sparql(update=<DROP …>, dry_run=true)`
returns `refused` (`destructive_update_forbidden` when the setting is off). If
it is refused, tell the user which setting to enable and wait; never try to work
around the refusal. `DROP ALL`, `CLEAR ALL` and any write to `urn:meta` or
`urn:hkb:meta:*` are refused whatever the setting.

## Flow

1. **Build the SELECT** for candidates:
   ```
   python3 scripts/purge_older_than.py --list --days 30
   ```
   Run the resulting query through `mcp__sparql-mcp__query_sparql`.

2. **Show the candidates** to the user as a table — repo, provenance,
   ingestedAt, age, nodeCount/edgeCount. Always include manual-provenance rows
   (Playwright/HAR/etc.) so the user can decide whether to keep specific
   captures regardless of age.

3. **Wait for explicit confirmation** ("ok", "go", "confirme"). A vague answer
   ("oui peut-être") is not a confirmation — re-ask.

4. **For each confirmed graph IRI**, run:
   ```
   python3 scripts/purge_older_than.py --drop <iri>
   ```
   Pipe the output into `mcp__sparql-mcp__update_sparql`. This both drops the
   data graph and annotates `cbm:purgedAt` on the meta record.

5. **Never delete the meta record.** Keeping it lets future SPARQL queries
   answer "was this code ever ingested?" even after the snapshot is gone.

## Refusing the wrong shape of request

- "fais le ménage" / "clean up" → ask for a duration, refuse to act.
- "drop everything" → ask for explicit per-IRI confirmation, do not run a
  blanket `CLEAR ALL`.
- "purge sauf prod" → out of scope; ask the user to filter by repo or
  provenance manually.
