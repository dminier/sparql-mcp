//! Application layer — tool handlers that orchestrate domain ports.
//!
//! No infrastructure imports here.  Each function receives `Arc<dyn SparqlStore>`
//! or `Arc<dyn DocStore>` and returns `CallToolResult`.

pub mod tools;
