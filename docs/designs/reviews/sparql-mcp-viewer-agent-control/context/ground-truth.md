# Ground truth for the review of `sparql-mcp-viewer-agent-control.md` (2026-11-18)

## Environment
*   `sparql-mcp serve` (MCP server, stdio, spawned per Claude Code session) and
    `sparql-mcp-viewer` (Bevy 0.20.1 GUI) both run **natively on Windows**. No WSL.
    Roadmap: one common binary per OS; native Linux later must not be precluded.
*   See also `../../sparql-mcp-viewer/context/windows-native-environment.md`.

## Decisions already made by the author (do not re-litigate; check the doc reflects them)
1.  Architecture: the core exposes `viewer_*` MCP tools and relays them over the
    existing side-channel made bidirectional (Alternative B). The viewer is NOT an MCP server.
2.  The user can always take back control from the agent.
3.  Search covers both the drawn view and the store. Use case: the user asks the
    agent to search, and asks for the result to be displayed.
4.  Screenshot is in v1 (so the agent can see what it drives).
5.  No viewer connected: tools do nothing (no-op success, never an error).

## Verified code facts
*   Side-channel: `crates/sparql-mcp-core/src/infrastructure/viewer_channel.rs`; viewer client:
    `crates/sparql-mcp-viewer/src/connection.rs`. Today one-way after `{"cmd":"snapshot"}`.
*   Tool dispatch: `crates/sparql-mcp-core/src/mcp/server.rs` (`dispatch_core` is synchronous).
*   Bevy 0.20.1 `PanOrbitCamera` is Transform/anchor driven; `LookTo` extension only rotates
    about the anchor; `Screenshot::primary_window()` + `ScreenshotCaptured` observer exists.
