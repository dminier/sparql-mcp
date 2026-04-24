# SPARQL patterns — the shared toolbox

Common query shapes used by every domain. Domain-specific catalogs (bug
bounty endpoints, ArchiMate capabilities) live in each skill's
`references/sparql-queries.md`.

## Prefix registry

`scripts/sparql_boilerplate.py::PrefixRegistry` keeps the canonical set
and lets domain skills add their own:

```python
from sparql_boilerplate import PrefixRegistry
pr = PrefixRegistry.standard()
pr.extend(ywh="https://sparql-mcp.dev/ns/ywh#")
print(pr.preamble())  # ready to prepend to any query
```

## Project listing

```sparql
SELECT ?project ?slug ?label WHERE {
  GRAPH <urn:meta> {
    ?project a smc:Project ;
             smc:slug ?slug ;
             rdfs:label ?label .
  }
} ORDER BY ?slug
```

## Entity inventory by type

```sparql
SELECT ?type (COUNT(?s) AS ?n) WHERE {
  GRAPH <urn:project:SLUG> { ?s a ?type }
} GROUP BY ?type ORDER BY DESC(?n)
```

## Recent additions

```sparql
SELECT ?s ?type ?firstSeen WHERE {
  GRAPH <urn:project:SLUG> {
    ?s a ?type ;
       hkb:firstSeen ?firstSeen .
    FILTER (?firstSeen > (NOW() - "P7D"^^xsd:duration))
  }
} ORDER BY DESC(?firstSeen) LIMIT 50
```

## Cross-graph reference count

```sparql
SELECT ?g (COUNT(?s) AS ?n) WHERE {
  GRAPH ?g { ?s a hkb:Endpoint }
} GROUP BY ?g
```

## Unknown-predicate detector (used by ontology_propose.py)

```sparql
SELECT DISTINCT ?p WHERE {
  GRAPH <urn:project:SLUG> { ?s ?p ?o }
  FILTER NOT EXISTS {
    GRAPH <urn:ontology> { ?p a rdf:Property }
  }
}
```

## Staging → project promotion

```sparql
MOVE SILENT GRAPH <urn:staging:doc:20260423T200000Z>
          TO GRAPH <urn:project:ea>
```

(Requires no unknown-term flags; otherwise `DELETE WHERE { ... }` +
`INSERT { ... } WHERE { ... }` with alias rewriting.)
