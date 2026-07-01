---
name: learn
description: |
  Writes a new sdlc:Learning, linked via sdlc:learnedFrom to the Retro,
  QAFinding, or Review it came from. Pilot skill of the Release+Rétro cycle
  in sdlc-workbench, paired with `skills/retro`.

  Triggers (EN): "capture a learning", "what should we remember from",
  "record a lesson".
  Triggers (FR): "capture un apprentissage", "qu'est-ce qu'il faut retenir
  de", "enregistre une leçon".
---

# /learn — capture a Learning

Part of `sdlc-workbench`. Defers generic KB plumbing to `kb-workbench`; this
file only states what's specific to writing a `sdlc:Learning`.

## 1. Contexte

Resolve the source this Learning comes from — a Retro, a QAFinding, or a
Review — and check it isn't already covered by an existing Learning (🔎
QUERY banner, verbatim):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?learning WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?learning sdlc:learnedFrom <urn:project:sparql-mcp-dev#<source>> .
  }
}
```

## 2. Write the Learning

State the lesson as something actionable for a **future** Spec or Review —
not a restatement of what happened, but what should change because of it
(e.g. a review checklist item to add, a QA gap to cover, a constraint to
state up front next time).

## 3. Persist

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#learning-<slug>> a sdlc:Learning ;
      dct:description "<actionable lesson>" ;
      dct:date "<now>"^^xsd:dateTime ;
      sdlc:learnedFrom <urn:project:sparql-mcp-dev#<source>> .
  }
}
```

Push via `mcp__sparql-mcp__update_sparql`, then **verify** (✅ OUTCOME
banner) with a SELECT confirming both the `sdlc:Learning` and its
`sdlc:learnedFrom` link landed.

## 4. Where this feeds back

A `sdlc:Learning` has no automatic consumer yet — it's up to whoever writes
the next `sdlc:Spec` (via `/spec`) to query recent Learnings first and fold
them in. Until an orchestration skill does this automatically, say so
explicitly rather than implying it happens on its own.
