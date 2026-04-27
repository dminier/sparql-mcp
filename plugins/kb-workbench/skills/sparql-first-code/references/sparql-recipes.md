# SPARQL recipes for code reasoning

All examples assume `PREFIX cbm: <http://codebase-memory.dev/ontology#>` and
`PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>`.

Scope a query to one snapshot with `GRAPH <urn:cbm:repo:date:sha> { … }` unless
you're doing history/diff work.

## Find the latest snapshot of a repo

```sparql
SELECT ?g ?prov ?sha ?ingestedAt WHERE {
  GRAPH <urn:cbm:meta> {
    ?g a cbm:CodeIngestion ;
       cbm:repoPath "/abs/path" ;
       cbm:provenance ?prov ;
       cbm:ingestedAt ?ingestedAt .
    OPTIONAL { ?g cbm:gitCommit ?sha }
    FILTER NOT EXISTS { ?g cbm:purgedAt ?_ }
  }
} ORDER BY DESC(?ingestedAt) LIMIT 1
```

## Find a function by name

```sparql
SELECT ?fn ?file ?line WHERE {
  GRAPH <urn:cbm:demo-app:2026-04-27:a1b2c3d> {
    ?fn a cbm:Function ;
        cbm:name "handleLogin" ;
        cbm:filePath ?file ;
        cbm:startLine ?line .
  }
}
```

## Callers of a function (1 hop)

```sparql
SELECT ?caller ?callerName WHERE {
  GRAPH ?g {
    ?target cbm:name "handleLogin" .
    ?caller cbm:calls ?target ;
            cbm:name ?callerName .
  }
}
```

## Callees (transitive, up to 3 hops)

```sparql
SELECT DISTINCT ?callee WHERE {
  GRAPH ?g {
    ?source cbm:name "handleLogin" .
    ?source cbm:calls{1,3} ?callee .   # only on stores supporting property paths
  }
}
```

## Fan-out > 20 (refactor candidates)

```sparql
SELECT ?fn (COUNT(?callee) AS ?n) WHERE {
  GRAPH ?g {
    ?fn a cbm:Function ; cbm:calls ?callee .
  }
} GROUP BY ?fn HAVING (?n > 20) ORDER BY DESC(?n)
```

## Dead code (no incoming CALLS, not a route)

```sparql
SELECT ?fn ?name WHERE {
  GRAPH ?g {
    ?fn a cbm:Function ; cbm:name ?name .
    FILTER NOT EXISTS { ?_ cbm:calls ?fn }
    FILTER NOT EXISTS { ?_ cbm:definesRoute ?fn }
  }
}
```

## Diff between two snapshots of the same repo

```sparql
# functions removed between OLD and NEW
SELECT ?qn WHERE {
  GRAPH <urn:cbm:demo-app:2026-04-20:OLD> { ?fn cbm:qualifiedName ?qn ; a cbm:Function }
  FILTER NOT EXISTS {
    GRAPH <urn:cbm:demo-app:2026-04-27:NEW> { ?other cbm:qualifiedName ?qn }
  }
}
```

## Audit: which ingestions came from manual procedures?

```sparql
SELECT ?g ?repo ?prov ?note ?ingestedAt WHERE {
  GRAPH <urn:cbm:meta> {
    ?g a cbm:CodeIngestion ;
       cbm:repo ?repo ;
       cbm:provenance ?prov ;
       cbm:provenanceNote ?note ;
       cbm:ingestedAt ?ingestedAt .
    FILTER (?prov != "git")
  }
} ORDER BY DESC(?ingestedAt)
```
