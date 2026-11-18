# Ground truth for the review of `sparql-mcp-batch-ingestion.md` (2026-11-18)

## Environment
*   Core and viewer run natively on Windows (no WSL). One binary per OS; Linux later.
*   The core is a stdio MCP server spawned per Claude Code session; it owns the RocksDB
    store lock (one process at a time).

## Author's request (intent to check against)
*   The agent will produce batch processing over MCP and needs professional batching
    ("lotissement"): inserts must be secure and efficient, limiting MCP-side risks
    (max insert sizes, etc.).

## Verified code facts (2026-11-18)
*   Oxigraph 0.5.7: `Store::len()` doc says "This function executes a full scan".
    `Store::insert` opens one transaction per call. `Store::extend` is a memory-heavy
    transaction. `BulkLoader` is non-transactional.
*   No `oxhttp` in `Cargo.lock`: Oxigraph's HTTP client (needed by `LOAD <http…>`) is not compiled.
*   Write tools: `crates/sparql-mcp-core/src/application/tools/{sparql,ontology}.rs`;
    adapter: `crates/sparql-mcp-core/src/infrastructure/oxigraph.rs`; port:
    `crates/sparql-mcp-core/src/domain/sparql_store.rs`; events: `application/event_sink.rs`.
*   Project graphs are `<urn:project:ID>`, registry in `<urn:meta>`; load ledger graph
    `urn:hkb:meta:loaded-files`.
*   Unverified by the author (reviewers may flag): whether one Oxigraph SPARQL update
    request (`ADD …; DROP …`) executes in a single transaction; whether `contains` is
    available inside an Oxigraph 0.5 transaction.
