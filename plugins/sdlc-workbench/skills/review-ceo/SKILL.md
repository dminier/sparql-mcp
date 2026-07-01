---
name: review-ceo
description: |
  Specializes /review for sdlc:CEOReviewerPersona — judges a Spec/Plan from
  a product/business-priority standpoint (reviewKind "ceo"). Extension of
  the Plan cycle in sdlc-workbench.

  Triggers (EN): "CEO review this spec/plan", "is this worth building",
  "product priority review".
  Triggers (FR): "revue produit de cette spec/plan", "est-ce que ça vaut le
  coup", "priorité business".
---

# /review-ceo — product/priority review

Backs `sdlc:CEOReviewerPersona`. Reuses `skills/review`'s contexte/persist
mechanics wholesale — this file only states the judgment lens.

## Judgment lens (reviewKind = "ceo")

Ask, in this order, and let the answer to each gate the next:

1. **Problem worth solving?** Is the problem in the Spec real and
   recurring, or a one-off already handled elsewhere?
2. **Priority vs. what else is in flight** — does this displace
   higher-value work currently queued?
3. **Scope discipline** — does the Spec's non-goals section actually hold
   the line, or is scope creeping into the goals?
4. **Payoff clarity** — can you state, in one sentence, what becomes true
   once this ships that isn't true today?

A `blocked` verdict here should name which gate failed. A
`changes-requested` verdict should point at the exact section of the Spec
to tighten.

## Everything else

Follow `skills/review` §1 (contexte SELECT), §3 (persist INSERT with
`sdlc:reviewKind "ceo"`), and §4 (audit reminder) verbatim.
