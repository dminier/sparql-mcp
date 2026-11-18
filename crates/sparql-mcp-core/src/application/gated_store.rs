//! `SparqlStore` decorators that put every store write behind the write gate.
//!
//! *   [`GatedStore`] is what tools, plugins and the CLI receive.
//! *   [`PrivilegedStore`] is held only by core modules that own the registry
//!     and ledger graphs (`project_create`, recording imports, the importer).
//!
//! In Phase 0a both only take the gate. The protected-graph and destructive
//! checks are added to `GatedStore` in Phase 0c. The gate is not reentrant, so
//! code that needs several store calls in one critical section uses
//! [`PrivilegedStore::exclusive`], which holds the gate once and hands out the
//! ungated inner store.
//!
//! Store methods are synchronous and block while waiting for the gate: call
//! them from a blocking thread, as the server does.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::application::write_gate::WriteGate;
use crate::domain::{
    FileLoadResult, LoadOpts, QueryResult, SparqlStore, StoredQuad, UpdateOutcome,
};

/// Write-gated view of a store, for non-privileged callers.
pub struct GatedStore {
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
}

/// Write-gated view of a store, for core modules that own internal graphs.
pub struct PrivilegedStore {
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
}

/// Build both handles from one store and one gate.
pub fn build_stores(
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
) -> (Arc<GatedStore>, Arc<PrivilegedStore>) {
    (
        Arc::new(GatedStore {
            inner: inner.clone(),
            gate: gate.clone(),
        }),
        Arc::new(PrivilegedStore { inner, gate }),
    )
}

impl PrivilegedStore {
    /// Hold the gate once and run `f` with the ungated inner store. Use it
    /// for a sequence of store calls that must not interleave with other
    /// writers (a `CLEAR` followed by loads, a count/write/count). Returns
    /// `write_busy` if the gate cannot be taken in time.
    pub fn exclusive<T>(
        &self,
        label: &'static str,
        f: impl FnOnce(&Arc<dyn SparqlStore>) -> T,
    ) -> Result<T> {
        self.gate.run_blocking(label, || Ok(f(&self.inner)))
    }
}

macro_rules! gated_impl {
    ($ty:ty) => {
        impl SparqlStore for $ty {
            // ── gated writes ─────────────────────────────────────────────
            fn update(&self, sparql: &str) -> Result<()> {
                self.gate
                    .run_blocking("update", || self.inner.update(sparql))
            }

            fn update_with_outcome(&self, sparql: &str) -> Result<UpdateOutcome> {
                self.gate.run_blocking("update_with_outcome", || {
                    self.inner.update_with_outcome(sparql)
                })
            }

            fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64> {
                self.gate
                    .run_blocking("load_rdf", || self.inner.load_rdf(content, opts))
            }

            fn load_rdf_with_triples(
                &self,
                content: &[u8],
                opts: LoadOpts,
            ) -> Result<(u64, Vec<StoredQuad>)> {
                self.gate.run_blocking("load_rdf_with_triples", || {
                    self.inner.load_rdf_with_triples(content, opts)
                })
            }

            fn load_rdf_file(
                &self,
                path: &Path,
                opts: LoadOpts,
                force: bool,
            ) -> Result<FileLoadResult> {
                self.gate.run_blocking("load_rdf_file", || {
                    self.inner.load_rdf_file(path, opts, force)
                })
            }

            fn load_ontology_dir(&self, dir: &Path) -> Result<usize> {
                self.gate
                    .run_blocking("load_ontology_dir", || self.inner.load_ontology_dir(dir))
            }

            // ── pass-through reads ───────────────────────────────────────
            fn query(&self, sparql: &str) -> Result<QueryResult> {
                self.inner.query(sparql)
            }

            fn export_graph(&self, graph_iri: Option<&str>, dest: &Path) -> Result<usize> {
                self.inner.export_graph(graph_iri, dest)
            }

            fn list_graphs(&self) -> Result<Vec<String>> {
                self.inner.list_graphs()
            }

            fn triple_count(&self) -> Result<u64> {
                self.inner.triple_count()
            }
        }
    };
}

gated_impl!(GatedStore);
gated_impl!(PrivilegedStore);
