//! SparqlStore port — the only storage interface the application touches.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

// ── Input types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputFormat {
    #[default]
    Turtle,
    NTriples,
    RdfXml,
}

impl InputFormat {
    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "turtle" | "ttl" => Some(Self::Turtle),
            "ntriples" | "nt" => Some(Self::NTriples),
            "rdfxml" | "rdf" | "xml" => Some(Self::RdfXml),
            _ => None,
        }
    }

    pub fn guess_from_path(path: &str) -> Self {
        let p = path.to_ascii_lowercase();
        if p.ends_with(".nt") {
            Self::NTriples
        } else if p.ends_with(".rdf") || p.ends_with(".xml") {
            Self::RdfXml
        } else {
            Self::Turtle
        }
    }

    pub fn as_label(self) -> &'static str {
        match self {
            Self::Turtle => "turtle",
            Self::NTriples => "ntriples",
            Self::RdfXml => "rdfxml",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoadOpts {
    pub format: InputFormat,
    pub graph_iri: Option<String>,
    pub base_iri: Option<String>,
}

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum RdfTerm {
    Iri(String),
    BlankNode(String),
    Literal {
        value: String,
        datatype: Option<String>,
        lang: Option<String>,
    },
}

impl RdfTerm {
    pub fn as_value_str(&self) -> &str {
        match self {
            Self::Iri(s) | Self::BlankNode(s) => s,
            Self::Literal { value, .. } => value,
        }
    }
}

#[derive(Debug)]
pub struct SolutionSet {
    pub variables: Vec<String>,
    pub rows: Vec<HashMap<String, RdfTerm>>,
}

#[derive(Debug)]
pub enum QueryResult {
    Boolean(bool),
    Solutions(SolutionSet),
    /// N-Triples formatted graph.
    Graph(String),
}

/// What a SPARQL 1.1 Update actually changed, for event emission.
///
/// A ground update (`INSERT DATA` / `DELETE DATA`) names the exact triples
/// touched, encoded as N-Triples term strings (subject, predicate, object).
/// A pattern-based update (`INSERT/DELETE … WHERE`, `LOAD`, `CLEAR`, …)
/// only reports aggregate counts — the literal triples bound by the
/// pattern are not cheaply knowable.
#[derive(Debug, Clone)]
pub enum UpdateOutcome {
    Ground {
        inserted: Vec<[String; 3]>,
        deleted: Vec<[String; 3]>,
        triples_before: u64,
        triples_after: u64,
    },
    Pattern {
        triples_before: u64,
        triples_after: u64,
    },
}

impl UpdateOutcome {
    pub fn triples_before(&self) -> u64 {
        match self {
            Self::Ground { triples_before, .. } | Self::Pattern { triples_before, .. } => {
                *triples_before
            }
        }
    }

    pub fn triples_after(&self) -> u64 {
        match self {
            Self::Ground { triples_after, .. } | Self::Pattern { triples_after, .. } => {
                *triples_after
            }
        }
    }
}

#[derive(Debug)]
pub struct FileLoadResult {
    pub path: String,
    pub format: InputFormat,
    pub bytes_read: usize,
    pub sha256: String,
    pub skipped: bool,
    pub triples_before: u64,
    pub triples_after: u64,
    /// Triples parsed from the file, N-Triples-encoded (empty when
    /// `skipped` is `true`) — the exact data a `StoreEvent::Insert` for
    /// this load reports.
    pub triples: Vec<[String; 3]>,
}

impl FileLoadResult {
    pub fn delta(&self) -> i64 {
        self.triples_after as i64 - self.triples_before as i64
    }
}

// ── Port trait ────────────────────────────────────────────────────────────────

/// The sole storage abstraction used by the application layer.
///
/// Swap the implementation (Oxigraph → Fuseki, Stardog, …) by providing a
/// different infrastructure adapter without touching application code.
pub trait SparqlStore: Send + Sync {
    /// Run a SPARQL 1.1 SELECT / ASK / CONSTRUCT / DESCRIBE query.
    fn query(&self, sparql: &str) -> Result<QueryResult>;

    /// Run a SPARQL 1.1 Update (INSERT DATA, DELETE, CLEAR, …).
    fn update(&self, sparql: &str) -> Result<()>;

    /// Run a SPARQL 1.1 Update and report what changed (see
    /// [`UpdateOutcome`]) — used by the viewer side-channel to emit precise
    /// events for ground updates without a full-graph diff.
    fn update_with_outcome(&self, sparql: &str) -> Result<UpdateOutcome>;

    /// Parse `content` and load it into the store.
    /// Returns the number of triples added (delta).
    fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64>;

    /// Same as [`Self::load_rdf`], but also returns every triple parsed
    /// (N-Triples-encoded) — used by `load_ontology` to emit a precise
    /// `StoreEvent::Insert`, since the payload is always ground data.
    fn load_rdf_with_triples(
        &self,
        content: &[u8],
        opts: LoadOpts,
    ) -> Result<(u64, Vec<[String; 3]>)>;

    /// Load an RDF file from disk, with SHA-256 idempotence.
    fn load_rdf_file(&self, path: &Path, opts: LoadOpts, force: bool) -> Result<FileLoadResult>;

    /// Dump a named graph (or default graph when `graph_iri` is `None`)
    /// to a Turtle file at `dest`. Returns bytes written.
    fn export_graph(&self, graph_iri: Option<&str>, dest: &Path) -> Result<usize>;

    /// List all named graph IRIs present in the store, sorted.
    fn list_graphs(&self) -> Result<Vec<String>>;

    /// Total number of quads currently in the store.
    fn triple_count(&self) -> Result<u64>;

    /// Load every `*.ttl` file in `dir` into the default graph.
    fn load_ontology_dir(&self, dir: &Path) -> Result<usize>;
}
