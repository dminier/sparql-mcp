//! `sparql-mcp-viewer` library root — `main.rs` and the integration tests
//! use this crate directly.
//!
//! Modules are declared as their phase adds them (see the Delivery phases
//! table in `docs/designs/sparql-mcp-viewer.md`). `interact` and `ingest`
//! (Phase 5) do not exist yet.

pub mod camera;
mod camera_input;
pub mod connection;
pub mod graph;
pub mod headless;
pub mod layout;
pub mod plugin;
pub mod render;
