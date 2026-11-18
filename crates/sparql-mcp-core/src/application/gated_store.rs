//! `SparqlStore` decorators that put every store write behind the write gate
//! and, for non-privileged callers, behind the write guards.
//!
//! *   [`GatedStore`] is what tools, plugins and the CLI receive. Its writes
//!     take the gate, and are refused when they are too large, touch a
//!     protected graph, or are destructive without `allow_destructive_updates`
//!     (see [`crate::application::guards`]).
//! *   [`PrivilegedStore`] is held only by core modules that own the registry
//!     and ledger graphs (`project_create`, recording imports, the importer).
//!     It takes the gate but applies no guard.
//!
//! The gate is not reentrant, so code that needs several store calls in one
//! critical section uses [`PrivilegedStore::exclusive`], which holds the gate
//! once and hands out the ungated inner store.
//!
//! Store methods are synchronous and block while waiting for the gate: call
//! them from a blocking thread, as the server does. Refusals are raised as
//! [`ToolError`]s before the gate is taken.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::application::guards::{check_update, classify_sparql, is_protected};
use crate::application::limits::ToolError;
use crate::application::path_guard;
use crate::application::write_gate::WriteGate;
use crate::config::IngestConfig;
use crate::domain::{
    FileLoadResult, LoadOpts, QueryResult, SparqlStore, StoredQuad, UpdateOutcome,
};

/// What [`GatedStore`] enforces.
#[derive(Debug, Clone)]
pub struct GuardConfig {
    /// Allow `DROP`, `CLEAR` and delete templates of three variables.
    pub allow_destructive_updates: bool,
    /// Largest SPARQL update, in bytes.
    pub max_update_bytes: usize,
    /// Largest inline RDF payload, in bytes.
    pub max_batch_bytes: usize,
    /// Largest file loaded in one transaction, in bytes.
    pub max_load_file_bytes: u64,
    /// Resolved `file_roots`, or `None` to leave file access unconfined.
    pub file_roots: Option<Vec<PathBuf>>,
}

impl Default for GuardConfig {
    fn default() -> Self {
        let c = IngestConfig::default();
        Self {
            allow_destructive_updates: c.allow_destructive_updates,
            max_update_bytes: c.max_update_bytes,
            max_batch_bytes: c.max_batch_bytes,
            max_load_file_bytes: c.max_load_file_bytes,
            file_roots: None,
        }
    }
}

impl GuardConfig {
    /// Build from `[ingest]`. Fails if a `file_roots` entry does not exist or
    /// is a path that must not be resolved (UNC, device), so a typo is a
    /// startup error rather than a silent "allow nothing".
    pub fn from_config(c: &IngestConfig) -> Result<Self> {
        let file_roots = c
            .file_roots
            .as_deref()
            .map(path_guard::resolve_roots)
            .transpose()?;
        Ok(Self {
            allow_destructive_updates: c.allow_destructive_updates,
            max_update_bytes: c.max_update_bytes,
            max_batch_bytes: c.max_batch_bytes,
            max_load_file_bytes: c.max_load_file_bytes,
            file_roots,
        })
    }
}

/// Write-gated, guarded view of a store, for non-privileged callers.
pub struct GatedStore {
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
    guards: GuardConfig,
}

/// Write-gated view of a store, for core modules that own internal graphs.
pub struct PrivilegedStore {
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
}

/// Build both handles from one store and one gate, with the default guards.
pub fn build_stores(
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
) -> (Arc<GatedStore>, Arc<PrivilegedStore>) {
    build_stores_with(inner, gate, GuardConfig::default())
}

/// Build both handles from one store and one gate.
pub fn build_stores_with(
    inner: Arc<dyn SparqlStore>,
    gate: WriteGate,
    guards: GuardConfig,
) -> (Arc<GatedStore>, Arc<PrivilegedStore>) {
    (
        Arc::new(GatedStore {
            inner: inner.clone(),
            gate: gate.clone(),
            guards,
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

/// The checks a handle applies before a write. `PrivilegedStore` applies none.
trait Policy {
    /// An update request, before it is parsed for execution.
    fn policy_update(&self, sparql: &str) -> Result<()>;
    /// An inline RDF payload of `bytes` bytes going to `graph` (`None` = the
    /// default graph).
    fn policy_inline(&self, bytes: usize, graph: Option<&str>) -> Result<()>;
    /// A file load into `graph`.
    fn policy_file(&self, path: &Path, graph: Option<&str>) -> Result<()>;
    /// A file about to be read: the resolved path to open.
    fn policy_read_path(&self, path: &Path) -> Result<PathBuf>;
    /// A file about to be written: the resolved path to create.
    fn policy_write_path(&self, path: &Path) -> Result<PathBuf>;
    /// Quads about to be written.
    fn policy_quads(&self, quads: &[StoredQuad]) -> Result<()>;
}

fn refuse_if_protected(graph: Option<&str>) -> Result<()> {
    match graph {
        Some(g) if is_protected(g) => Err(ToolError::GraphProtected {
            graph: g.to_string(),
        }
        .into_anyhow()),
        _ => Ok(()),
    }
}

impl Policy for GatedStore {
    fn policy_update(&self, sparql: &str) -> Result<()> {
        if sparql.len() > self.guards.max_update_bytes {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_update_bytes".to_string(),
                limit: self.guards.max_update_bytes as u64,
                value: sparql.len() as u64,
            }
            .into_anyhow());
        }
        let class = classify_sparql(sparql).map_err(ToolError::into_anyhow)?;
        check_update(&class, self.guards.allow_destructive_updates).map_err(ToolError::into_anyhow)
    }

    fn policy_inline(&self, bytes: usize, graph: Option<&str>) -> Result<()> {
        if bytes > self.guards.max_batch_bytes {
            return Err(ToolError::LimitExceeded {
                limit_key: "max_batch_bytes".to_string(),
                limit: self.guards.max_batch_bytes as u64,
                value: bytes as u64,
            }
            .into_anyhow());
        }
        refuse_if_protected(graph)
    }

    fn policy_file(&self, path: &Path, graph: Option<&str>) -> Result<()> {
        refuse_if_protected(graph)?;
        // A missing or unreadable file is the store's error to report.
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > self.guards.max_load_file_bytes {
                return Err(ToolError::LimitExceeded {
                    limit_key: "max_load_file_bytes".to_string(),
                    limit: self.guards.max_load_file_bytes,
                    value: meta.len(),
                }
                .into_anyhow());
            }
        }
        Ok(())
    }

    fn policy_read_path(&self, path: &Path) -> Result<PathBuf> {
        match &self.guards.file_roots {
            Some(roots) => path_guard::check_read(path, roots),
            None => {
                warn_unconfined();
                Ok(path.to_path_buf())
            }
        }
    }

    fn policy_write_path(&self, path: &Path) -> Result<PathBuf> {
        match &self.guards.file_roots {
            Some(roots) => path_guard::check_write(path, roots),
            None => {
                warn_unconfined();
                Ok(path.to_path_buf())
            }
        }
    }

    fn policy_quads(&self, quads: &[StoredQuad]) -> Result<()> {
        match quads
            .iter()
            .filter_map(|q| q.g.as_deref())
            .find(|g| is_protected(g))
        {
            Some(g) => refuse_if_protected(Some(g)),
            None => Ok(()),
        }
    }
}

impl Policy for PrivilegedStore {
    fn policy_update(&self, _sparql: &str) -> Result<()> {
        Ok(())
    }
    fn policy_inline(&self, _bytes: usize, _graph: Option<&str>) -> Result<()> {
        Ok(())
    }
    fn policy_file(&self, _path: &Path, _graph: Option<&str>) -> Result<()> {
        Ok(())
    }
    fn policy_read_path(&self, path: &Path) -> Result<PathBuf> {
        Ok(path.to_path_buf())
    }
    fn policy_write_path(&self, path: &Path) -> Result<PathBuf> {
        Ok(path.to_path_buf())
    }
    fn policy_quads(&self, _quads: &[StoredQuad]) -> Result<()> {
        Ok(())
    }
}

/// File access is unconfined while `[ingest] file_roots` is unset. Say so once.
fn warn_unconfined() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::warn!(
            "[ingest] file_roots is not set: file loads and exports can use any path. \n             Set file_roots to confine them."
        );
    });
}

macro_rules! gated_impl {
    ($ty:ty) => {
        impl SparqlStore for $ty {
            // ── gated writes ─────────────────────────────────────────────
            fn update(&self, sparql: &str) -> Result<()> {
                self.policy_update(sparql)?;
                self.gate
                    .run_blocking("update", || self.inner.update(sparql))
            }

            fn update_with_outcome(&self, sparql: &str) -> Result<UpdateOutcome> {
                self.policy_update(sparql)?;
                self.gate.run_blocking("update_with_outcome", || {
                    self.inner.update_with_outcome(sparql)
                })
            }

            fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64> {
                self.policy_inline(content.len(), opts.graph_iri.as_deref())?;
                self.gate
                    .run_blocking("load_rdf", || self.inner.load_rdf(content, opts))
            }

            fn load_rdf_with_triples(
                &self,
                content: &[u8],
                opts: LoadOpts,
            ) -> Result<(u64, Vec<StoredQuad>)> {
                self.policy_inline(content.len(), opts.graph_iri.as_deref())?;
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
                let path = self.policy_read_path(path)?;
                self.policy_file(&path, opts.graph_iri.as_deref())?;
                self.gate.run_blocking("load_rdf_file", || {
                    self.inner.load_rdf_file(&path, opts, force)
                })
            }

            fn load_ontology_dir(&self, dir: &Path) -> Result<usize> {
                self.gate
                    .run_blocking("load_ontology_dir", || self.inner.load_ontology_dir(dir))
            }

            fn write_quads(&self, quads: &[StoredQuad]) -> Result<u64> {
                self.policy_quads(quads)?;
                self.gate
                    .run_blocking("write_quads", || self.inner.write_quads(quads))
            }

            // Pure CPU work: no store access, so no gate.
            fn parse_rdf(&self, content: &[u8], opts: &LoadOpts) -> Result<Vec<StoredQuad>> {
                self.inner.parse_rdf(content, opts)
            }

            fn check_update(&self, sparql: &str) -> Result<()> {
                self.policy_update(sparql)
            }

            // ── pass-through reads ───────────────────────────────────────
            fn query(&self, sparql: &str) -> Result<QueryResult> {
                self.inner.query(sparql)
            }

            fn export_graph(&self, graph_iri: Option<&str>, dest: &Path) -> Result<usize> {
                let dest = self.policy_write_path(dest)?;
                self.inner.export_graph(graph_iri, &dest)
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
