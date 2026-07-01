---
name: qa-security
description: |
  Runs a security-focused review and/or QA pass and records it as
  sdlc:Review (reviewKind "security") and/or sdlc:QAFinding entries. Backs
  sdlc:SecurityOfficerPersona. Extension of the Quality cycle in
  sdlc-workbench.

  Triggers (EN): "security review this", "security QA on", "check for
  vulnerabilities in this plan/change".
  Triggers (FR): "revue sécurité de", "QA sécurité sur", "vérifie les
  vulnérabilités de ce plan/changement".
---

# /qa-security — security review + QA

Backs `sdlc:SecurityOfficerPersona`. Combines the `skills/review` verdict
mechanics (for reviewing a Spec/Plan before implementation) with the
`skills/qa-run` finding mechanics (for flagging concrete issues found once
code exists) — use whichever applies to what's actually being examined.

## Judgment lens (reviewKind = "security", when reviewing a Spec/Plan)

1. **OWASP Top 10 relevance** — does the Plan touch input handling
   (SPARQL query construction, MCP tool arguments, file paths) in a way
   that could introduce injection, path traversal, or privilege issues?
   This repo's own guidance flags SPARQL/command injection and OWASP Top
   10 explicitly.
2. **Data separation** — does the Plan risk committing personal/project
   data (store triples, RocksDB snapshots) into the public repo, violating
   the `CLAUDE.md` § Data separation rule?
3. **Secrets handling** — are credentials, tokens, or GDrive OAuth state
   ever written as literal values rather than `smc:SecretReference` paths
   (per kb-workbench's "what never belongs in the graph")?
4. **Store lock / concurrency** — could the Plan's concurrency model
   (parallel agents, scripts) cause a store-corruption risk under the
   RocksDB single-writer constraint?

## When examining actual code/output (QA pass, not a Plan)

Follow `skills/qa-run` §2-4 verbatim, but every `sdlc:QAFinding` recorded
here should additionally note which of the four lenses above it falls
under, in its `dct:description`.

## Persist

- For a Spec/Plan verdict: follow `skills/review` §3 with
  `sdlc:reviewKind "security"`.
- For concrete findings: follow `skills/qa-run` §3-4.

Either way, **verify** with a follow-up SELECT before reporting done, per
the shared "store first, verify, ask before TTL" doctrine.

## Audit reminder

A `blocker`-severity security finding is exactly what audit rule P6 exists
to catch if left open — do not downgrade a real security issue's severity
just to avoid tripping the rule.
