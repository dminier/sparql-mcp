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
    BatchApplied, BatchRefusal, BatchWrite, BoundedOutcome, FileLoadResult, InputFormat, LoadOpts,
    QueryResult, RdfTerm, SolutionSet, SparqlStore, StoredQuad, UpdateOutcome,
};

use crate::domain::loaded_files::{META_FILE, META_GRAPH, META_PATH, META_SHA256, META_TARGET};
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
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

    fn query_bounded(
        &self,
        sparql: &str,
        max_rows: usize,
        timeout: std::time::Duration,
    ) -> Result<BoundedOutcome> {
        use oxigraph::sparql::CancellationToken;
        let token = CancellationToken::new();
        // A timer cancels the evaluation. Dropping `done` (when this function
        // returns) wakes the timer thread, which then exits without cancelling.
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let timer = {
            let token = token.clone();
            std::thread::spawn(move || {
                if wait.recv_timeout(timeout) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                    token.cancel();
                }
            })
        };
        let outcome = (|| -> Result<BoundedOutcome> {
            let results = SparqlEvaluator::new()
                .with_cancellation_token(token.clone())
                .parse_query(sparql)?
                .on_store(&self.store)
                .execute();
            let results = match results {
                Ok(r) => r,
                Err(_) if token.is_cancelled() => return Ok(BoundedOutcome::TimedOut),
                Err(e) => return Err(e.into()),
            };
            match results {
                QueryResults::Boolean(b) => Ok(BoundedOutcome::Answer(b)),
                QueryResults::Solutions(solutions) => {
                    let variables = solutions
                        .variables()
                        .iter()
                        .map(|v| v.as_str().to_string())
                        .collect();
                    let mut rows = Vec::new();
                    let mut truncated = false;
                    for sol in solutions {
                        let sol = match sol {
                            Ok(s) => s,
                            Err(_) if token.is_cancelled() => return Ok(BoundedOutcome::TimedOut),
                            Err(e) => return Err(e.into()),
                        };
                        if rows.len() == max_rows {
                            truncated = true;
                            break;
                        }
                        let mut row = HashMap::new();
                        for (var, term) in sol.iter() {
                            row.insert(var.as_str().to_string(), convert_term(term));
                        }
                        rows.push(row);
                    }
                    Ok(BoundedOutcome::Rows {
                        variables,
                        rows,
                        truncated,
                    })
                }
                QueryResults::Graph(_) => anyhow::bail!("only SELECT and ASK are supported here"),
            }
        })();
        drop(done);
        let _ = timer.join();
        outcome
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

    fn parse_rdf(&self, content: &[u8], opts: &LoadOpts) -> Result<Vec<StoredQuad>> {
        Ok(parse_quads(content, opts)?.iter().map(to_stored).collect())
    }

    fn parse_rdf_raw(&self, content: &[u8], format: InputFormat) -> Result<Vec<StoredQuad>> {
        let parser = RdfParser::from_format(to_oxigraph_format(format));
        let quads: Vec<Quad> = parser
            .for_reader(content)
            .collect::<std::result::Result<_, _>>()?;
        Ok(quads.iter().map(to_stored).collect())
    }

    fn write_batch(
        &self,
        write: &BatchWrite,
    ) -> Result<std::result::Result<BatchApplied, BatchRefusal>> {
        let quads: Vec<Quad> = write.quads.iter().map(from_stored).collect::<Result<_>>()?;
        let ledger: Vec<Quad> = write
            .ledger
            .iter()
            .map(from_stored)
            .collect::<Result<_>>()?;
        let mut tx = self.store.start_transaction()?;

        // 1. The conditions, evaluated against what is in the store *now*, in
        //    the same transaction as the write.
        for (i, required) in write.require.iter().enumerate() {
            if !tx.contains(&from_stored(required)?)? {
                return Ok(Err(BatchRefusal::RequirementNotMet(i)));
            }
        }
        if let Some(idem) = &write.idempotency {
            if tx.contains(&from_stored(&idem.identity)?)? {
                return Ok(Err(BatchRefusal::Duplicate));
            }
            let subject = NamedNode::new(&idem.subject)?;
            let graph = GraphName::NamedNode(NamedNode::new(&idem.graph)?);
            let recorded = tx
                .quads_for_pattern(
                    Some(subject.as_ref().into()),
                    None,
                    None,
                    Some(graph.as_ref()),
                )
                .next()
                .transpose()?
                .is_some();
            if recorded {
                return Ok(Err(BatchRefusal::Conflict));
            }
        }

        // 2. The counters: read each, check its ceiling, replace it.
        let long = NamedNode::new(format!("{XSD}long"))?;
        let mut counters = Vec::with_capacity(write.counters.len());
        let mut counter_updates: Vec<(Quad, Quad)> = Vec::new();
        for (i, c) in write.counters.iter().enumerate() {
            let subject = NamedNode::new(&c.subject)?;
            let predicate = NamedNode::new(&c.predicate)?;
            let graph = GraphName::NamedNode(NamedNode::new(&c.graph)?);
            let current = tx
                .quads_for_pattern(
                    Some(subject.as_ref().into()),
                    Some(predicate.as_ref()),
                    None,
                    Some(graph.as_ref()),
                )
                .next()
                .transpose()?;
            let before: u64 = match &current {
                Some(q) => match &q.object {
                    Term::Literal(l) => l.value().parse().unwrap_or(0),
                    _ => 0,
                },
                None => 0,
            };
            let after = before.saturating_add(c.add);
            if let Some(max) = c.max {
                if after > max {
                    return Ok(Err(BatchRefusal::CounterLimit {
                        counter: i,
                        limit: max,
                        value: after,
                    }));
                }
            }
            let new_quad = Quad::new(
                subject,
                predicate,
                Literal::new_typed_literal(after.to_string(), long.clone()),
                graph,
            );
            counter_updates.push((current.unwrap_or_else(|| new_quad.clone()), new_quad));
            counters.push(after);
        }

        // 3. The write: the data, counted with `contains`, then the ledger.
        let mut inserted_new = 0u64;
        for q in &quads {
            if !tx.contains(q)? {
                inserted_new += 1;
            }
            tx.insert(q);
        }
        let already_present = (quads.len() as u64).saturating_sub(inserted_new);
        for q in &ledger {
            tx.insert(q);
        }
        for (old, new) in &counter_updates {
            tx.remove(old);
            tx.insert(new);
        }
        for r in &write.replace {
            let subject = NamedNode::new(&r.subject)?;
            let predicate = NamedNode::new(&r.predicate)?;
            let graph = GraphName::NamedNode(NamedNode::new(&r.graph)?);
            let old: Vec<Quad> = tx
                .quads_for_pattern(
                    Some(subject.as_ref().into()),
                    Some(predicate.as_ref()),
                    None,
                    Some(graph.as_ref()),
                )
                .collect::<std::result::Result<_, _>>()?;
            for q in &old {
                tx.remove(q);
            }
            tx.insert(&from_stored(&StoredQuad {
                s: format!("<{}>", r.subject),
                p: format!("<{}>", r.predicate),
                o: r.object.clone(),
                g: Some(r.graph.clone()),
            })?);
        }
        if let Some(rc) = &write.record_counts {
            let subject = NamedNode::new(&rc.subject)?;
            let graph = GraphName::NamedNode(NamedNode::new(&rc.graph)?);
            for (predicate, n) in [
                (&rc.inserted_new_predicate, inserted_new),
                (&rc.already_present_predicate, already_present),
            ] {
                tx.insert(&Quad::new(
                    subject.clone(),
                    NamedNode::new(predicate)?,
                    Literal::new_typed_literal(n.to_string(), long.clone()),
                    graph.clone(),
                ));
            }
        }
        tx.commit()?;
        Ok(Ok(BatchApplied {
            inserted_new,
            already_present,
            counters,
        }))
    }

    fn write_quads(&self, quads: &[StoredQuad]) -> Result<u64> {
        let quads: Vec<Quad> = quads.iter().map(from_stored).collect::<Result<_>>()?;
        self.write_quad_batch(&quads, &[])
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
        let quads = parse_quads(content, &opts)?;
        let inserted_new = self.write_quad_batch(&quads, extra)?;
        Ok((inserted_new, quads.iter().map(to_stored).collect()))
    }

    /// Write `quads` plus `extra` in one transaction; returns how many of
    /// `quads` were new. `Transaction::contains` sees the transaction's own
    /// writes, so a quad repeated in `quads` counts once.
    fn write_quad_batch(&self, quads: &[Quad], extra: &[Quad]) -> Result<u64> {
        let mut tx = self.store.start_transaction()?;
        let mut inserted_new = 0u64;
        for q in quads {
            if !tx.contains(q)? {
                inserted_new += 1;
            }
            tx.insert(q);
        }
        for q in extra {
            tx.insert(q);
        }
        tx.commit()?;
        Ok(inserted_new)
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

/// Parse `content` completely into quads. Blank nodes are renamed per call.
fn parse_quads(content: &[u8], opts: &LoadOpts) -> Result<Vec<Quad>> {
    let mut parser = RdfParser::from_format(to_oxigraph_format(opts.format)).rename_blank_nodes();
    if let Some(base) = &opts.base_iri {
        parser = parser.with_base_iri(base)?;
    }
    if let Some(iri) = &opts.graph_iri {
        parser = parser.with_default_graph(GraphName::NamedNode(NamedNode::new(iri)?));
    }
    Ok(parser
        .for_reader(content)
        .collect::<std::result::Result<_, _>>()?)
}

/// The inverse of [`to_stored`]: terms are in their N-Triples encoding.
fn from_stored(q: &StoredQuad) -> Result<Quad> {
    use std::str::FromStr;
    let subject = match Term::from_str(&q.s).with_context(|| format!("subject {}", q.s))? {
        Term::NamedNode(n) => oxigraph::model::NamedOrBlankNode::NamedNode(n),
        Term::BlankNode(b) => oxigraph::model::NamedOrBlankNode::BlankNode(b),
        other => anyhow::bail!("subject {other} is not an IRI or blank node"),
    };
    let predicate = NamedNode::from_str(&q.p).with_context(|| format!("predicate {}", q.p))?;
    let object = Term::from_str(&q.o).with_context(|| format!("object {}", q.o))?;
    let graph = match &q.g {
        Some(g) => GraphName::NamedNode(NamedNode::new(g)?),
        None => GraphName::DefaultGraph,
    };
    Ok(Quad::new(subject, predicate, object, graph))
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

    // ── write_batch: conditions and the write in one transaction ──────────────

    use crate::domain::{BatchRefusal, BatchWrite, Counter, Idempotency, RecordCounts};

    const JG: &str = "urn:test:jobs";
    const LONG: &str = "http://www.w3.org/2001/XMLSchema#long";

    fn q(s: &str, p: &str, o: &str, g: Option<&str>) -> StoredQuad {
        StoredQuad {
            s: format!("<{s}>"),
            p: format!("<{p}>"),
            o: if o.starts_with('"') {
                o.to_string()
            } else {
                format!("<{o}>")
            },
            g: g.map(str::to_string),
        }
    }

    fn long(n: u64) -> String {
        format!("\"{n}\"^^<{LONG}>")
    }

    fn present(a: &OxigraphAdapter, quad: &StoredQuad) -> bool {
        a.store.contains(&from_stored(quad).unwrap()).unwrap()
    }

    /// A batch of one data quad, guarded by an "open" requirement, an
    /// idempotency record and a counter with a ceiling of 10.
    fn batch(n: u32, add: u64) -> BatchWrite {
        let rec = format!("urn:test:rec{n}");
        BatchWrite {
            quads: vec![q(&format!("urn:d:{n}"), "urn:p", "urn:o", Some("urn:data"))],
            ledger: vec![q(&rec, "urn:sha", "\"abc\"", Some(JG))],
            require: vec![q("urn:job", "urn:state", "urn:open", Some(JG))],
            idempotency: Some(Idempotency {
                subject: rec.clone(),
                graph: JG.into(),
                identity: q(&rec, "urn:sha", "\"abc\"", Some(JG)),
            }),
            counters: vec![Counter {
                subject: "urn:job".into(),
                predicate: "urn:total".into(),
                graph: JG.into(),
                add,
                max: Some(10),
            }],
            record_counts: Some(RecordCounts {
                subject: rec,
                graph: JG.into(),
                inserted_new_predicate: "urn:new".into(),
                already_present_predicate: "urn:dup".into(),
            }),
            replace: vec![],
        }
    }

    fn open_job(a: &OxigraphAdapter) {
        a.write_quads(&[q("urn:job", "urn:state", "urn:open", Some(JG))])
            .unwrap();
    }

    #[test]
    fn a_batch_writes_data_ledger_counters_and_counts_together() {
        let a = adapter();
        open_job(&a);
        let applied = a.write_batch(&batch(1, 4)).unwrap().unwrap();
        assert_eq!((applied.inserted_new, applied.already_present), (1, 0));
        assert_eq!(applied.counters, vec![4]);
        assert!(present(
            &a,
            &q("urn:d:1", "urn:p", "urn:o", Some("urn:data"))
        ));
        assert!(present(
            &a,
            &q("urn:test:rec1", "urn:sha", "\"abc\"", Some(JG))
        ));
        assert!(present(&a, &q("urn:job", "urn:total", &long(4), Some(JG))));
        assert!(present(
            &a,
            &q("urn:test:rec1", "urn:new", &long(1), Some(JG))
        ));
        assert!(present(
            &a,
            &q("urn:test:rec1", "urn:dup", &long(0), Some(JG))
        ));

        // The counter is advanced, not duplicated: one quad, the new value.
        a.write_batch(&batch(2, 3)).unwrap().unwrap();
        assert!(present(&a, &q("urn:job", "urn:total", &long(7), Some(JG))));
        assert!(!present(&a, &q("urn:job", "urn:total", &long(4), Some(JG))));
    }

    #[test]
    fn a_requirement_that_is_not_met_writes_nothing() {
        let a = adapter(); // no open job
        let before = a.store.len().unwrap();
        let refused = a.write_batch(&batch(1, 1)).unwrap().unwrap_err();
        assert_eq!(refused, BatchRefusal::RequirementNotMet(0));
        assert_eq!(
            a.store.len().unwrap(),
            before,
            "a refused batch wrote something"
        );
    }

    #[test]
    fn the_same_batch_twice_is_a_duplicate_and_a_different_one_a_conflict() {
        let a = adapter();
        open_job(&a);
        a.write_batch(&batch(1, 1)).unwrap().unwrap();
        let len = a.store.len().unwrap();

        // Same record, same identity: the write was already accepted.
        assert_eq!(
            a.write_batch(&batch(1, 1)).unwrap().unwrap_err(),
            BatchRefusal::Duplicate
        );

        // Same record subject, a different identity: refused as a conflict.
        let mut other = batch(1, 1);
        other.idempotency.as_mut().unwrap().identity =
            q("urn:test:rec1", "urn:sha", "\"different\"", Some(JG));
        assert_eq!(
            a.write_batch(&other).unwrap().unwrap_err(),
            BatchRefusal::Conflict
        );

        assert_eq!(
            a.store.len().unwrap(),
            len,
            "a refused retry wrote something"
        );
        assert!(present(&a, &q("urn:job", "urn:total", &long(1), Some(JG))));
    }

    #[test]
    fn a_counter_ceiling_refuses_the_whole_batch() {
        let a = adapter();
        open_job(&a);
        a.write_batch(&batch(1, 8)).unwrap().unwrap();
        let len = a.store.len().unwrap();
        // 8 + 3 = 11 > 10: refused, and neither the data nor the counter moved.
        let refused = a.write_batch(&batch(2, 3)).unwrap().unwrap_err();
        assert_eq!(
            refused,
            BatchRefusal::CounterLimit {
                counter: 0,
                limit: 10,
                value: 11
            }
        );
        assert_eq!(a.store.len().unwrap(), len);
        assert!(!present(
            &a,
            &q("urn:d:2", "urn:p", "urn:o", Some("urn:data"))
        ));
        assert!(present(&a, &q("urn:job", "urn:total", &long(8), Some(JG))));
        // Exactly the ceiling is allowed.
        a.write_batch(&batch(3, 2)).unwrap().unwrap();
    }

    #[test]
    fn quads_already_in_the_store_are_counted_as_present() {
        let a = adapter();
        open_job(&a);
        a.write_quads(&[q("urn:d:1", "urn:p", "urn:o", Some("urn:data"))])
            .unwrap();
        let applied = a.write_batch(&batch(1, 1)).unwrap().unwrap();
        assert_eq!((applied.inserted_new, applied.already_present), (0, 1));
        assert!(present(
            &a,
            &q("urn:test:rec1", "urn:dup", &long(1), Some(JG))
        ));
    }

    #[test]
    fn raw_parsing_keeps_labels_and_a_batch_is_deterministic() {
        let a = adapter();
        let nt = b"_:a <urn:p> _:b .\n_:a <urn:q> \"x\" .\n";
        let first = a.parse_rdf_raw(nt, InputFormat::NTriples).unwrap();
        let second = a.parse_rdf_raw(nt, InputFormat::NTriples).unwrap();
        assert_eq!(
            first, second,
            "the same N-Triples must parse to the same quads"
        );
        assert_eq!(first[0].s, "_:a");
        assert_eq!(first[0].o, "_:b");

        // What a Turtle `[]` looks like: the skolemization relies on it.
        let ttl = b"<urn:s> <urn:p> [ <urn:q> <urn:o> ] .";
        let quads = a.parse_rdf_raw(ttl, InputFormat::Turtle).unwrap();
        let anon = quads.iter().find(|q| q.o.starts_with("_:")).unwrap();
        println!("generated blank node: {}", anon.o);
        let id = anon.o.trim_start_matches("_:");
        assert!(
            id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
            "unexpected generated blank node shape: {id}"
        );
    }
}
