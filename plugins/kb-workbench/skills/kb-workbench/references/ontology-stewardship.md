# Ontology stewardship

The ontology isn't frozen. It grows with the KB. This skill treats
unknown terms as a signal — *new knowledge that hasn't been typed yet* —
and helps the human curate them.

## The loop

```
ingest → graph has unknowns → ontology_propose.py → human decides → ontology grows
```

1. An adapter lands triples in staging. Some triples use predicates or
   classes that aren't declared in any `ontology/*.ttl`.
2. `scripts/ontology_propose.py --project <slug>` scans **every** named
   graph referencing the project (project graph + current staging
   graphs) and compares their vocabulary to the loaded ontology.
3. The script groups findings into four buckets:
   - **unknown predicate** — used in triples but not declared as
     `rdf:Property`. Reported with sample `(subject_class, object_type)`
     signatures to suggest a domain / range.
   - **unknown class** — used as `a <Class>` but not declared as
     `rdfs:Class`. Reported with sample instances.
   - **redundant pair** — two predicates with identical domain + range
     and semantically close labels. Candidates for merge.
   - **orphan ontology term** — declared in `ontology/*.ttl` but never
     used in any project. Candidates for removal.
4. Output: `output/ontology/candidates-<ts>.ttl` + a plain-text report.

## The candidate file

The emitted `candidates-<ts>.ttl` is syntactically valid Turtle that
*would* declare the missing terms if loaded. Example:

```turtle
# Proposed by ontology_propose.py on 2026-04-23 at 20:00 UTC
# Used in <urn:project:example-project> (4 triples) but not declared.

kb:rateLimitWindow
    a rdf:Property ;
    rdfs:label "Rate limit window" ;
    rdfs:comment "Proposed — observed as kb:Endpoint → xsd:duration. Confirm domain/range." ;
    rdfs:domain kb:Endpoint ;
    rdfs:range xsd:duration .
```

The human reviews, edits the label/comment/domain/range, and either:

- **accepts** → moves the declaration into the appropriate layer
  (`ontology/2-<domain>.ttl`, `ontology/3-archimate.ttl`, …) and reloads
  via `mcp__sparql-mcp__load_ontology_file`.
- **aliases** → writes an `owl:equivalentProperty` or
  `owl:equivalentClass` to map the new term to an existing one; the next
  promotion step rewrites staging triples through the alias.
- **defers** → leaves the candidate in the file for later, keeping the
  staging graph around.

## Detecting "a more elegant structure"

Beyond unknown terms, the script flags structural smells:

- **Predicate clustering** — if two predicates share the same
  `(domain, range)` and appear on overlapping subjects, suggest they be
  merged or disambiguated.
- **Class subsumption gaps** — if instances of `A` and `B` have
  near-identical predicate sets, suggest a common super-class.
- **Deep unary chains** — `X → predicate → blank → predicate → value`
  patterns that could be a single reified object.

These are hints, never automated changes. The human decides.

## Why human-in-the-loop matters

An ontology is both a vocabulary and a contract. Auto-accepting
candidates would turn every typo in an ingested doc into a new class.
The cost of reviewing a proposal is small; the cost of an un-curated
ontology is a graph that nobody can query reliably.
