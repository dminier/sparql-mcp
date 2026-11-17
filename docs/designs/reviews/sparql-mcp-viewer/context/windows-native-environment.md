# Ground truth update: native Windows, no WSL (2026-11-17)

Confirmed by the author for Round 2 of this review:

*   Both `sparql-mcp serve` (the MCP server) and `sparql-mcp-viewer` (the Bevy
    GUI client) now run **natively on Windows** (MSVC toolchain). There is no
    WSL anywhere in the development loop or the target deployment.
*   The WSL↔Windows TCP boundary described in `interface-agentic-rust-stylee.md`
    (lines ~129-174, ~297, ~636) and the WSL2 localhost-forwarding concern in
    `sparql-mcp-viewer.md` (Phase 0 spike, R20, R36, CUJ 4, the CI/build
    commands referencing WSL) are **obsolete**. The loopback connection
    (`127.0.0.1`) is now a same-OS connection between two native Windows
    processes; there is no cross-OS forwarding step and no `networkingMode`
    fallback to decide.
*   Any instruction, fallback, rationale, or test step in
    `sparql-mcp-viewer.md` that depends on WSL (building Linux Bevy packages
    "in WSL", running the e2e check "from WSL", the Phase 0 WSL2 forwarding
    spike, "smoke-tested from WSL") is now a documentation defect: it
    describes an environment that no longer exists for this project. Treat
    these as findings to fix, not as still-valid constraints.
*   `interface-agentic-rust-stylee.md` is the companion design and is not the
    target of this round, but it shares the same stale WSL assumption. Do not
    edit it; just note in findings where `sparql-mcp-viewer.md`'s text relies
    on a companion-doc claim that is now also stale, so the author can run a
    follow-up round on that document separately.

## Roadmap clarification (added after Round 2 was launched)

The author's intent, precisely: ship a **single common binary per OS**. Right
now that means one native Windows binary (server + viewer both run as native
Windows processes, no WSL). Linux is **not** ruled out long-term — the same
"single common binary, same-OS, no cross-OS boundary" architecture is meant
to extend to a native Linux binary later. This is a roadmap note, not a
requirement for Round 2's findings: it does not change anything about the
Windows-only, no-WSL ground truth above, which remains exactly what Round 2
should check against. It does mean the document's current Non-Goal
("Platforms other than Windows. Not targeted in v1") should eventually be
softened to "not in v1, but the design should not preclude it" — flag this
as at most a Nit/Major if a reviewer independently notices the Non-Goal
wording looks more absolute than the roadmap intends; do not go hunting for
Linux-specific gaps beyond that.
