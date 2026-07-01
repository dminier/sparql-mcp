---
name: qa-run
description: |
  Writes a new sdlc:QARun (functional QA) and its sdlc:QAFinding entries
  for a Plan under qualification. Backs sdlc:QALeadPersona. Extension of
  the Quality cycle in sdlc-workbench (the pilot /review skill writes
  reviews, not QA runs — this skill is the missing piece for actual
  test/QA execution).

  Triggers (EN): "run QA on", "record a QA run", "log a QA finding",
  "qualify this plan".
  Triggers (FR): "lance la QA sur", "enregistre un passage QA", "note un
  défaut QA", "qualifie ce plan".
---

# /qa-run — functional QA execution

Part of `sdlc-workbench`. Defers generic KB plumbing to `kb-workbench`;
this file only states what's specific to writing `sdlc:QARun` +
`sdlc:QAFinding`.

## 1. Contexte

Check for prior QA runs on the same target and any still-open findings (🔎
QUERY banner, verbatim):

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
SELECT ?run ?qaStatus ?finding ?severity ?findingStatus WHERE {
  GRAPH <urn:project:sparql-mcp-dev> {
    ?run a sdlc:QARun ; sdlc:qaStatus ?qaStatus .
    OPTIONAL {
      ?finding sdlc:foundIn ?run ;
               sdlc:severity ?severity ;
               sdlc:findingStatus ?findingStatus .
    }
  }
}
```

## 2. Execute

Run the actual verification for the target — in this repo, typically
`cargo test -p sparql-mcp-core`, `cargo clippy -D warnings`, `cargo fmt
--check`, plus any manual checks the Plan calls for. Do not fabricate a
`passed` status without having actually run something; a `sdlc:QARun` with
no real verification behind it defeats audit rule P1's purpose entirely.

## 3. Persist the QARun

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#qarun-<slug>> a sdlc:QARun ;
      sdlc:qaStatus "<passed|failed|running>" ;
      dct:description "<what was run: cargo test filter, clippy, manual steps>" ;
      dct:date "<now>"^^xsd:dateTime .
  }
}
```

## 4. Persist findings, if any

One `sdlc:QAFinding` per distinct defect, each `sdlc:foundIn` the QARun
above:

```sparql
PREFIX sdlc: <https://sparql-mcp.dev/ns/sdlc#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <urn:project:sparql-mcp-dev> {
    <urn:project:sparql-mcp-dev#finding-<slug>> a sdlc:QAFinding ;
      sdlc:severity "<blocker|major|minor|info>" ;
      sdlc:findingStatus "open" ;
      dct:description "<what's broken>" ;
      dct:date "<now>"^^xsd:dateTime ;
      sdlc:foundIn <urn:project:sparql-mcp-dev#qarun-<slug>> .
  }
}
```

Push both via `mcp__sparql-mcp__update_sparql`, then **verify** (✅ OUTCOME
banner) with a SELECT confirming the QARun and every QAFinding landed with
the right `sdlc:foundIn` link.

## 5. Audit reminder

Re-run the audit after this write: rule P6 (blocker) flags any
`blocker`-severity finding left `open` for more than 3 days, and rule P1
(blocker) will keep any `sdlc:ShipEvent` from qualifying until a
`sdlc:QARun` here reaches `qaStatus "passed"` with no unresolved blocker
findings.
