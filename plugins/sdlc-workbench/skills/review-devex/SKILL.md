---
name: review-devex
description: |
  Specializes /review for sdlc:DevExReviewerPersona — judges a Spec/Plan
  for developer-experience impact: tooling, onboarding, docs (reviewKind
  "devex"). Extension of the Plan cycle in sdlc-workbench.

  Triggers (EN): "devex review this plan", "onboarding impact review",
  "tooling review".
  Triggers (FR): "revue devex de ce plan", "impact sur l'onboarding",
  "revue outillage".
---

# /review-devex — developer-experience review

Backs `sdlc:DevExReviewerPersona`. Reuses `skills/review`'s
contexte/persist mechanics wholesale — this file only states the judgment
lens.

## Judgment lens (reviewKind = "devex")

1. **CLAUDE.md drift** — does the Plan introduce a new convention, tool,
   or doctrine rule that isn't yet reflected in `CLAUDE.md`, leaving future
   agents to discover it by surprise?
2. **Setup cost** — does the Plan add a new dependency/script/service a
   contributor must install before the repo works, and if so, is that cost
   documented where a first-time contributor will see it?
3. **Command surface growth** — does a new skill/command overlap
   confusingly with an existing one (ambiguous trigger phrases), or is its
   scope distinct enough to justify its own entry?
4. **Doctrine cost vs. duplication** — does the Plan defer to existing
   generic plumbing (`kb-workbench`) where possible, or does it
   re-implement something that already exists (a recurring anti-pattern
   this repo explicitly wants avoided)?

## Everything else

Follow `skills/review` §1 (contexte SELECT), §3 (persist INSERT with
`sdlc:reviewKind "devex"`), and §4 (audit reminder) verbatim.
