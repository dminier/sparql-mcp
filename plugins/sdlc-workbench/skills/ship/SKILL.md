---
name: ship
description: |
  Writes a new sdlc:ShipEvent, linked via sdlc:qualifiedBy to the passing
  sdlc:QARun that qualifies it. Backs sdlc:ReleaseEngineerPersona.
  Extension of the Release+Rétro cycle in sdlc-workbench, the missing step
  between qualification and the /retro pilot.

  Triggers (EN): "ship this", "record a release", "log this deployment".
  Triggers (FR): "ship ça", "enregistre une release", "log ce déploiement".
---

# /ship — record a ShipEvent

Part of `sdlc-workbench`. Defers generic KB plumbing to `kb-workbench`;
this file only states what's specific to writing a `sdlc:ShipEvent`.

## 1. Contexte — confirm qualification before shipping

This is the one step where the contexte check is a **gate**, not just
background: audit rule P1 (blocker) exists precisely to prevent an
unqualified ship. Query for a passing QARun on the target Plan (🔎 QUERY
banner, verbatim):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?run ?qaStatus WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?run a sdlc:QARun ; sdlc:qaStatus ?qaStatus .
  }
} ORDER BY DESC(?qaStatus) LIMIT 5
```

Also check for any open `blocker`-severity `sdlc:QAFinding` tied to that
run:

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?finding WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?finding a sdlc:QAFinding ;
             sdlc:foundIn <urn:project:sparql-mcp-dev#qarun-<slug>> ;
             sdlc:severity "blocker" ;
             sdlc:findingStatus "open" .
  }
}
```

**If no `qaStatus "passed"` run exists, or a blocker finding is still
open, stop and say so** — do not record the ShipEvent anyway. This is the
one place in the whole skill set where the write should be refused rather
than merely flagged for later audit.

## 2. Persist

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#ship-<version>> a sdlc:ShipEvent ;
      sdlc:version "<version>" ;
      sdlc:commitSha "<sha>" ;
      sdlc:deployedAt "<now>"^^xsd:dateTime ;
      sdlc:qualifiedBy <urn:project:sparql-mcp-dev#qarun-<slug>> .
  }
}
```

Push via `mcp__sparql-mcp__update_sparql`, then **verify** (✅ OUTCOME
banner) with a SELECT confirming the `sdlc:ShipEvent`, its facets, and the
`sdlc:qualifiedBy` link all landed.

## 3. Next step

Point the user at `skills/retro` once the release has had time to settle —
a retro shortly after every ShipEvent is how this workbench's Release+Rétro
cycle stays closed-loop.
