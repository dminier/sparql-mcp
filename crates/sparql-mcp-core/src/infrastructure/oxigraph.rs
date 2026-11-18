//! OxigraphAdapter — implements SparqlStore using Oxigraph's embedded store.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, GraphNameRef, Literal, NamedNode, Quad, Term};
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::domain::{
    FileLoadResult, InputFormat, LoadOpts, QueryResult, RdfTerm, SolutionSet, SparqlStore,
    StoredQuad, UpdateOutcome,
};

// Named graph holding SHA-256 records for idempotent file loading.
const META_GRAPH: &str = "urn:hkb:meta:loaded-files";
const META_SHA256: &str = "urn:hkb:meta:sha256";
const META_PATH: &str = "urn:hkb:meta:path";
const META_FILE: &str = "urn:hkb:meta:LoadedFile";
/// The graph a loaded file went into (empty string for the default graph).
const META_TARGET: &str = "urn:hkb:meta:graph";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

// ── Adapter ───────────────────────────────────────────────────────────────────

pub struct OxigraphAdapter {
    store: Store,
    /// Test-only count of whole-store scans (`Store::len`), so a test can
    /// assert that ground writes cost no scan.
    #[cfg(test)]
    len_calls: std::sync::atomic::AtomicUsize,
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
        Ok(Self::from_store(store))
    }

    /// Create a non-persistent in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self> {
        Ok(Self::from_store(Store::new()?))
    }

    fn from_store(store: Store) -> Self {
        Self {
            store,
            #[cfg(test)]
            len_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The only place that calls `Store::len`, which Oxigraph documents as
    /// "executes a full scan". Ground writes and loads never call it.
    fn store_len(&self) -> Result<u64> {
        #[cfg(test)]
        self.len_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.store.len()? as u64)
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

        match ground {
            Some(ground) => {
                // A ground update names every quad it touches, so its effect
                // is counted with `contains` and never needs a whole-store
                // scan. Duplicate quads in the request count once.
                let mut inserted_new = 0u64;
                for q in ground.inserted.iter().collect::<HashSet<_>>() {
                    if !self.store.contains(q)? {
                        inserted_new += 1;
                    }
                }
                let mut removed_existing = 0u64;
                for q in ground.deleted.iter().collect::<HashSet<_>>() {
                    if self.store.contains(q)? {
                        removed_existing += 1;
                    }
                }
                SparqlEvaluator::new()
                    .parse_update(sparql)?
                    .on_store(&self.store)
                    .execute()?;
                Ok(UpdateOutcome::Ground {
                    inserted: ground.inserted.iter().map(to_stored).collect(),
                    deleted: ground.deleted.iter().map(to_stored).collect(),
                    inserted_new,
                    removed_existing,
                })
            }
            None => {
                // A pattern update only reports aggregate sizes, which is a
                // full scan before and after. It is rare and small.
                let before = self.store_len()?;
                SparqlEvaluator::new()
                    .parse_update(sparql)?
                    .on_store(&self.store)
                    .execute()?;
                let after = self.store_len()?;
                Ok(UpdateOutcome::Pattern {
                    triples_before: before,
                    triples_after: after,
                })
            }
        }
    }

    fn load_rdf(&self, content: &[u8], opts: LoadOpts) -> Result<u64> {
        self.load_rdf_impl(content, opts, &[])
            .map(|(inserted_new, _)| inserted_new)
    }

    fn load_rdf_with_triples(
        &self,
        content: &[u8],
        opts: LoadOpts,
    ) -> Result<(u64, Vec<StoredQuad>)> {
        self.load_rdf_impl(content, opts, &[])
    }

    fn load_rdf_file(&self, path: &Path, opts: LoadOpts, force: bool) -> Result<FileLoadResult> {
        let path_str = path.to_string_lossy().to_string();
        let _format = InputFormat::guess_from_path(&path_str);
        let effective_format = opts.format;

        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let bytes_read = bytes.len();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));

        // The ledger entry is keyed by (file hash, target graph): the same
        // file loaded into two graphs is two loads.
        let target = opts.graph_iri.clone().unwrap_or_default();

        if !force && self.file_already_loaded(&sha256, &target)? {
            return Ok(FileLoadResult {
                path: path_str,
                format: effective_format,
                bytes_read,
                sha256,
                skipped: true,
                inserted_new: 0,
                triples: Vec::new(),
            });
        }

        // The ledger entry is written in the same transaction as the data,
        // so a load is never recorded without having landed, or vice versa.
        let ledger = loaded_file_quads(&sha256, &target, &path_str)?;
        let (inserted_new, triples) = self.load_rdf_impl(&bytes, opts, &ledger)?;

        Ok(FileLoadResult {
            path: path_str,
            format: effective_format,
            bytes_read,
            sha256,
            skipped: false,
            inserted_new,
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
        self.store_len()
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
    /// Parse `content` completely, then write it, plus the `extra` quads (a
    /// ledger entry), in **one** transaction. A syntax error therefore leaves
    /// nothing behind.
    ///
    /// Returns how many parsed quads were not in the store before, and every
    /// parsed quad. `Transaction::contains` sees the transaction's own
    /// writes, so a quad repeated in the payload counts once, and the count
    /// costs no whole-store scan.
    ///
    /// Blank nodes are renamed per call: two separate calls that both use
    /// `_:b0` are two different nodes, never silently merged.
    fn load_rdf_impl(
        &self,
        content: &[u8],
        opts: LoadOpts,
        extra: &[Quad],
    ) -> Result<(u64, Vec<StoredQuad>)> {
        let format = to_oxigraph_format(opts.format);
        let mut parser = RdfParser::from_format(format).rename_blank_nodes();
        if let Some(base) = &opts.base_iri {
            parser = parser.with_base_iri(base)?;
        }
        if let Some(iri) = &opts.graph_iri {
            let node = NamedNode::new(iri)?;
            parser = parser.with_default_graph(GraphName::NamedNode(node));
        }

        let quads: Vec<Quad> = parser
            .for_reader(content)
            .collect::<std::result::Result<_, _>>()?;

        let mut tx = self.store.start_transaction()?;
        let mut inserted_new = 0u64;
        for q in &quads {
            if !tx.contains(q)? {
                inserted_new += 1;
            }
            tx.insert(q);
        }
        for q in extra {
            tx.insert(q);
        }
        tx.commit()?;

        Ok((inserted_new, quads.iter().map(to_stored).collect()))
    }

    /// Whether `hash` was already loaded into `target` (`""` = default graph).
    /// Entries written before the ledger was keyed by graph have no
    /// `meta:graph` and never match, so such a file is reloaded once, which is
    /// idempotent.
    fn file_already_loaded(&self, hash: &str, target: &str) -> Result<bool> {
        let target = target.replace('\\', "\\\\").replace('"', "\\\"");
        let q = format!(
            "ASK {{ GRAPH <{META_GRAPH}> {{ ?f <{META_SHA256}> \"{hash}\" ; <{META_TARGET}> \"{target}\" }} }}"
        );
        match SparqlEvaluator::new()
            .parse_query(&q)?
            .on_store(&self.store)
            .execute()?
        {
            QueryResults::Boolean(b) => Ok(b),
            _ => Ok(false),
        }
    }
}

/// The ledger entry for one loaded file, as quads in the ledger graph. Built
/// directly as terms, not as Turtle text, so a Windows path with `\` needs no
/// escaping.
fn loaded_file_quads(hash: &str, target: &str, path: &str) -> Result<Vec<Quad>> {
    let target_hash = format!("{:x}", Sha256::digest(target.as_bytes()));
    let subject = NamedNode::new(format!("urn:hkb:meta:file:{hash}:{target_hash}"))?;
    let graph = GraphName::NamedNode(NamedNode::new(META_GRAPH)?);
    let entry = |predicate: &str, object: Term| -> Result<Quad> {
        Ok(Quad::new(
            subject.clone(),
            NamedNode::new(predicate)?,
            object,
            graph.clone(),
        ))
    };
    Ok(vec![
        entry(RDF_TYPE, Term::NamedNode(NamedNode::new(META_FILE)?))?,
        entry(META_SHA256, Literal::new_simple_literal(hash).into())?,
        entry(META_PATH, Literal::new_simple_literal(path).into())?,
        entry(META_TARGET, Literal::new_simple_literal(target).into())?,
    ])
}

// ── Update classification ─────────────────────────────────────────────────────

/// The quads of a ground update, as Oxigraph quads.
struct GroundQuads {
    inserted: Vec<Quad>,
    deleted: Vec<Quad>,
}

/// If `update` consists solely of ground `INSERT DATA` / `DELETE DATA`
/// operations (no `WHERE` pattern), return the quads inserted and deleted,
/// each with its own graph. Any other operation (`INSERT/DELETE … WHERE`,
/// `LOAD`, `CLEAR`, `CREATE`, `DROP`, `ADD`, `MOVE`, `COPY`) makes the whole
/// update non-ground: `None`.
fn ground_quads(update: &spargebra::Update) -> Option<GroundQuads> {
    use spargebra::term::{GraphName as SparqlGraph, GroundTerm};

    let graph = |g: &SparqlGraph| match g {
        SparqlGraph::NamedNode(n) => GraphName::NamedNode(n.clone()),
        SparqlGraph::DefaultGraph => GraphName::DefaultGraph,
    };
    let mut inserted = Vec::new();
    let mut deleted = Vec::new();
    for op in &update.operations {
        match op {
            // `INSERT DATA` allows blank nodes (`spargebra::term::Quad`);
            // `DELETE DATA` forbids them per the SPARQL 1.1 grammar, so
            // spargebra gives it the narrower `GroundQuad` type instead.
            spargebra::GraphUpdateOperation::InsertData { data } => {
                for q in data {
                    inserted.push(Quad::new(
                        q.subject.clone(),
                        q.predicate.clone(),
                        q.object.clone(),
                        graph(&q.graph_name),
                    ));
                }
            }
            spargebra::GraphUpdateOperation::DeleteData { data } => {
                for q in data {
                    let object = match &q.object {
                        GroundTerm::NamedNode(n) => Term::NamedNode(n.clone()),
                        GroundTerm::Literal(l) => Term::Literal(l.clone()),
                        #[allow(unreachable_patterns)]
                        _ => return None,
                    };
                    deleted.push(Quad::new(
                        q.subject.clone(),
                        q.predicate.clone(),
                        object,
                        graph(&q.graph_name),
                    ));
                }
            }
            _ => return None,
        }
    }
    Some(GroundQuads { inserted, deleted })
}

/// A quad with each term in its N-Triples encoding (`Display` on Oxigraph's
/// term types already produces that syntax) and its graph name.
fn to_stored(q: &Quad) -> StoredQuad {
    StoredQuad {
        s: q.subject.to_string(),
        p: q.predicate.to_string(),
        o: q.object.to_string(),
        g: match &q.graph_name {
            GraphName::NamedNode(n) => Some(n.as_str().to_string()),
            GraphName::BlankNode(b) => Some(b.to_string()),
            GraphName::DefaultGraph => None,
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn adapter() -> OxigraphAdapter {
        OxigraphAdapter::open_in_memory().unwrap()
    }

    fn scans(a: &OxigraphAdapter) -> usize {
        a.len_calls.load(Ordering::SeqCst)
    }

    #[test]
    fn ground_writes_and_loads_never_scan_the_store() {
        let a = adapter();
        a.load_rdf(
            b"<urn:s> <urn:p> <urn:o> .",
            LoadOpts {
                format: InputFormat::NTriples,
                ..Default::default()
            },
        )
        .unwrap();
        a.update_with_outcome("INSERT DATA { <urn:a> <urn:p> <urn:b> }")
            .unwrap();
        a.update_with_outcome("DELETE DATA { <urn:a> <urn:p> <urn:b> }")
            .unwrap();
        assert_eq!(scans(&a), 0, "a ground write called Store::len");

        // A pattern update still needs the aggregate sizes: two scans.
        a.update_with_outcome("DELETE WHERE { ?s ?p ?o }").unwrap();
        assert_eq!(scans(&a), 2);
    }

    #[test]
    fn ground_update_counts_are_exact_and_name_the_graph() {
        let a = adapter();
        let out = a
            .update_with_outcome(
                "INSERT DATA { GRAPH <urn:g> { <urn:s> <urn:p> <urn:o> . <urn:s> <urn:p> <urn:o> } \
                 <urn:d> <urn:p> <urn:o> }",
            )
            .unwrap();
        match out {
            UpdateOutcome::Ground {
                inserted,
                inserted_new,
                ..
            } => {
                // The repeated quad counts once, and the default-graph quad is
                // a separate quad.
                assert_eq!(inserted_new, 2);
                assert!(inserted.iter().any(|q| q.g.as_deref() == Some("urn:g")));
                assert!(inserted.iter().any(|q| q.g.is_none()));
            }
            other => panic!("expected ground, got {other:?}"),
        }
        // Inserting the same data again adds nothing.
        match a
            .update_with_outcome("INSERT DATA { <urn:d> <urn:p> <urn:o> }")
            .unwrap()
        {
            UpdateOutcome::Ground { inserted_new, .. } => assert_eq!(inserted_new, 0),
            other => panic!("expected ground, got {other:?}"),
        }
    }

    #[test]
    fn a_syntax_error_leaves_nothing_behind() {
        let a = adapter();
        let mut doc = String::new();
        for n in 0..100 {
            doc.push_str(&format!("<urn:s:{n}> <urn:p> <urn:o> .\n"));
        }
        doc.push_str("<urn:broken> <urn:p> .\n"); // missing object
        let err = a.load_rdf(
            doc.as_bytes(),
            LoadOpts {
                format: InputFormat::NTriples,
                ..Default::default()
            },
        );
        assert!(err.is_err());
        assert_eq!(a.store.len().unwrap(), 0, "a partial load was written");
    }

    #[test]
    fn blank_node_labels_are_not_merged_across_calls() {
        let a = adapter();
        let load = |ttl: &str| {
            a.load_rdf(
                ttl.as_bytes(),
                LoadOpts {
                    format: InputFormat::NTriples,
                    graph_iri: Some("urn:g".into()),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        load("_:b0 <urn:name> \"first\" .");
        load("_:b0 <urn:name> \"second\" .");
        let rows = match a
            .query("SELECT DISTINCT ?s WHERE { GRAPH <urn:g> { ?s <urn:name> ?n } }")
            .unwrap()
        {
            QueryResult::Solutions(s) => s.rows.len(),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(rows, 2, "_:b0 from two calls was merged into one node");
    }

    #[test]
    fn the_same_file_is_a_separate_load_per_graph() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.nt");
        fs::write(&path, "<urn:s> <urn:p> <urn:o> .\n").unwrap();
        let a = adapter();
        let load = |graph: &str| {
            a.load_rdf_file(
                &path,
                LoadOpts {
                    format: InputFormat::NTriples,
                    graph_iri: Some(graph.into()),
                    ..Default::default()
                },
                false,
            )
            .unwrap()
        };
        assert!(!load("urn:g1").skipped);
        assert!(load("urn:g1").skipped, "same file, same graph");
        let second = load("urn:g2");
        assert!(
            !second.skipped,
            "same file into another graph is a new load"
        );
        assert_eq!(second.inserted_new, 1);
        assert_eq!(scans(&a), 0);
    }
}
