# SPARQL-first doctrine

## The rule

Any question whose answer could live in the graph is answered by a SPARQL
query — first, before filesystem/grep/memory.

The graph is the single source of truth for structured knowledge:
programs, assets, endpoints, findings, capabilities, applications,
vendors, watch signals, decisions, recordings, probes, observations —
whatever the domain. The filesystem holds projections (Obsidian notes,
TTL snapshots, recon dumps); it lags and can be partial.

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
   *recon artefacts*, *vault markdown* that the graph **points to** —
   never for entity metadata that belongs in the graph.

## Order of operations for any write

1. Construct the triples in Turtle (`PREFIX ... INSERT DATA { ... }`).
2. **Display the full payload** in the response before sending.
3. Push via `mcp__sparql-mcp__update_sparql` (or `load_ontology` for
   schema-level triples).
4. **Verify** with a SELECT or ASK that the triples landed.
5. **Ask the user** before writing a versioned TTL export / snapshot to
   disk — the user may prefer to keep it ephemeral.

## Project isolation

Every program / engagement / domain uses its own **named graph**:
`<urn:project:<slug>>`. Cross-graph SPARQL is fine for reference
patterns (e.g. "which assets appear across multiple project scopes?")
but writes go to one graph only.

Metadata about projects themselves (onboarding date, scope, owners) lives
in `<urn:meta>`.

## Prefix discipline

Always declare the prefixes you use. The shared ones are in
`scripts/sparql_boilerplate.py::PrefixRegistry.STANDARD`; domain skills
extend them. Typical set:

```turtle
PREFIX smc:       <https://sparql-mcp.dev/ns#>
PREFIX hkb:       <https://sparql-mcp.dev/ns/hkb#>
PREFIX archimate: <https://purl.org/archimate#>
PREFIX ywh:       <https://sparql-mcp.dev/ns/ywh#>
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
