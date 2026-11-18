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
/// One RDF quad with each term in its N-Triples encoding (`<iri>`, `_:b0`,
/// `"lit"^^<dt>`). `g` is the named graph IRI **without** angle brackets, or
/// `None` for the default graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoredQuad {
    pub s: String,
    pub p: String,
    pub o: String,
    pub g: Option<String>,
}

impl StoredQuad {
    /// The subject/predicate/object triple, as used by event payloads.
    pub fn triple(&self) -> [String; 3] {
        [self.s.clone(), self.p.clone(), self.o.clone()]
    }
}

/// What a SPARQL update changed.
///
/// A ground update (`INSERT DATA` / `DELETE DATA` only) reports exactly the
/// quads involved, with counts that cost no whole-store scan. A pattern
/// update (`INSERT/DELETE … WHERE`, `LOAD`, `CLEAR`, …) only reports
/// aggregate store sizes, which cost a full scan and are therefore only
/// computed for pattern updates.
#[derive(Debug, Clone)]
pub enum UpdateOutcome {
    Ground {
        inserted: Vec<StoredQuad>,
        deleted: Vec<StoredQuad>,
        /// Inserted quads that were not in the store before.
        inserted_new: u64,
        /// Deleted quads that were in the store before.
        removed_existing: u64,
    },
    Pattern {
        triples_before: u64,
        triples_after: u64,
    },
}

impl UpdateOutcome {
    /// Store size before the update. `None` for a ground update, which does
    /// not scan the store.
    pub fn triples_before(&self) -> Option<u64> {
        match self {
            Self::Ground { .. } => None,
            Self::Pattern { triples_before, .. } => Some(*triples_before),
        }
    }

    /// Store size after the update. `None` for a ground update.
    pub fn triples_after(&self) -> Option<u64> {
        match self {
            Self::Ground { .. } => None,
            Self::Pattern { triples_after, .. } => Some(*triples_after),
        }
    }

    /// Net change in quads: exact for a ground update, a difference of store
    /// sizes for a pattern update.
    pub fn delta(&self) -> i64 {
        match self {
            Self::Ground {
                inserted_new,
                removed_existing,
                ..
            } => *inserted_new as i64 - *removed_existing as i64,
            Self::Pattern {
                triples_before,
                triples_after,
            } => *triples_after as i64 - *triples_before as i64,
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
    /// Quads parsed from the file that were not in the store before.
    pub inserted_new: u64,
    /// Quads parsed from the file (empty when `skipped` is `true`): the
    /// exact data a `StoreEvent::Insert` for this load reports.
    pub triples: Vec<StoredQuad>,
}

impl FileLoadResult {
    pub fn delta(&self) -> i64 {
        self.inserted_new as i64
    }
}

/// A write that is applied only if conditions hold, with the conditions
/// checked and the write applied in **one transaction**
/// ([`SparqlStore::write_batch`]).
///
/// The port knows nothing about what the conditions mean (the ingestion jobs
/// use them to check that a job is open and that a batch was not already
/// accepted); it only evaluates them against the store's own state, so no
/// other writer can slip between the check and the write.
#[derive(Debug, Clone, Default)]
pub struct BatchWrite {
    /// The data to insert.
    pub quads: Vec<StoredQuad>,
    /// More quads inserted with the data (a batch record, for instance).
    pub ledger: Vec<StoredQuad>,
    /// Each of these quads must be in the store, or nothing is written
    /// ([`BatchRefusal::RequirementNotMet`] with its index).
    pub require: Vec<StoredQuad>,
    /// Refuse a write that was already accepted.
    pub idempotency: Option<Idempotency>,
    /// Numeric counters (`xsd:long` objects) to advance in the same
    /// transaction, each with an optional ceiling.
    pub counters: Vec<Counter>,
    /// Record, inside the transaction, how many of `quads` were new.
    pub record_counts: Option<RecordCounts>,
}

/// A write is *the same* when `identity` is already in the store, and a
/// *conflict* when something else is already recorded for `subject` in `graph`.
#[derive(Debug, Clone)]
pub struct Idempotency {
    pub subject: String,
    pub graph: String,
    pub identity: StoredQuad,
}

/// Add `add` to the number that is the object of `(subject, predicate)` in
/// `graph`. If the result would exceed `max`, nothing is written
/// ([`BatchRefusal::CounterLimit`]).
#[derive(Debug, Clone)]
pub struct Counter {
    pub subject: String,
    pub predicate: String,
    pub graph: String,
    pub add: u64,
    pub max: Option<u64>,
}

/// Write, in `graph`, the number of new and of already present quads as two
/// `xsd:long` quads on `subject`.
#[derive(Debug, Clone)]
pub struct RecordCounts {
    pub subject: String,
    pub graph: String,
    pub inserted_new_predicate: String,
    pub already_present_predicate: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchApplied {
    pub inserted_new: u64,
    pub already_present: u64,
    /// The counters after the write, in the order of `BatchWrite::counters`.
    pub counters: Vec<u64>,
}

/// Why [`SparqlStore::write_batch`] wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchRefusal {
    /// `require[i]` is not in the store.
    RequirementNotMet(usize),
    /// The idempotency identity is already recorded: the same write was
    /// accepted before.
    Duplicate,
    /// Something else is recorded for the same subject.
    Conflict,
    /// `counters[counter]` would reach `value`, above its ceiling `limit`.
    CounterLimit {
        counter: usize,
        limit: u64,
        value: u64,
    },
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
    ) -> Result<(u64, Vec<StoredQuad>)>;

    /// Parse RDF into quads **without touching the store**: pure CPU work that
    /// takes no write gate. Blank nodes are renamed per call, as in
    /// [`Self::load_rdf`]. Pair it with [`Self::write_quads`] to load a large
    /// payload in bounded chunks, releasing the write gate between them.
    fn parse_rdf(&self, content: &[u8], opts: &LoadOpts) -> Result<Vec<StoredQuad>>;

    /// Write `quads` in one transaction. Returns how many were not in the
    /// store before; the count costs no whole-store scan.
    fn write_quads(&self, quads: &[StoredQuad]) -> Result<u64>;

    /// Whether `update` would be refused for this handle's caller, without
    /// running it: too large, a syntax error, a protected graph, or a
    /// destructive operation. A plain store never refuses.
    fn check_update(&self, _sparql: &str) -> Result<()> {
        Ok(())
    }

    /// Like [`Self::parse_rdf`], but blank node labels are kept **as written**
    /// (`_:a` stays `_:a`) instead of being renamed, so a caller can map them
    /// to stable names. Nodes the parser generates itself (Turtle `[]`) get
    /// random labels. No graph is applied: every quad is in the default graph.
    fn parse_rdf_raw(&self, content: &[u8], format: InputFormat) -> Result<Vec<StoredQuad>>;

    /// Check the conditions of `write` and apply it, in one transaction. The
    /// outer `Err` is a store failure; the inner one is a refusal, and then
    /// nothing was written.
    fn write_batch(
        &self,
        write: &BatchWrite,
    ) -> Result<std::result::Result<BatchApplied, BatchRefusal>>;

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
