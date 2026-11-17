//! `sparql-mcp-viewer` library root — `main.rs` and the integration tests
//! use this crate directly.
//!
//! Modules are declared as their phase adds them (see the Delivery phases
//! table in `docs/designs/sparql-mcp-viewer.md`). `layout`, `render`,
//! `camera`, `interact` and `ingest` do not exist yet.

pub mod connection;
pub mod graph;
pub mod headless;
pub mod plugin;
