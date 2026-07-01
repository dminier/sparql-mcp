---
name: review-eng
description: |
  Specializes /review for sdlc:EngReviewerPersona — judges a Spec/Plan for
  architecture and implementation soundness (reviewKind "eng"). Extension
  of the Plan cycle in sdlc-workbench.

  Triggers (EN): "engineering review this plan", "architecture review",
  "is this plan sound".
  Triggers (FR): "revue d'architecture de ce plan", "ce plan tient-il la
  route techniquement".
---

# /review-eng — architecture/implementation review

Backs `sdlc:EngReviewerPersona`. Reuses `skills/review`'s contexte/persist
mechanics wholesale — this file only states the judgment lens.

## Judgment lens (reviewKind = "eng")

1. **Consistency with this repo's layering** — does the Plan respect the
   domain/infrastructure/mcp/plugin hexagonal split (`CLAUDE.md`), or does
   it leak infrastructure concerns into domain logic?
2. **Task granularity** — is each task in the Plan small enough to be one
   failing-test → implement → green → commit loop, per the TDD-by-default
   methodology?
3. **Store-concurrency awareness** — if the Plan involves parallel
   sub-agents or scripts touching the Oxigraph/RocksDB store, does it
   respect the single-writer-per-directory constraint (§7 of the parent
   `SKILL.md`)?
4. **Test coverage plan** — does the Plan say what `cargo test` filters
   will exercise the new code, not just "add tests" as a bullet?

A `blocked` verdict here should name the specific task in the Plan that
violates one of these, not the Plan as a whole.

## Everything else

Follow `skills/review` §1 (contexte SELECT), §3 (persist INSERT with
`sdlc:reviewKind "eng"`), and §4 (audit reminder) verbatim.
