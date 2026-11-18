# SPARQL-first doctrine

## The rule

Any question whose answer could live in the graph is answered by a SPARQL
query — first, before filesystem/grep/memory.

The graph is the single source of truth for structured knowledge:
projects, components, services, issues, capabilities, applications,
vendors, watch signals, decisions, recordings, observations —
whatever the domain. The filesystem holds projections (Obsidian notes,
TTL snapshots, capture dumps); it lags and can be partial.

## Order of operations for any read

1. **MCP SPARQL tools first** — `mcp__sparql-mcp__query_sparql`
   (SELECT / ASK / CONSTRUCT / DESCRIBE), `list_graphs`, `project_list`,
   `stats`. Display the full query text in the response.
2. **Offline fallback** when the MCP server is unreachable:
   `echo '<query>' | uv run python .claude/skills/kb-workbench/scripts/sparql_boilerplate.py run`.
   Same SPARQL, same prefixes, reads directly from the on-disk oxigraph
   store under `./store`. If you use this path, say so — the on-disk
   store can momentarily lag the live MCP server.
3. **TTL backup** — under `backups/<ts>/<slug>.ttl` — is a last resort
   and must be announced as such.
4. **Filesystem / CBM / grep** are for *source code*, *binary captures*,
   *capture artefacts*, *vault markdown* that the graph **points to** —
   never for entity metadata that belongs in the graph.

## Bulk loads: use an ingestion job

When a load needs more than a handful of calls (an analysis of a codebase, a
document set, an export), do not loop over `update_sparql` / `load_ontology`:

1. `mcp__sparql-mcp__ingest_begin` (default `mode: "staged"`); read the returned
   `limits`.
2. `mcp__sparql-mcp__ingest_batch` with `seq` 1, 2, 3, …, N-Triples data, about
   2,000–5,000 triples per batch. After **any** timeout or error, re-send the
   **same `seq`** with the same data: it is always safe, a batch that already landed
   answers `duplicate: true`. On `parse_error` or `limit_exceeded` nothing was
   recorded: fix or shrink the batch and re-send it under the same `seq` (the
   remainder of a split batch goes in new `seq`s at the end).
3. Optional: `mcp__sparql-mcp__ingest_validate` with checks that match the ontology (a
   read-only `SELECT` that should return nothing, or an `ASK`; `{{staging}}` and `{{target}}`
   name the two graphs). Then `ingest_commit` with `require_validation: true` refuses to
   move data that failed, or that changed after it was validated.
4. `mcp__sparql-mcp__ingest_commit`, then pass the reply's `new_subjects` to the
   viewer if one is connected. `ingest_abort` discards the job.
5. After a restart, `ingest_status` lists the received `seq`s: re-send the missing ones.

For a **large N-Triples file already on disk** (a dump, an export), do not read it into the
conversation: call `mcp__sparql-mcp__ingest_file` with its path (it must be inside the
server's `file_roots`; `file_roots_unset` means ask the user to configure it). It returns a
`job_id` at once and loads in the background; poll `ingest_status` (`file.percent`,
`triples_per_sec`, `eta_secs`). After a restart the job is `interrupted`: call `ingest_file`
again with the same `path` and `resume_job`. `already_loaded` means this exact file is
already in that graph. Turtle files are not streamed: convert them to N-Triples first.

Use `update_sparql` only for small, targeted edits.

## Order of operations for any write

1. Construct the triples in Turtle (`PREFIX ... INSERT DATA { ... }`).
2. **Display the full payload** in the response before sending.
3. Push via `mcp__sparql-mcp__update_sparql` (or `load_ontology` for
   schema-level triples).
4. **Verify** with a SELECT or ASK that the triples landed. The write reply
   gives `inserted_new` (quads that were not already there) and `delta`;
   `triples_before`/`triples_after` are `null` for `INSERT DATA`/`DELETE DATA`.
   A refusal comes back as an error result with a JSON `code`
   (`graph_protected`, `limit_exceeded`, `write_busy`, …): read it, do not retry
   blindly. `load_ontology` without `graph_iri` writes to the active project
   graph.
5. **Ask the user** before writing a versioned TTL export / snapshot to
   disk — the user may prefer to keep it ephemeral.

## Project isolation

Every project / domain uses its own **named graph**:
`<urn:project:<slug>>`. Cross-graph SPARQL is fine for reference
patterns (e.g. "which components appear across multiple projects?")
but writes go to one graph only.

Metadata about projects themselves (onboarding date, scope, owners) lives
in `<urn:meta>`. Read it freely, but it is written only by `project_create`:
`update_sparql` and the load tools refuse `<urn:meta>`.

## Prefix discipline

Always declare the prefixes you use. The shared ones are in
`scripts/sparql_boilerplate.py::PrefixRegistry.STANDARD`; domain skills
extend them. Typical set:

```turtle
PREFIX smc:       <https://sparql-mcp.dev/ns#>
PREFIX kb:        <https://sparql-mcp.dev/ns/kb#>
PREFIX archimate: <https://purl.org/archimate#>
PREFIX rec:       <https://sparql-mcp.dev/ns/rec#>
PREFIX rdf:       <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs:      <http://www.w3.org/2000/01/rdf-schema#>
PREFIX owl:       <http://www.w3.org/2002/07/owl#>
PREFIX prov:      <http://www.w3.org/ns/prov#>
PREFIX xsd:       <http://www.w3.org/2001/XMLSchema#>
```

## Offline fallback details

`scripts/sparql_boilerplate.py` opens the on-disk oxigraph store in
read-only mode (no lock contention) when the MCP server is down. It
loads the ontology TTLs as background schema so class/type checks still
work. It does **not** apply writes — writes require the live server.

```bash
echo 'SELECT * WHERE { ?s a smc:Project } LIMIT 5' \
  | uv run python .claude/skills/kb-workbench/scripts/sparql_boilerplate.py run
```

## What never belongs in the graph

- Raw PII (user names, addresses, emails from scraped data). Describe,
  list, redact — never copy users' data into the KB. Provenance is a
  path (`prov:wasDerivedFrom <file:///...>`), not a payload.
- Large binary payloads. Reference them by path / hash.
- Secrets. Store them as `smc:SecretReference` with a path, never the
  literal value.
