---
name: review
description: |
  Writes a new sdlc:Review verdict against an existing sdlc:Spec or
  sdlc:Plan, and links it via sdlc:reviewedBy. Pilot skill of the Quality
  cycle in sdlc-workbench; reviewKind-specific persona skills (CEO/eng/
  design/DevEx/security) extend this generic pilot later.

  Triggers (EN): "review this spec", "review this plan", "run a review on",
  "what's the verdict on".
  Triggers (FR): "revois cette spec", "revois ce plan", "lance une revue
  sur", "quel est le verdict sur".
---

# /review — write a Review

Part of `sdlc-workbench`. Defers generic KB plumbing to `kb-workbench`; this
file only states what's specific to writing a `sdlc:Review`.

## 1. Contexte

Resolve the target (Spec or Plan) and its existing review history (🔎
QUERY banner, verbatim):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
SELECT ?review ?reviewKind ?verdict ?date WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#<target>> sdlc:reviewedBy ?review .
    ?review sdlc:reviewKind ?reviewKind ;
            sdlc:verdict ?verdict ;
            dct:date ?date .
  }
} ORDER BY DESC(?date)
```

If the target already has a `blocked` or `changes-requested` verdict from
the same `reviewKind`, treat this as a re-review superseding it rather than
an independent one — say so explicitly.

## 2. Produce the verdict

Ask (or infer) which `reviewKind` this review is (`ceo | eng | design |
devex | security`), then apply that lens's own judgment to the target's
content — this skill does not prescribe a fixed checklist; the reviewKind
determines what's being judged (business priority, architecture soundness,
UX consistency, developer experience, or security posture).

Produce one of three verdicts: `approved`, `changes-requested`, `blocked`,
plus a short rationale.

## 3. Persist

Construct the INSERT (✏️ PUSH banner, verbatim):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#review-<slug>> a sdlc:Review ;
      sdlc:reviewKind "<kind>" ;
      sdlc:verdict "<verdict>" ;
      dct:description "<rationale>" ;
      dct:date "<now>"^^xsd:dateTime .
    <urn:project:sparql-mcp-dev#<target>> sdlc:reviewedBy
      <urn:project:sparql-mcp-dev#review-<slug>> .
  }
}
```

Push via `mcp__sparql-mcp__update_sparql`, then **verify** (✅ OUTCOME
banner) with a SELECT confirming both the `sdlc:Review` and the
`sdlc:reviewedBy` link landed.

## 4. Audit reminder

If the target is a `sdlc:Plan`, remind the user that audit rule P2 (major)
requires at least one `verdict "approved"` review before the Plan should
move to implementation — re-running the audit after this write is cheap
and catches the case where this review didn't clear it.
