//! Per-project detail: identity, ontology usage (with inheritance), and quality
//! metrics. Pure read-only collectors over a `SparqlStore`.
//!
//! The project graph IRI is validated before interpolation, so no
//! attacker-controlled text reaches SPARQL.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};

use crate::domain::{QueryResult, RdfTerm, SparqlStore};

const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";

/// Everything the detail screen needs for one project.
pub struct ProjectDetail {
    pub id: String,
    pub label: String,
    pub description: String,
    pub classes: Vec<OntologyClassUse>,
    pub metrics: Vec<Metric>,
}

/// A class instantiated in the project graph, with ontology metadata.
pub struct OntologyClassUse {
    pub iri: String,
    pub label: String,
    pub comment: String,
    pub instances: u64,
    pub super_classes: Vec<String>,
}

/// A single quality metric with human explanation + interpretation.
pub struct Metric {
    pub name: &'static str,
    pub value: String,
    pub explanation: &'static str,
    pub interpretation: String,
}

/// Raw integer counts behind the metrics — exposed for precise testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMetrics {
    pub triples: u64,
    pub nodes: u64,
    pub classes: u64,
    pub predicates: u64,
    pub subjects: u64,
    pub typed_subjects: u64,
    pub literal_objects: u64,
}

pub fn collect_project_detail(
    store: &dyn SparqlStore,
    id: &str,
    label: &str,
    description: &str,
    graph_iri: &str,
) -> Result<ProjectDetail> {
    let raw = collect_raw_metrics(store, graph_iri)?;
    Ok(ProjectDetail {
        id: id.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        classes: collect_classes(store, graph_iri)?,
        metrics: build_metrics(&raw),
    })
}

pub fn collect_raw_metrics(store: &dyn SparqlStore, graph_iri: &str) -> Result<RawMetrics> {
    validate_graph_iri(graph_iri)?;
    let g = graph_iri;
    Ok(RawMetrics {
        triples: scalar(store, &format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"))?,
        nodes: scalar(
            store,
            &format!(
                "SELECT (COUNT(DISTINCT ?n) AS ?c) WHERE {{ GRAPH <{g}> \
                 {{ {{ ?n ?p ?o }} UNION {{ ?s ?p ?n }} }} FILTER(isIri(?n)) }}"
            ),
        )?,
        classes: scalar(store, &format!("SELECT (COUNT(DISTINCT ?c2) AS ?c) WHERE {{ GRAPH <{g}> {{ ?s a ?c2 }} }}"))?,
        predicates: scalar(store, &format!("SELECT (COUNT(DISTINCT ?p) AS ?c) WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"))?,
        subjects: scalar(store, &format!("SELECT (COUNT(DISTINCT ?s) AS ?c) WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} }}"))?,
        typed_subjects: scalar(store, &format!("SELECT (COUNT(DISTINCT ?s) AS ?c) WHERE {{ GRAPH <{g}> {{ ?s a ?t }} }}"))?,
        literal_objects: scalar(
            store,
            &format!("SELECT (COUNT(?o) AS ?c) WHERE {{ GRAPH <{g}> {{ ?s ?p ?o FILTER(isLiteral(?o)) }} }}"),
        )?,
    })
}

pub fn build_metrics(r: &RawMetrics) -> Vec<Metric> {
    let density = ratio(r.triples, r.nodes);
    let typed = pct(r.typed_subjects, r.subjects);
    let literal = pct(r.literal_objects, r.triples);
    vec![
        Metric {
            name: "Triples",
            value: r.triples.to_string(),
            explanation: "Total RDF statements in the project graph.",
            interpretation: "Raw size of the knowledge graph.".into(),
        },
        Metric {
            name: "Nodes",
            value: r.nodes.to_string(),
            explanation: "Distinct IRIs used as subject or object.",
            interpretation: "Number of real entities/things modeled.".into(),
        },
        Metric {
            name: "Classes",
            value: r.classes.to_string(),
            explanation: "Distinct rdf:type classes instantiated.",
            interpretation: "Schema breadth — how many kinds of things.".into(),
        },
        Metric {
            name: "Predicates",
            value: r.predicates.to_string(),
            explanation: "Distinct properties used.",
            interpretation: "Vocabulary richness of relations/attributes.".into(),
        },
        Metric {
            name: "Density",
            value: format!("{density:.2}"),
            explanation: "Average statements per node (triples / nodes).",
            interpretation: bucket(
                density,
                &[
                    (1.5, "sparse — list-like, lightly described"),
                    (3.0, "moderate — typical description depth"),
                ],
                "rich — entities are densely described",
            ),
        },
        Metric {
            name: "Typed ratio",
            value: format!("{typed:.0}%"),
            explanation: "Share of subjects carrying an rdf:type.",
            interpretation: bucket(
                typed,
                &[
                    (50.0, "low — many ad-hoc/untyped nodes"),
                    (90.0, "partial — some nodes lack a type"),
                ],
                "high — almost everything is typed",
            ),
        },
        Metric {
            name: "Literal ratio",
            value: format!("{literal:.0}%"),
            explanation: "Share of object positions that are literals (vs IRIs).",
            interpretation: bucket(
                literal,
                &[
                    (30.0, "link-heavy — mostly a graph of relations"),
                    (70.0, "balanced — links and attributes"),
                ],
                "attribute-heavy — mostly literal data",
            ),
        },
    ]
}

pub fn collect_classes(store: &dyn SparqlStore, graph_iri: &str) -> Result<Vec<OntologyClassUse>> {
    validate_graph_iri(graph_iri)?;
    let g = graph_iri;

    // used classes + instance counts
    let used = solutions(
        store,
        &format!("SELECT ?c (COUNT(?s) AS ?n) WHERE {{ GRAPH <{g}> {{ ?s a ?c }} }} GROUP BY ?c ORDER BY DESC(?n)"),
    )?;

    // inheritance + labels/comments, from the project graph UNION the default graph
    let supers = pairs(
        store,
        &format!(
            "SELECT ?c ?v WHERE {{ {{ GRAPH <{g}> {{ ?c <{RDFS}subClassOf> ?v }} }} \
             UNION {{ ?c <{RDFS}subClassOf> ?v }} }}"
        ),
    )?;
    let labels = first_value(
        store,
        &format!(
            "SELECT ?c ?v WHERE {{ {{ GRAPH <{g}> {{ ?c <{RDFS}label> ?v }} }} \
             UNION {{ ?c <{RDFS}label> ?v }} }}"
        ),
    )?;
    let comments = first_value(
        store,
        &format!(
            "SELECT ?c ?v WHERE {{ {{ GRAPH <{g}> {{ ?c <{RDFS}comment> ?v }} }} \
             UNION {{ ?c <{RDFS}comment> ?v }} }}"
        ),
    )?;

    Ok(used
        .into_iter()
        .filter_map(|row| {
            let iri = iri_cell(&row, "c")?;
            let instances = row
                .get("n")
                .and_then(|t| t.as_value_str().parse::<u64>().ok())
                .unwrap_or(0);
            Some(OntologyClassUse {
                label: labels.get(&iri).cloned().unwrap_or_default(),
                comment: comments.get(&iri).cloned().unwrap_or_default(),
                super_classes: supers.get(&iri).cloned().unwrap_or_default(),
                iri,
                instances,
            })
        })
        .collect())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn validate_graph_iri(iri: &str) -> Result<()> {
    if iri.is_empty()
        || iri.chars().any(|c| {
            c.is_whitespace() || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '\\' | '^' | '`')
        })
    {
        bail!("unsafe graph IRI: {iri:?}");
    }
    Ok(())
}

fn solutions(store: &dyn SparqlStore, sparql: &str) -> Result<Vec<HashMap<String, RdfTerm>>> {
    let QueryResult::Solutions(sol) = store.query(sparql)? else {
        bail!("expected SELECT solutions");
    };
    Ok(sol.rows)
}

fn scalar(store: &dyn SparqlStore, sparql: &str) -> Result<u64> {
    let rows = solutions(store, sparql)?;
    let row = rows.first().context("count query returned no rows")?;
    let term = row.get("c").context("count cell missing")?;
    term.as_value_str()
        .parse::<u64>()
        .with_context(|| format!("count not an integer: {}", term.as_value_str()))
}

/// `?c ?v` → multimap of c-IRI to list of v-IRIs (deduplicated, order-stable).
fn pairs(store: &dyn SparqlStore, sparql: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for row in solutions(store, sparql)? {
        if let (Some(c), Some(v)) = (iri_cell(&row, "c"), iri_cell(&row, "v")) {
            let entry = map.entry(c).or_default();
            if !entry.contains(&v) {
                entry.push(v);
            }
        }
    }
    Ok(map)
}

/// `?c ?v` → first value (string) seen per c.
fn first_value(store: &dyn SparqlStore, sparql: &str) -> Result<HashMap<String, String>> {
    let mut map: HashMap<String, String> = HashMap::new();
    for row in solutions(store, sparql)? {
        if let Some(c) = iri_cell(&row, "c") {
            if let Some(v) = row.get("v") {
                map.entry(c).or_insert_with(|| v.as_value_str().to_string());
            }
        }
    }
    Ok(map)
}

fn iri_cell(row: &HashMap<String, RdfTerm>, var: &str) -> Option<String> {
    match row.get(var) {
        Some(RdfTerm::Iri(s)) => Some(s.clone()),
        _ => None,
    }
}

fn ratio(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

fn pct(num: u64, den: u64) -> f64 {
    ratio(num, den) * 100.0
}

/// Pick an interpretation phrase by ascending threshold; `high` is the fallback.
fn bucket(value: f64, thresholds: &[(f64, &str)], high: &str) -> String {
    for (limit, phrase) in thresholds {
        if value < *limit {
            return (*phrase).to_string();
        }
    }
    high.to_string()
}
