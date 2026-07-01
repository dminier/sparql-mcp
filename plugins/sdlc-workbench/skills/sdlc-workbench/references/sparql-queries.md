# sdlc-workbench — SPARQL query catalog

Domain-specific SELECTs for the SDLC lifecycle graph, kept out of
kb-workbench's generic `references/sparql-patterns.md`. All queries target
`<urn:project:sparql-mcp-dev>` (or `<urn:project:SLUG>` for a generalized
target) and assume the `sdlc:` / `dct:` prefixes from `prefixes.yaml`.

Every persona skill's "contexte" step (see the parent `SKILL.md`) issues one
or more of these SELECTs against `mcp__sparql-mcp__query_sparql` before doing
any work, and displays the query verbatim per doctrine.

## Project dashboard — current phase snapshot

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
SELECT ?spec ?plan ?lastReview ?lastShip ?lastRetro WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    OPTIONAL { ?spec a sdlc:Spec }
    OPTIONAL { ?plan a sdlc:Plan }
    OPTIONAL {
      ?lastReview a sdlc:Review ; dct:date ?rd .
    } ORDER BY DESC(?rd) LIMIT 1
    OPTIONAL {
      ?lastShip a sdlc:ShipEvent ; sdlc:deployedAt ?sd .
    } ORDER BY DESC(?sd) LIMIT 1
    OPTIONAL {
      ?lastRetro a sdlc:Retro ; dct:date ?td .
    } ORDER BY DESC(?td) LIMIT 1
  }
}
```

## Open Specs without a Plan

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?spec WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?spec a sdlc:Spec .
    FILTER NOT EXISTS { ?plan sdlc:derivesFrom ?spec }
  }
}
```

## Review history for a given Spec or Plan

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?review ?reviewKind ?verdict ?date WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?target sdlc:reviewedBy ?review .
    ?review sdlc:reviewKind ?reviewKind ;
            sdlc:verdict ?verdict ;
            dct:date ?date .
  }
} ORDER BY DESC(?date)
```

Bind `?target` to the Spec or Plan IRI being inspected.

## Open QAFindings by severity

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?finding ?severity ?date WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?finding a sdlc:QAFinding ;
             sdlc:severity ?severity ;
             sdlc:findingStatus "open" ;
             dct:date ?date .
  }
} ORDER BY DESC(?severity) ?date
```

## ShipEvent timeline with qualifying QARun

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?ship ?version ?deployedAt ?run ?qaStatus WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?ship a sdlc:ShipEvent ;
          sdlc:version ?version ;
          sdlc:deployedAt ?deployedAt .
    OPTIONAL {
      ?ship sdlc:qualifiedBy ?run .
      ?run sdlc:qaStatus ?qaStatus .
    }
  }
} ORDER BY DESC(?deployedAt)
```

## Retro → Learning breakdown (for the retro/learning MOC)

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?retro ?learning WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?retro a sdlc:Retro .
    OPTIONAL { ?learning sdlc:learnedFrom ?retro }
  }
}
```

## Persona inventory (facet check, mirrors audit rule P5)

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?persona ?mcpNative ?maturity ?adoptionState WHERE {
  GRAPH <urn:ontology> {
    ?persona a sdlc:AgentPersona .
    OPTIONAL { ?persona sdlc:mcpNative ?mcpNative }
    OPTIONAL { ?persona sdlc:maturity ?maturity }
    OPTIONAL { ?persona sdlc:adoptionState ?adoptionState }
  }
}
```

Adjust the `GRAPH` clause to wherever the ontology TTLs get loaded
(`load_ontology_file` target) versus the project data graph.
