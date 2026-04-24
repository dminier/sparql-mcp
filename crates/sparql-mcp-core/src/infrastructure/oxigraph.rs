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
        let store = Store::open(path)
            .with_context(|| format!("opening Oxigraph store at {}", path.display()))?;
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

    fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64> {
        let before = self.store.len()? as u64;
        let format = to_oxigraph_format(opts.format);
        let mut parser = RdfParser::from_format(format);
        if let Some(base) = opts.base_iri {
            parser = parser.with_base_iri(&base)?;
        }
        if let Some(iri) = opts.graph_iri {
            let node = NamedNode::new(&iri)?;
            parser = parser.with_default_graph(GraphName::NamedNode(node));
        }
        self.store.load_from_reader(parser, content)?;
        let after = self.store.len()? as u64;
        Ok(after.saturating_sub(before))
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
            });
        }

        self.load_rdf(&bytes, opts)?;
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
        let turtle = format!(
            "@prefix meta: <urn:hkb:meta:> .\n\
             <urn:hkb:meta:file:{hash}> a <{META_FILE}> ;\n\
                 <{META_SHA256}> \"{hash}\" ;\n\
                 <{META_PATH}> \"{path}\" .\n"
        );
        let graph_node = NamedNode::new(META_GRAPH)?;
        let parser = RdfParser::from_format(RdfFormat::Turtle)
            .with_default_graph(GraphName::NamedNode(graph_node));
        self.store.load_from_reader(parser, turtle.as_bytes())?;
        Ok(())
    }
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
