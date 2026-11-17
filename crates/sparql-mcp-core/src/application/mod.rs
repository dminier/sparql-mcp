//! Application layer — tool handlers that orchestrate domain ports.
//!
//! No infrastructure imports here.  Each function receives `Arc<dyn SparqlStore>`
//! or `Arc<dyn DocStore>` and returns `CallToolResult`.

pub mod event_sink;
pub mod tools;

pub use event_sink::{
    BroadcastEventSink, EventSink, NullEventSink, StoreEvent, SubjectKind, CLASS_TYPE_IRIS,
};
