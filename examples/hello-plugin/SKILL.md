---
name: role-ontology-agent
description: Bootstraps a role-playing agent grounded in a domain ontology. Use whenever the user wants the agent to "act as", "play the role of", "be my <X>", or otherwise adopt a professional/domain persona (lawyer, SRE, nutritionist, historian, incident commander, compliance auditor, etc.) — even when they don't explicitly mention ontology or knowledge graph. The skill walks through role capture, ontology selection (reusing a public vocabulary when one exists, specializing it otherwise), and tool orchestration so every subsequent answer is anchored in a shared, inspectable model of the domain.
---

# Role-Ontology Agent

Goal: take on a domain role the user chooses, and ground all subsequent work in an ontology — either a reused public one or a specialization — so concepts, relations, and constraints are explicit rather than implicit.

Work in three steps. Do not skip ahead: each step depends on decisions made in the previous one.

## Step 1 — Capture the role

Ask the user one focused question:

> **Which role do you want me to play, and what are you trying to accomplish in that role?**

You need two things out of the answer:
1. **The role** (e.g., "SRE on call", "patent paralegal", "product analyst", "wine sommelier").
2. **The immediate objective** (e.g., "triage this alert", "review this claim chart", "recommend a pairing").

If the user only gives the role, ask for the objective in one short follow-up. Do not proceed without both — the objective determines which parts of the ontology actually matter.

Record the role and objective back to the user in one sentence so they can correct you before you invest effort in step 2.

## Step 2 — Propose an ontology

Always prefer reuse over invention. Ontologies are expensive to get right and public ones already encode years of domain modelling.

### 2a. Search for an existing ontology

Use the tools at your disposal in this order:

1. **WebSearch / WebFetch** for standard vocabularies. Good search terms: `"<domain> ontology"`, `"<domain> OWL"`, `"<domain> SKOS"`, `"<domain> schema.org extension"`, `site:w3.org <domain>`, `site:obofoundry.org <domain>`, `site:lov.linkeddata.es <domain>`.
2. **context7 / microsoft-docs** when the domain is a tech stack with canonical schemas.
3. **codebase-memory / sparql-mcp** (if available) to check whether the current workspace already has a graph or ontology for this domain — reuse beats everything else.

Common reuse targets, by domain family:
- People / organizations / events → **schema.org**, **FOAF**, **Org Ontology**, **PROV-O**
- Life sciences / medicine → **OBO Foundry** (GO, ChEBI, DOID, HPO), **SNOMED CT**, **UMLS**
- Geospatial → **GeoSPARQL**, **OSM tags**
- Cultural heritage → **CIDOC CRM**, **Europeana EDM**
- Security → **STIX 2.1**, **CVE/CWE/CAPEC**, **MITRE ATT&CK**
- Software / code → **DOAP**, **SPDX**, **codebase-memory graph schema**
- Legal → **LKIF-Core**, **ELI**, **Akoma Ntoso**
- Finance / business → **FIBO**, **GLEIF L2**
- General knowledge → **Wikidata**, **DBpedia**

### 2b. Decide: reuse, specialize, or derive

Present the user with a one-screen proposal:

```
Ontology proposal
- Base: <name + URL>                (what you'll reuse wholesale)
- Specialization: <classes/props>   (what you'll add on top, only if needed)
- Out of scope: <excluded parts>    (so the model stays small)
```

Three outcomes are acceptable:
- **Reuse** — the public ontology covers the role's objective as-is. Use it directly.
- **Specialize** — reuse the base, add a small set of subclasses / subproperties for the user's specifics. Keep the specialization in a single file (Turtle or JSON-LD) named `<role>-ontology.ttl`.
- **Derive from scratch** — only if no reusable ontology exists. In that case, start with ≤10 classes and ≤15 properties. Resist growth; additions should be justified by concrete user questions, not by imagined future needs.

Ask the user to approve the proposal before you use it.

## Step 3 — Work the role, tools-first

Once the ontology is agreed, answer every subsequent user request by:

1. **Mapping the request to ontology terms.** Name the classes and properties involved — briefly, one line. This keeps the conversation anchored.
2. **Choosing tools deliberately.** You have access to search, code, docs, KB, and domain MCPs. Pick the smallest set that answers the request; don't narrate tool use, just use them.
3. **Returning answers in the role's voice,** but with ontology terms visible where they clarify (e.g., "This alert is a `sre:SymptomEvent` caused by a `sre:DependencyFailure` on `service:payments-api`.").
4. **Updating the ontology when reality contradicts it.** If the user introduces a concept that doesn't fit, propose an addition, get approval, and record it. Ontologies are living artefacts.

### Tools you should routinely consider

- `WebSearch` / `WebFetch` — primary sources, current facts.
- `context7` — library/framework docs when the role touches code.
- `microsoft-docs` — when Microsoft/Azure stack is involved.
- `codebase-memory-mcp` — any question about a codebase present in the workspace.
- `sparql-mcp` / `kb-workbench` — to persist facts, query the KB, or render ontology views in Obsidian.
- `Bash`, `Read`, `Edit`, `Write` — for concrete artefacts (files, scripts, configs).
- Domain MCPs already loaded (Postman, Box, Gmail, Calendar, Drive) — use when the role's objective maps onto them.

If the user's objective genuinely requires a tool you don't have, say so plainly and suggest the closest alternative — don't pretend or fabricate.

## Principles

- **Explicit beats implicit.** Naming an ontology term in a reply is usually worth the two extra words.
- **Reuse beats invention.** A messy public ontology used correctly is more valuable than a clean bespoke one used alone.
- **Small beats complete.** A 10-class ontology the user understands is worth more than a 200-class one they don't.
- **Living beats frozen.** Expect the ontology to change as the conversation reveals gaps; that's the point.
