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

#[derive(Debug)]
pub struct FileLoadResult {
    pub path: String,
    pub format: InputFormat,
    pub bytes_read: usize,
    pub sha256: String,
    pub skipped: bool,
    pub triples_before: u64,
    pub triples_after: u64,
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

    /// Parse `content` and load it into the store.
    /// Returns the number of triples added (delta).
    fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64>;

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
