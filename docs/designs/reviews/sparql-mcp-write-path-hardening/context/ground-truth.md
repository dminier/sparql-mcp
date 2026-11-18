# Ground truth for the review of `sparql-mcp-write-path-hardening.md` (2026-11-18)

*   Core and viewer run natively on Windows (no WSL). One binary per OS; Linux later.
*   The core is a stdio MCP server spawned per Claude Code session and owns the RocksDB lock.
*   This design was split out of `sparql-mcp-batch-ingestion.md` by the author after EGM
    round 1 of that design (finding M4). Its findings F1-F9 were verified there by
    egm-codebase-details (see `../sparql-mcp-batch-ingestion/consolidated_findings_report.md`);
    F10 and the extra writers (cbm_load_graph, recording, project_create, importer, plugins,
    CLI) come from that same review.
*   Oxigraph 0.5.7 facts verified on 2026-11-18: `len()` is a full scan; `insert` is one
    transaction per call; one SPARQL update request is one transaction; `Transaction::contains`
    sees uncommitted writes; RocksDB transactions have no conflict detection; BulkLoader is
    atomic by default; no `oxhttp` in Cargo.lock; spargebra rewrites ADD/MOVE/COPY.
*   `AGENTS.md` invariants apply (stdout reserved for JSON-RPC, a StoreEvent on every write to
    the active graph, Turtle path escaping, feature-gated `recording` tools compile under
    --all-features). Tests normally use the in-memory store; this design deliberately requires
    temp RocksDB stores for gate tests.
*   Goal of this review: decide whether the design is ready for implementation (Phase 0a first).
