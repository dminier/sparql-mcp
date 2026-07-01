---
name: spec
description: |
  Writes a new sdlc:Spec into the SDLC graph — the entry point of the whole
  lifecycle chain (Spec -> Plan -> Review -> QARun -> ShipEvent). Pilot skill
  of the Plan cycle in sdlc-workbench.

  Triggers (EN): "write a spec", "spec this feature", "draft a spec for",
  "record a spec".
  Triggers (FR): "écris une spec", "rédige une spec pour", "spécifie cette
  fonctionnalité", "enregistre une spec".
---

# /spec — write a Spec

Part of `sdlc-workbench`. Defers all generic KB plumbing (SPARQL-first
doctrine, store-first/verify/ask, prefix discipline) to `kb-workbench`; this
file only states what's specific to writing a `sdlc:Spec`.

## 1. Contexte

Before drafting anything, check for an existing Spec on the same topic so
you don't duplicate one — issue this SELECT (🔎 QUERY banner, verbatim) via
`mcp__sparql-mcp__query_sparql`:

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
SELECT ?spec ?title ?date WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?spec a sdlc:Spec ; dct:title ?title ; dct:date ?date .
    FILTER (CONTAINS(LCASE(?title), LCASE("<keyword>")))
  }
}
```

If a matching Spec exists, surface it and ask the user whether to extend it
or write a genuinely new one, rather than silently creating a duplicate.

## 2. Draft the Spec

Gather from the user (or infer from the conversation) and confirm before
writing:

- **Title** — short, unique.
- **Problem / context** — what's missing or broken today.
- **Goals** — what this Spec commits to.
- **Non-goals** — what it explicitly excludes (keeps scope honest).
- **Constraints** — technical or organizational limits that shape the Plan.

This is content, not ceremony — no dated file, no fixed template beyond the
four bullets above. Keep it as prose in the Spec's `dct:description`.

## 3. Persist

Construct the INSERT (✏️ PUSH banner, verbatim), using a fresh IRI under
`urn:project:sparql-mcp-dev#` (slug derived from the title):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#spec-<slug>> a sdlc:Spec ;
      dct:title "<title>" ;
      dct:description "<problem / goals / non-goals / constraints>" ;
      dct:date "<now>"^^xsd:dateTime .
  }
}
```

Push via `mcp__sparql-mcp__update_sparql`. Then **verify** (✅ OUTCOME
banner) with a SELECT on the same IRI, confirming the triples landed before
telling the user it's done.

## 4. Next step

Point the user at writing the corresponding `sdlc:Plan`
(`sdlc:derivesFrom` this Spec) once ready — that skill is not yet part of
this pilot batch; for now, `derivesFrom` links are created when the Plan
skill lands.
