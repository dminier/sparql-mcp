# Multi-source ingestion

Everything the KB learns enters through one of four adapters. Each adapter
writes to a **staging graph** (`<urn:staging:<source>:<ts>>`); a later
promotion step merges staging into the project graph after the human has
reviewed.

## The four adapters

### 1. `code` — structural intelligence from indexed repos

Source: codebase-memory-mcp (CBM) already indexes every repo under
`plugins/<domain>/<project>/code/*` and `plugins/<domain>/assets/code/*`.

Extraction path:
- `search_graph` / `query_graph` → list of nodes with qualified names
- `get_code_snippet(qn)` → file_path, start_line, end_line, complexity
- `trace_path(fn, mode=calls)` → caller/callee edges

Mapping to RDF:
- CBM `Function`/`Method`/`Class` → `code:Symbol` with `code:qualifiedName`,
  `code:filePath`, `code:startLine`, `code:complexity`
- CBM `CALLS` edge → `code:calls` predicate
- Bridge to domain: `kb:implementedBy`, `archimate:realizes`

**Never** grep a file inside an indexed repo; always go through CBM.
Grep gives you lines; CBM gives you the function boundary.

### 2. `doc` — local PDF / Markdown / HTML

Input: scope documents, architecture PDFs, internal specs, ADRs.

Path: `kb_ingest.py doc <file>` →
1. Extract text (PyMuPDF for PDF, markdown-it for .md, readability for HTML)
2. Chunk + entity extraction (prompt-driven — agent fills the blanks)
3. Emit `smc:Document`, `smc:DocumentChunk` + domain-specific entities
   (`kb:Asset`, `archimate:Capability`, …) with `prov:wasDerivedFrom`
   pointing at the source path.

### 3. `web` — watch signals + external references

Input: WebSearch results, WebFetch of blog/docs, microsoft-docs /
context7 queries.

Path: `kb_ingest.py web <url>` →
1. Fetch + extract (reuses `doc` pipeline internals)
2. Tag as `smc:WatchEvent` with `smc:observedAt`, `smc:sourceUrl`,
   `smc:signalType` (release, rachat, abandon, new-offer, CVE…)
3. Link to affected domain entities (vendor, brique, policy)

### 4. `recording` — Playwright session captures

Input: already existing session recordings under
`output/recordings/<ts>/`.

Path: `kb_ingest.py recording <dir>` →
- Replay the session JSON
- Emit `rec:Interaction`, `rec:HttpRequest`, `rec:HttpResponse`
- Derive `kb:Endpoint` candidates (method + host + path)
- Attach `prov:wasDerivedFrom` to the recording file.

## The staging → promotion flow

Every adapter writes to `<urn:staging:<source>:<ts>>`. After ingest:

1. Agent presents a diff summary: *N new classes of type X, M new
   predicates, K candidate entities, L alias suggestions*.
2. Human decides:
   - **promote** — `MOVE GRAPH <urn:staging:...> TO <urn:project:<slug>>`
   - **merge with alias** — rewrite unknown predicates to known ones
     during promotion (the `ontology_propose.py` output feeds this)
   - **discard** — `DROP GRAPH <urn:staging:...>`
3. Only after promotion does the vault render pick up the new data.

This is the generic data-discipline rule ("Store first, then
persist") applied to every source.

## When an adapter sees unknown terms

If the extraction produces a predicate or class not declared in
`ontology/*.ttl`, the adapter **does not abort** — it records the triple
in staging with full provenance. The ontology stewardship loop
(`ontology_propose.py`) sees it on the next audit pass and proposes a
declaration. This keeps ingestion resilient while making the drift
visible.
