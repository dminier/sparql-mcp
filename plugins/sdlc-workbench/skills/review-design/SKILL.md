---
name: review-design
description: |
  Specializes /review for sdlc:DesignReviewerPersona — judges a Spec/Plan
  for UX/interface consistency (reviewKind "design"). Extension of the Plan
  cycle in sdlc-workbench.

  Triggers (EN): "design review this spec/plan", "UX review", "interface
  consistency review".
  Triggers (FR): "revue design de cette spec/plan", "revue UX", "cohérence
  d'interface".
---

# /review-design — UX/interface review

Backs `sdlc:DesignReviewerPersona`. Reuses `skills/review`'s
contexte/persist mechanics wholesale — this file only states the judgment
lens.

Applies where the Spec/Plan touches a human-facing surface of this repo:
the TUI project viewer, Obsidian rendering output (notes/canvas from
kb-workbench/sdlc-workbench), or CLI output formatting. For pure
backend/protocol changes with no human-facing surface, say so and skip a
full design review rather than manufacturing one.

## Judgment lens (reviewKind = "design")

1. **Consistency with existing conventions** — does new output follow the
   established patterns (e.g. kb-workbench's folder-note convention,
   marker-based auto-region preservation, the 🔎/✏️/✅ banner protocol) or
   introduce a divergent one without reason?
2. **Information hierarchy** — for a new note/canvas/dashboard, is the most
   important fact (verdict, status, blocker) visible without opening a
   sub-note?
3. **Discoverability** — can a human find this new surface from an
   existing MOC/dashboard, or does it require knowing the exact file path?

## Everything else

Follow `skills/review` §1 (contexte SELECT), §3 (persist INSERT with
`sdlc:reviewKind "design"`), and §4 (audit reminder) verbatim.
