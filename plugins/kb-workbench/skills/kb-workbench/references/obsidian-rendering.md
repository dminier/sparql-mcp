# Obsidian rendering

The vault is the human-facing projection of the KB. Rendering is the
reverse of ingestion: SPARQL SELECT → template → Markdown + Canvas.

## Render spec

Each domain skill ships a `render_spec.yaml`:

```yaml
# .claude/skills/<domain>/render_spec.yaml
vault_root: ~/Documents/kb-vault
entities:
  - type: hkb:Finding
    template: templates/finding-card.md
    folder: programs/{program_slug}/findings
    slug_predicate: hkb:slug
    canvas_pattern: grid
    auto_region:
      start: "<!-- kb:auto-start -->"
      end: "<!-- kb:auto-end -->"
    query: |
      SELECT ?finding ?slug ?severity ?cvss ?cwe ?summary
      WHERE { ... }

  - type: archimate:Capability
    template: templates/capability-card.md
    folder: Architecture/Capabilities
    canvas_pattern: layered
    query: ...
```

The generic renderer in `scripts/obsidian_render.py` walks the spec,
runs each SELECT, merges the result into the template, writes the card,
and regenerates the canvas.

## Folder-note convention

For each entity that has children (a finding with scripts/runs, a
capability with applications), generate:

```
<folder>/<slug>.md          ← the card, with a link to the folder
<folder>/<slug>/            ← contents (scripts, runs, attachments)
    finding_en.md
    scripts/
    runs/
```

Obsidian treats same-name file+directory as a folder-note pair: clicking
the folder opens the card.

## Marker-based preservation

Every generated card has two regions:

```markdown
<!-- kb:auto-start -->
... regenerated on every sync ...
<!-- kb:auto-end -->

<!-- kb-vault: hand-written notes below — preserved on sync -->

... the human's notes, never overwritten ...
```

The renderer replaces only the `kb:auto-*` region; everything below the
`kb-vault` marker is read, preserved, and re-appended.

## Canvas patterns

Each pattern is a pure-geometry placement function in
`scripts/canvas_layout.py`:

### `grid`

N cards laid out in a grid. Good for homogeneous collections (findings,
watch signals).

Parameters: `cell_width`, `cell_height`, `cols`, `gap`.

### `layered`

ArchiMate-style horizontal bands: Strategy / Business / Application /
Technology. Elements placed in the band matching their RDF type. Arrows
between elements based on `archimate:realizes` / `archimate:serves` /
`archimate:flowsTo`.

### `vendor-vs-oss`

Two vertical columns ("OSS" | "Éditeur"), one row per criterion, a
summary card floating above. Used in EA vendor comparisons.

### `trajectory`

ArchiMate plateaus (T0 / T+6M / T+12M / target) as vertical swim lanes;
migration arrows between them.

## TTL snapshot

After every render, the renderer calls
`mcp__sparql-mcp__export_graph(graph_iri="urn:project:<slug>")` and
drops the returned TTL at `<vault_root>/<folder>/<slug>.ttl`. This is
the ground-truth snapshot — future queries and migrations read it, not
the Markdown.

## Idempotency

Running the render twice with no KB changes produces byte-identical
output (modulo ordering, which is stable via SPARQL `ORDER BY`). This is
testable — CI can `obsidian_render.py && git diff --exit-code`.
