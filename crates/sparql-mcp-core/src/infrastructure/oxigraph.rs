//! OxigraphAdapter — implements SparqlStore using Oxigraph's embedded store.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, GraphNameRef, NamedNode};
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::domain::{
    FileLoadResult, InputFormat, LoadOpts, QueryResult, RdfTerm, SolutionSet, SparqlStore,
    UpdateOutcome,
};

// Named graph holding SHA-256 records for idempotent file loading.
const META_GRAPH: &str = "urn:hkb:meta:loaded-files";
const META_SHA256: &str = "urn:hkb:meta:sha256";
const META_PATH: &str = "urn:hkb:meta:path";
const META_FILE: &str = "urn:hkb:meta:LoadedFile";

// ── Adapter ───────────────────────────────────────────────────────────────────

pub struct OxigraphAdapter {
    store: Store,
}

impl OxigraphAdapter {
    /// Open (or create) a persistent RocksDB-backed store at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        // RocksDB's `Store::open` only creates the leaf directory, not its
        // ancestors — required when the user first runs against a fresh
        // XDG data home like ~/.local/share/sparql-mcp/store.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating store parent directory {}", parent.display()))?;
        }
        let store = Store::open(path).with_context(|| {
            format!(
                "opening Oxigraph store at {} (another process may hold the RocksDB lock)",
                path.display()
            )
        })?;
        Ok(Self { store })
    }

    /// Create a non-persistent in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            store: Store::new()?,
        })
    }
}

impl SparqlStore for OxigraphAdapter {
    fn query(&self, sparql: &str) -> Result<QueryResult> {
        let results = SparqlEvaluator::new()
            .parse_query(sparql)?
            .on_store(&self.store)
            .execute()?;
        convert_query_results(results)
    }

    fn update(&self, sparql: &str) -> Result<()> {
        SparqlEvaluator::new()
            .parse_update(sparql)?
            .on_store(&self.store)
            .execute()?;
        Ok(())
    }

    fn update_with_outcome(&self, sparql: &str) -> Result<UpdateOutcome> {
        // `spargebra::Update` is parsed separately (and purely) from the
        // execution path below, just to classify the operations — the
        // oxigraph `Update` type that `.on_store(...)` needs is deprecated
        // for exactly this kind of inspection (see `ground_quads`).
        let ground = spargebra::SparqlParser::new()
            .parse_update(sparql)
            .ok()
            .and_then(|parsed| ground_quads(&parsed));

        let before = self.store.len()? as u64;
        SparqlEvaluator::new()
            .parse_update(sparql)?
            .on_store(&self.store)
            .execute()?;
        let after = self.store.len()? as u64;

        match ground {
            Some((inserted, deleted)) => Ok(UpdateOutcome::Ground {
                inserted,
                deleted,
                triples_before: before,
                triples_after: after,
            }),
            None => Ok(UpdateOutcome::Pattern {
                triples_before: before,
                triples_after: after,
            }),
        }
    }

    fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64> {
        self.load_rdf_impl(content, opts).map(|(delta, _)| delta)
    }

    fn load_rdf_with_triples(
        &self,
        content: &[u8],
        opts: LoadOpts,
    ) -> Result<(u64, Vec<[String; 3]>)> {
        self.load_rdf_impl(content, opts)
    }

    fn load_rdf_file(&self, path: &Path, opts: LoadOpts, force: bool) -> Result<FileLoadResult> {
        let path_str = path.to_string_lossy().to_string();
        let _format = InputFormat::guess_from_path(&path_str);
        let effective_format = opts.format;

        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let bytes_read = bytes.len();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));

        let triples_before = self.store.len()? as u64;

        if !force && self.file_already_loaded(&sha256)? {
            return Ok(FileLoadResult {
                path: path_str,
                format: effective_format,
                bytes_read,
                sha256,
                skipped: true,
                triples_before,
                triples_after: triples_before,
                triples: Vec::new(),
            });
        }

        let (_delta, triples) = self.load_rdf_impl(&bytes, opts)?;
        self.record_loaded_file(&sha256, &path_str)?;
        let triples_after = self.store.len()? as u64;

        Ok(FileLoadResult {
            path: path_str,
            format: effective_format,
            bytes_read,
            sha256,
            skipped: false,
            triples_before,
            triples_after,
            triples,
        })
    }

    fn export_graph(&self, graph_iri: Option<&str>, dest: &Path) -> Result<usize> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        match graph_iri {
            Some(iri) => {
                let node = NamedNode::new(iri)?;
                self.store.dump_graph_to_writer(
                    GraphNameRef::NamedNode(node.as_ref()),
                    RdfFormat::Turtle,
                    &mut buf,
                )?;
            }
            None => {
                self.store.dump_graph_to_writer(
                    GraphNameRef::DefaultGraph,
                    RdfFormat::Turtle,
                    &mut buf,
                )?;
            }
        }
        let n = buf.len();
        fs::write(dest, &buf).with_context(|| format!("writing export to {}", dest.display()))?;
        Ok(n)
    }

    fn list_graphs(&self) -> Result<Vec<String>> {
        let results = SparqlEvaluator::new()
            .parse_query("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY ?g")
            .expect("static query")
            .on_store(&self.store)
            .execute()?;
        let QueryResults::Solutions(solutions) = results else {
            anyhow::bail!("expected SELECT results");
        };
        let mut graphs = Vec::new();
        for sol in solutions {
            let sol = sol?;
            if let Some(oxigraph::model::Term::NamedNode(n)) = sol.get("g") {
                graphs.push(n.as_str().to_string());
            }
        }
        Ok(graphs)
    }

    fn triple_count(&self) -> Result<u64> {
        Ok(self.store.len()? as u64)
    }

    fn load_ontology_dir(&self, dir: &Path) -> Result<usize> {
        if !dir.exists() {
            info!(path = %dir.display(), "ontology directory does not exist; skipping load");
            return Ok(0);
        }
        let mut paths: Vec<_> = fs::read_dir(dir)
            .with_context(|| format!("reading ontology directory {}", dir.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("ttl"))
            .collect();
        paths.sort();

        let mut count = 0usize;
        for path in &paths {
            debug!(path = %path.display(), "loading ontology file");
            let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            self.store
                .load_from_reader(RdfFormat::Turtle, bytes.as_slice())
                .with_context(|| format!("parsing {}", path.display()))?;
            count += 1;
        }
        info!(files = count, "ontology loaded from disk");
        Ok(count)
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

impl OxigraphAdapter {
    /// Parse `content` and insert it, returning both the delta and every
    /// triple parsed (N-Triples-encoded) — `load_rdf` and
    /// `load_rdf_with_triples` are thin wrappers around this so both stay
    /// in sync.
    fn load_rdf_impl(&self, content: &[u8], opts: LoadOpts) -> Result<(u64, Vec<[String; 3]>)> {
        let before = self.store.len()? as u64;
        let format = to_oxigraph_format(opts.format);
        let mut parser = RdfParser::from_format(format);
        if let Some(base) = &opts.base_iri {
            parser = parser.with_base_iri(base)?;
        }
        if let Some(iri) = &opts.graph_iri {
            let node = NamedNode::new(iri)?;
            parser = parser.with_default_graph(GraphName::NamedNode(node));
        }

        let mut triples = Vec::new();
        for quad in parser.for_reader(content) {
            let quad = quad?;
            triples.push(quad_to_triple(&quad));
            self.store.insert(quad.as_ref())?;
        }

        let after = self.store.len()? as u64;
        Ok((after.saturating_sub(before), triples))
    }

    fn file_already_loaded(&self, hash: &str) -> Result<bool> {
        let q = format!("ASK {{ GRAPH <{META_GRAPH}> {{ ?f <{META_SHA256}> \"{hash}\" }} }}");
        match SparqlEvaluator::new()
            .parse_query(&q)?
            .on_store(&self.store)
            .execute()?
        {
            QueryResults::Boolean(b) => Ok(b),
            _ => Ok(false),
        }
    }

    fn record_loaded_file(&self, hash: &str, path: &str) -> Result<()> {
        // `path` is a filesystem path, not a pre-escaped Turtle literal —
        // on native Windows it always contains `\`, which Turtle's string
        // grammar treats as the start of an escape sequence (breaking the
        // parse on anything but `\\`, `\"`, `\uXXXX`, …). Escape `\` and
        // `"` before embedding it.
        let escaped_path = path.replace('\\', "\\\\").replace('"', "\\\"");
        let turtle = format!(
            "@prefix meta: <urn:hkb:meta:> .\n\
             <urn:hkb:meta:file:{hash}> a <{META_FILE}> ;\n\
                 <{META_SHA256}> \"{hash}\" ;\n\
                 <{META_PATH}> \"{escaped_path}\" .\n"
        );
        let graph_node = NamedNode::new(META_GRAPH)?;
        let parser = RdfParser::from_format(RdfFormat::Turtle)
            .with_default_graph(GraphName::NamedNode(graph_node));
        self.store.load_from_reader(parser, turtle.as_bytes())?;
        Ok(())
    }
}

// ── Update classification ─────────────────────────────────────────────────────

/// If `update` consists solely of ground `INSERT DATA` / `DELETE DATA`
/// operations (no `WHERE` pattern), return the literal quads inserted and
/// deleted, each encoded as an N-Triples term string. Any other operation
/// (`INSERT/DELETE … WHERE`, `LOAD`, `CLEAR`, `CREATE`, `DROP`, `ADD`,
/// `MOVE`, `COPY`) makes the whole update non-ground: `None`.
/// (inserted triples, deleted triples), each N-Triples-encoded.
type GroundTriples = (Vec<[String; 3]>, Vec<[String; 3]>);

fn ground_quads(update: &spargebra::Update) -> Option<GroundTriples> {
    let mut inserted = Vec::new();
    let mut deleted = Vec::new();
    for op in &update.operations {
        match op {
            // `INSERT DATA` allows blank nodes (`spargebra::term::Quad`);
            // `DELETE DATA` forbids them per the SPARQL 1.1 grammar, so
            // spargebra gives it the narrower `GroundQuad` type instead —
            // both have the same `subject`/`predicate`/`object` shape.
            spargebra::GraphUpdateOperation::InsertData { data } => {
                inserted.extend(data.iter().map(|q| {
                    [
                        q.subject.to_string(),
                        q.predicate.to_string(),
                        q.object.to_string(),
                    ]
                }));
            }
            spargebra::GraphUpdateOperation::DeleteData { data } => {
                deleted.extend(data.iter().map(|q| {
                    [
                        q.subject.to_string(),
                        q.predicate.to_string(),
                        q.object.to_string(),
                    ]
                }));
            }
            _ => return None,
        }
    }
    Some((inserted, deleted))
}

/// Render an RDF-parser quad's subject/predicate/object as N-Triples term
/// strings (`Display` on Oxigraph's term types already produces this
/// syntax — see `convert_query_results`'s `Graph` branch above for the
/// same pattern).
fn quad_to_triple(q: &oxigraph::model::Quad) -> [String; 3] {
    [
        q.subject.to_string(),
        q.predicate.to_string(),
        q.object.to_string(),
    ]
}

// ── Conversion helpers ────────────────────────────────────────────────────────

fn to_oxigraph_format(fmt: InputFormat) -> RdfFormat {
    match fmt {
        InputFormat::Turtle => RdfFormat::Turtle,
        InputFormat::NTriples => RdfFormat::NTriples,
        InputFormat::RdfXml => RdfFormat::RdfXml,
    }
}

fn convert_query_results(results: QueryResults<'_>) -> Result<QueryResult> {
    match results {
        QueryResults::Boolean(b) => Ok(QueryResult::Boolean(b)),
        QueryResults::Solutions(solutions) => {
            let variables: Vec<String> = solutions
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            let mut rows = Vec::new();
            for sol in solutions {
                let sol = sol?;
                let mut row = HashMap::new();
                for (var, term) in sol.iter() {
                    row.insert(var.as_str().to_string(), convert_term(term));
                }
                rows.push(row);
            }
            Ok(QueryResult::Solutions(SolutionSet { variables, rows }))
        }
        QueryResults::Graph(triples) => {
            let mut out = String::new();
            for t in triples {
                let t = t?;
                out.push_str(&format!("{} {} {} .\n", t.subject, t.predicate, t.object));
            }
            Ok(QueryResult::Graph(out))
        }
    }
}

fn convert_term(term: &oxigraph::model::Term) -> RdfTerm {
    match term {
        oxigraph::model::Term::NamedNode(n) => RdfTerm::Iri(n.as_str().to_string()),
        oxigraph::model::Term::BlankNode(b) => RdfTerm::BlankNode(b.as_str().to_string()),
        oxigraph::model::Term::Literal(l) => RdfTerm::Literal {
            value: l.value().to_string(),
            datatype: Some(l.datatype().as_str().to_string()),
            lang: l.language().map(str::to_string),
        },
    }
}
