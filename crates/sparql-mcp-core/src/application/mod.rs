//! Application layer — tool handlers that orchestrate domain ports.
//!
//! No infrastructure imports here.  Each function receives `Arc<dyn SparqlStore>`
//! or `Arc<dyn DocStore>` and returns `CallToolResult`.

pub mod event_sink;
pub mod gated_store;
pub mod guards;
pub mod limits;
pub mod path_guard;
pub mod tools;
pub mod write_gate;

pub use event_sink::{
    emit_quads, BroadcastEventSink, EventLimits, EventSink, NullEventSink, QuadChange, StoreEvent,
    SubjectKind, CLASS_TYPE_IRIS,
};
pub use gated_store::{build_stores, build_stores_with, GatedStore, GuardConfig, PrivilegedStore};
pub use limits::ToolError;
pub use write_gate::WriteGate;
