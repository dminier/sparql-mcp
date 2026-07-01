# Persona catalog — which skill backs which persona

Concrete persona instances live in `ontology/3-sdlc-personas.ttl` (L3),
each `a sdlc:AgentPersona` with the 5 mandatory facets. This file maps each
persona to the skill that implements it (`sdlc:backedBySkill`) and tracks
delivery status honestly.

| Persona | Reviews/records | Backing skill | Status |
|---|---|---|---|
| `sdlc:CEOReviewerPersona` | Product/business-priority review (`sdlc:reviewKind "ceo"`) | `skills/review-ceo` | selected |
| `sdlc:EngReviewerPersona` | Architecture/implementation review (`reviewKind "eng"`) | `skills/review-eng` | selected |
| `sdlc:DesignReviewerPersona` | UX/interface review (`reviewKind "design"`) | `skills/review-design` | selected |
| `sdlc:DevExReviewerPersona` | Developer-experience review (`reviewKind "devex"`) | `skills/review-devex` | selected |
| `sdlc:QALeadPersona` | Runs `sdlc:QARun` + records `sdlc:QAFinding` | `skills/qa-run` | selected |
| `sdlc:SecurityOfficerPersona` | Security review (`reviewKind "security"`) + security QA findings | `skills/qa-security` | selected |
| `sdlc:ReleaseEngineerPersona` | Records `sdlc:ShipEvent` qualified by a passed `sdlc:QARun` | `skills/ship` | selected |

The generic pilot skills (`skills/spec`, `skills/review`, `skills/retro`,
`skills/learn`) remain in place and are not persona-specific — they write
the lifecycle entities (`sdlc:Spec`, `sdlc:Review`, `sdlc:Retro`,
`sdlc:Learning`) regardless of which persona/reviewKind is invoked. Every
persona-specific skill above reuses that pilot's contexte/persist mechanics
and only adds its own judgment lens or entity (`review-*` specialize
`skills/review`; `qa-run`/`qa-security` write `sdlc:QARun`/`sdlc:QAFinding`;
`ship` writes `sdlc:ShipEvent`).

`adoptionState` is `selected` rather than `adopted` for all seven: the
skill files exist and are wired to their persona via `sdlc:backedBySkill`,
but none has accumulated real usage in `urn:project:sparql-mcp-dev` yet.
Promote to `adopted` only after actual dogfooding, not merely because the
file exists — keep this facet honest, per the same rule that drove
`mcpNative` to be asserted rather than assumed.

Audit rule P5 (`.claude/skills/sdlc-workbench/rules/audit.yaml`) checks
that every `sdlc:AgentPersona` has `sdlc:mcpNative` set. It does not check
`sdlc:backedBySkill` or `adoptionState` — a persona may legitimately exist
in the catalog before its skill is written, or exist with a skill but no
usage history yet.
