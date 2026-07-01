---
name: retro
description: |
  Writes a new sdlc:Retro entry after a ShipEvent or another milestone.
  Pilot skill of the Release+Rétro cycle in sdlc-workbench, paired with
  `skills/learn` which captures the durable Learnings a Retro produces.

  Triggers (EN): "run a retro", "retro on this release", "what went well/
  wrong with".
  Triggers (FR): "fais une rétro", "rétro sur cette release", "qu'est-ce qui
  a bien/mal marché sur".
---

# /retro — write a Retro

Part of `sdlc-workbench`. Defers generic KB plumbing to `kb-workbench`; this
file only states what's specific to writing a `sdlc:Retro`.

## 1. Contexte

Pull the recent ShipEvent(s) and any open QAFindings the retro should cover
(🔎 QUERY banner, verbatim):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?ship ?version ?deployedAt WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?ship a sdlc:ShipEvent ; sdlc:version ?version ; sdlc:deployedAt ?deployedAt .
  }
} ORDER BY DESC(?deployedAt) LIMIT 5
```

## 2. Run the retro

Cover, per agent-persona rather than per human (this is a multi-agent
retro): what each persona involved (reviewers, QA lead, release engineer)
observed as going well, going wrong, and what should change next cycle.
Keep it honest and specific to what actually happened — no invented praise
or invented blame.

## 3. Persist

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#retro-<slug>> a sdlc:Retro ;
      dct:title "<title>" ;
      dct:description "<per-persona breakdown>" ;
      dct:date "<now>"^^xsd:dateTime .
  }
}
```

Push via `mcp__sparql-mcp__update_sparql`, then **verify** (✅ OUTCOME
banner) with a SELECT confirming the triples landed.

## 4. Next step

Immediately follow up with `skills/learn` to capture at least one durable
`sdlc:Learning` linked via `sdlc:learnedFrom` to this Retro — audit rule P4
(info) flags a Retro with none, and a retro that produces no lesson is a
missed opportunity, not a completed one.
