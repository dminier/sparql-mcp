# Ontology design — the 3-layer pattern

When a domain skill (enterprise-architecture, research-notebook, incident-response, …)
declares its vocabulary, resist the urge to dump everything at the same
level. Three layers keep the ontology **usable long-term**:

```
L1 — Core EA / foundational   (ArchiMate + SMC primitives — frozen)
L2 — Domain                   (e.g. ArchiMate: BusinessCapability, ApplicationService)
L3 — Specialized layers       (e.g. Agent Stack: AgentPlatform, MCPGateway, LLMFamily…)
```

- **L1 is stable.** `archimate:Capability`, `archimate:ApplicationComponent`,
  `archimate:ApplicationInterface`, `archimate:SystemSoftware`,
  `archimate:Principle`, `smc:ADR`, `smc:WatchEvent`, `smc:Vendor`,
  `smc:Pattern`. Don't reinvent these — reuse.
- **L2 extends L1** with sub-classes that carry domain meaning. The
  heuristic: *"would this class also exist in another company of the same
  industry?"* If yes, L2. If it only exists because of this one project,
  push it to L3.
- **L3 is where most new classes land.** It's the volatile layer. Specialize
  L1 via `rdfs:subClassOf` so generic queries still work.

## The 5 cross-cutting facets

Any brick at L2/L3 that represents a *product* (OSS, vendor, custom) **must**
carry these 5 facets as properties — they are what make inventory, audit,
comparison, roadmap possible. Without them, the graph degenerates into a
list of names.

| Facet | Type | Values |
|---|---|---|
| `sourcing` | enum | `oss` · `vendor` · `hybrid` |
| `sovereigntyTier` | enum | `secnumcloud` · `bleu` · `s3ns` · `eu-hyperscaler` · `us-hyperscaler` · `on-prem` · `saas-global` |
| `maturity` | enum | `poc` · `alpha` · `beta` · `rc` · `ga` · `deprecated` |
| `adoptionState` | enum | `candidate` · `evaluated` · `selected` · `adopted` · `deprecated` |
| `mcpNative` *(or equivalent protocol flag)* | boolean / enum | `true` · `false` · `partial` |

Plus the three obligatory link properties: `vendor`, `realizesCapability`,
`mitigatesRisk`. A brick without these three cannot be placed on a canvas
or scored in an ADR.

## Example — the 14-class agent-stack layer (L3)

```
AgentPlatform       ⊑ ApplicationComponent   # watsonx Orchestrate, AgentCore…
AgentFramework      ⊑ SystemSoftware         # LangGraph, CrewAI, OpenAI Agents SDK
AgentDevKit         ⊑ ApplicationComponent   # watsonx ADK, Google ADK CLI
MCPGateway          ⊑ ApplicationComponent   # ContextForge, Kong AI MCP
MCPServer           ⊑ ApplicationComponent   # IBM CCS, custom metier
MCPTool             ⊑ ApplicationInterface   # ged.search_documents…
LLMGateway          ⊑ SystemSoftware         # LiteLLM, watsonx AI Gateway
LLMFamily           ⊑ ApplicationComponent   # Granite, Claude, GPT, Gemini
RAGPlatform         ⊑ ApplicationComponent   # watsonx Discovery, Glean
VectorStore         ⊑ SystemSoftware         # Qdrant, Milvus, Pinecone
EmbeddingModel      ⊑ ApplicationComponent   # Slate, BGE-m3
MemoryStore         ⊑ SystemSoftware         # mem0, Zep, Memory Bank
GuardrailsModel     ⊑ ApplicationComponent   # Granite Guardian, Llama Guard
GuardrailsFramework ⊑ SystemSoftware         # NeMo Guardrails, Guardrails AI
EvalFramework       ⊑ SystemSoftware         # Promptfoo, Braintrust
ObservabilityPlatform ⊑ SystemSoftware       # Langfuse, Datadog LLM Obs
GovernancePlatform  ⊑ ApplicationComponent   # watsonx.governance, Credo AI
PolicyEngine        ⊑ SystemSoftware         # OPA, Cedar, Styra
IdPService          ⊑ ApplicationComponent   # Entra ID, Keycloak
```

Why this matters: a canvas "agent-stack layered" can then be **generated
automatically** by one SPARQL query that groups by `rdf:type` within the
`ags:*` namespace. The moment a new product is ingested with the right
sub-class, it appears on the canvas.

## Security ontology (L3 parallel layer)

```
SecurityRisk        — OWASP LLM01…LLM10, CWE, internal incidents
SecurityControl     — a concrete mitigation (e.g. "OPA deny-by-default")
SecurityFramework   — OWASP LLM, NIST AI RMF, ISO 42001, EU AI Act, DORA
SecurityPrimitive   — ACL, MarkingSet, JWT, OAuth, mTLS, TokenExchange
SecurityPrincipal   — user, service account, agent, population
```

Core relations:
- `SecurityControl mitigates SecurityRisk`
- `SecurityControl implements SecurityFramework`
- `SecurityControl buildsOn SecurityPrimitive`
- `Brick mitigatesRisk SecurityRisk`   *(inverse of `controlledBy`)*

That's enough to answer *"which bricks cover LLM06?"* or *"which risks
have zero controls yet?"* with one SELECT.

## Canonical audit rules (generic, any domain)

| ID | Rule | Severity |
|---|---|---|
| A1 | `ApplicationComponent` without `vendor` | blocker |
| A2 | L3 brick without `sourcing` | major |
| A3 | L3 brick without `sovereigntyTier` | major |
| A4 | `SecurityRisk` with zero `mitigates` incoming | blocker |
| A5 | `ADR` without `dct:date` | minor |
| A6 | `WatchEvent` without `dct:date` | minor |
| A7 | `MCPTool` (or interface) without `exposedBy` | major |
| A8 | Brick `adoptionState=adopted` without linked `ADR` | major |
| A9 | Sovereignty tier incoherent with vendor (e.g. `secnumcloud` + US-only vendor) | blocker |
| A10 | Duplicate labels across bricks of same sub-class | major |

Each domain skill adds its own rules on top in `rules/audit.yaml`, but
these 10 are candidates for the shared default.

## Promotion flow — candidate → promoted

1. Ingest emits triples using fresh predicates/classes as needed.
2. `ontology_propose.py` runs after each ingest, surfaces new terms,
   writes `output/ontology/candidates-<ts>.ttl`.
3. Human reviews in the next session. Three possible outcomes:
   - **accept** → copy the term declaration into `ontology/<layer>.ttl`,
     add `rdfs:label` + `rdfs:comment` + `rdfs:subClassOf` / `rdfs:domain`
     / `rdfs:range`, re-ingest ontology file.
   - **alias** → add an `owl:equivalentClass` / `owl:equivalentProperty`
     mapping to an existing term in the ontology, update the candidate
     file for history.
   - **defer** → leave in candidates, revisit in 1 quarter.
4. Only when an ontology term is **accepted** it becomes tag-able in
   Obsidian (see `obsidian-convention.md`).

## Anti-patterns observed in real projects

- **Flat vocabulary** — 300 predicates at one level. Symptom: nobody
  remembers which one exists, duplicates appear. Fix: enforce the 3-layer
  split.
- **Untyped links** — generic `relates` everywhere. Symptom: canvas can't
  distinguish "uses", "realizes", "mitigates", "federates". Fix: typed
  object properties at L1, reuse them.
- **Prose in prose** — a single `description` blob holds what should be
  5 facets. Symptom: can't filter/group. Fix: extract facets, keep
  description for the 1-paragraph summary.
- **No sourcing / sovereignty facets** — comparison becomes subjective.
  Symptom: ADRs full of hand-waving. Fix: make the 5 facets mandatory
  and audit A2/A3 as blocker.
- **ADR without date** — can't tell which is current. Fix: audit A5 must
  block merges.
