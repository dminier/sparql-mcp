//! Infrastructure layer — concrete adapters for the domain ports.
//!
//! Each submodule implements a trait from `crate::domain` using a specific
//! third-party library or system resource.

pub mod fs_doc_store;
pub mod oxigraph;
pub mod viewer_channel;

pub use fs_doc_store::FsDocStore;
pub use oxigraph::OxigraphAdapter;
pub use viewer_channel::{serve_viewer_channel, spawn_viewer_channel};
