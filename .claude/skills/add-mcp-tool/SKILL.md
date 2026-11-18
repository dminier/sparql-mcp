---
name: add-mcp-tool
description: Add or change a core MCP tool in sparql-mcp-core (new tool, new argument, renamed tool, feature-gated tool). Use whenever the task touches application/tools/, core_tool_defs, dispatch_core, or the user says "expose X to the agent", "new MCP tool", "add a tool", "ajoute un outil MCP", "expose via MCP" — even if they only mention the business logic, because a tool has several registration points that are easy to miss.
---

# Add an MCP tool

A core tool lives in four places. Missing one compiles fine and fails at
runtime (tool not listed, "unknown tool", or the viewer silently out of sync),
so walk the whole list.

## 1. Implement — `crates/sparql-mcp-core/src/application/tools/<group>.rs`

Put it in the existing group module if it fits (`sparql`, `ontology`,
`project`, `export`, `doc`, `cbm`, `gdrive`), otherwise add a module and
declare it in `tools/mod.rs`.

Write the pair, reusing the helpers from `tools/sparql.rs`:

```rust
use super::sparql::{make_tool, require_str};

pub fn tool_<name>_def() -> Tool {
    make_tool("<name>", "<what it does and when an agent should call it>", json!({
        "type": "object",
        "properties": { "id": { "type": "string", "description": "…" } },
        "required": ["id"]
    }))
}

pub fn <name>(store: &Arc<dyn SparqlStore>, args: &JsonObject) -> Result<CallToolResult, McpError> { … }
```

The description is what the calling agent reads to decide whether to use the
tool: say what it returns and any side effect. Return JSON text content (look
at a neighbouring tool for the shape) and use `McpError::invalid_params` for
bad input, so agents get a recoverable error instead of a crash.

Keep the logic here and talk to storage only through the `SparqlStore` /
`DocStore` traits — `mcp/server.rs` must stay a dispatcher.

## 2. Emit write events

If the tool writes to the active project graph, take `active_graph` and
`event_sink: &dyn EventSink` and emit a `StoreEvent` after the commit, the way
`sparql::update_sparql` and `ontology::load_ontology` do. Otherwise the live
viewer diverges from the store with no error anywhere.

## 3. Register — `crates/sparql-mcp-core/src/mcp/server.rs`

- add `tool_<name>_def()` to `core_tool_defs()`;
- add the `"<name>" => <group>::<name>(…)` arm to `dispatch_core()`.

For a feature-gated tool, put `#[cfg(feature = "…")]` on the module, the def
push and the match arm — all three.

## 4. Test — `crates/sparql-mcp-core/tests/tools_<group>.rs`

Call the function directly against `OxigraphAdapter::open_in_memory()`
(see `tests/tools_project.rs`), passing `&NullEventSink` where a sink is
needed (see `tests/tools_sparql.rs`). Cover the happy path and one invalid
input. For a write tool, also assert the emitted event by subscribing to a
`BroadcastEventSink` (see `tests/viewer_channel.rs`).

## 5. Finish

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
```

Then run the `sync-docs` skill: the README tool table and `CHANGELOG.md`
must list the new tool.

A tool specific to one domain does not belong in core: implement the
`ToolPlugin` trait in its own crate instead (`examples/hello-plugin/`).
