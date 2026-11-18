//! Domain layer — pure interfaces and value types.
//!
//! No infrastructure imports here.  Everything is a trait, an enum, or a
//! plain data struct.  Adapters in `crate::infrastructure` implement the traits.

pub mod doc_store;
pub mod loaded_files;
pub mod sparql_store;

pub use doc_store::DocStore;
pub use sparql_store::{
    BatchApplied, BatchRefusal, BatchWrite, BoundedOutcome, Counter, FileLoadResult, Idempotency,
    InputFormat, LoadOpts, QueryResult, RdfTerm, RecordCounts, Replace, SolutionSet, SparqlStore,
    StoredQuad, UpdateOutcome,
};
