//! Store and per-project statistics for the TUI viewer.
//!
//! Pure read-only collectors over a `SparqlStore`. No infrastructure imports.
//! Per-project counts use grouped (`GROUP BY ?g`) queries joined in Rust — one
//! query each for triples and nodes regardless of project count, and no project
//! graph IRI is ever interpolated into SPARQL.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::domain::{QueryResult, RdfTerm, SparqlStore};

const SMC_NS: &str = "https://sparql-mcp.dev/ns#";
const META_GRAPH: &str = "urn:meta";

const TRIPLES_BY_GRAPH: &str =
    "SELECT ?g (COUNT(*) AS ?c) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g";
const NODES_BY_GRAPH: &str = "SELECT ?g (COUNT(DISTINCT ?n) AS ?c) WHERE { \
     GRAPH ?g { { ?n ?p ?o } UNION { ?s ?p ?n } } FILTER(isIri(?n)) } GROUP BY ?g";

/// Global store statistics shown in the TUI header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStats {
    pub triples: u64,
    pub graphs: u64,
    /// Distinct IRIs appearing as subject or object in any named graph.
    pub nodes: u64,
}

/// Per-project summary row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectStat {
    pub id: String,
    pub label: String,
    /// `smc:description`, falling back to the label when absent.
    pub description: String,
    pub graph_iri: String,
    pub triples: u64,
    pub nodes: u64,
}

pub fn collect_store_stats(store: &dyn SparqlStore) -> Result<StoreStats> {
    let triples = store.triple_count()?;
    let graphs = store.list_graphs()?.len() as u64;
    let nodes = count_scalar(
        store,
        "SELECT (COUNT(DISTINCT ?n) AS ?c) WHERE { \
         GRAPH ?g { { ?n ?p ?o } UNION { ?s ?p ?n } } FILTER(isIri(?n)) }",
    )?;
    Ok(StoreStats {
        triples,
        graphs,
        nodes,
    })
}

pub fn collect_project_stats(store: &dyn SparqlStore) -> Result<Vec<ProjectStat>> {
    let projects = list_projects(store)?;
    let triples_by_graph = counts_by_graph(store, TRIPLES_BY_GRAPH)?;
    let nodes_by_graph = counts_by_graph(store, NODES_BY_GRAPH)?;

    Ok(projects
        .into_iter()
        .map(|p| ProjectStat {
            triples: triples_by_graph.get(&p.graph_iri).copied().unwrap_or(0),
            nodes: nodes_by_graph.get(&p.graph_iri).copied().unwrap_or(0),
            id: p.id,
            label: p.label,
            description: p.description,
            graph_iri: p.graph_iri,
        })
        .collect())
}

struct ProjectRow {
    id: String,
    label: String,
    description: String,
    graph_iri: String,
}

fn list_projects(store: &dyn SparqlStore) -> Result<Vec<ProjectRow>> {
    let sparql = format!(
        "PREFIX smc: <{SMC_NS}>\n\
         SELECT ?id ?label ?graph ?desc WHERE {{\n\
             GRAPH <{META_GRAPH}> {{\n\
                 ?p a smc:Project ; smc:projectId ?id ; smc:label ?label .\n\
                 OPTIONAL {{ ?p smc:namedGraph ?graph }}\n\
                 OPTIONAL {{ ?p smc:description ?desc }}\n\
             }}\n\
         }} ORDER BY ?id"
    );
    let QueryResult::Solutions(sol) = store.query(&sparql)? else {
        anyhow::bail!("expected SELECT solutions for project list");
    };
    Ok(sol
        .rows
        .iter()
        .map(|row| {
            let id = str_cell(row, "id");
            let label = str_cell(row, "label");
            let graph_iri = iri_cell(row, "graph").unwrap_or_else(|| format!("urn:project:{id}"));
            let description = match row.get("desc") {
                Some(t) if !t.as_value_str().is_empty() => t.as_value_str().to_string(),
                _ => label.clone(),
            };
            ProjectRow {
                id,
                label,
                description,
                graph_iri,
            }
        })
        .collect())
}

/// Run a grouped `SELECT ?g (COUNT(..) AS ?c) ... GROUP BY ?g` and collect a
/// graph-IRI -> count map.
fn counts_by_graph(store: &dyn SparqlStore, sparql: &str) -> Result<HashMap<String, u64>> {
    let QueryResult::Solutions(sol) = store.query(sparql)? else {
        anyhow::bail!("expected SELECT solutions for grouped count");
    };
    let mut map = HashMap::with_capacity(sol.rows.len());
    for row in &sol.rows {
        let Some(graph) = iri_cell(row, "g") else {
            continue;
        };
        let count = row
            .get("c")
            .and_then(|t| t.as_value_str().parse::<u64>().ok())
            .unwrap_or(0);
        map.insert(graph, count);
    }
    Ok(map)
}

fn count_scalar(store: &dyn SparqlStore, sparql: &str) -> Result<u64> {
    let QueryResult::Solutions(sol) = store.query(sparql)? else {
        anyhow::bail!("expected SELECT solutions for count query");
    };
    let row = sol.rows.first().context("count query returned no rows")?;
    let var = sol
        .variables
        .first()
        .context("count query has no variables")?;
    let term = row.get(var).context("count cell missing")?;
    term.as_value_str()
        .parse::<u64>()
        .with_context(|| format!("count value not an integer: {}", term.as_value_str()))
}

fn str_cell(row: &HashMap<String, RdfTerm>, var: &str) -> String {
    row.get(var)
        .map(|t| t.as_value_str().to_string())
        .unwrap_or_default()
}

fn iri_cell(row: &HashMap<String, RdfTerm>, var: &str) -> Option<String> {
    row.get(var).and_then(|t| match t {
        RdfTerm::Iri(s) => Some(s.clone()),
        _ => None,
    })
}
