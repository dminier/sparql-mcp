//! Integration tests — stats collectors (application::stats).
//!
//! In-memory store seeded with two projects in <urn:meta> and two project
//! graphs with known triples, so every count is deterministic.

use std::sync::Arc;

use sparql_mcp::application::stats::{collect_project_stats, collect_store_stats};
use sparql_mcp::domain::{LoadOpts, SparqlStore};
use sparql_mcp::infrastructure::OxigraphAdapter;

const META: &str = r#"
@prefix smc: <https://sparql-mcp.dev/ns#> .
<urn:project:alpha> a smc:Project ;
    smc:projectId "alpha" ; smc:label "Alpha" ;
    smc:namedGraph <urn:project:alpha> ; smc:description "First project" .
<urn:project:beta> a smc:Project ;
    smc:projectId "beta" ; smc:label "Beta" ;
    smc:namedGraph <urn:project:beta> .
"#;

// 2 triples; IRI nodes (subject|object): a1, a2, a3 = 3
const ALPHA: &str = r#"
@prefix ex: <http://example.org/> .
ex:a1 ex:rel ex:a2 .
ex:a2 ex:rel ex:a3 .
"#;

// 1 triple; IRI nodes: b1 = 1 (object is a literal, filtered out)
const BETA: &str = r#"
@prefix ex: <http://example.org/> .
ex:b1 ex:rel "literal" .
"#;

fn seeded() -> Arc<dyn SparqlStore> {
    let s: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let load = |ttl: &str, g: &str| {
        s.load_rdf(
            ttl.as_bytes(),
            LoadOpts {
                graph_iri: Some(g.into()),
                ..Default::default()
            },
        )
        .unwrap();
    };
    load(META, "urn:meta");
    load(ALPHA, "urn:project:alpha");
    load(BETA, "urn:project:beta");
    s
}

#[test]
fn store_stats_are_exact() {
    let s = seeded();
    let st = collect_store_stats(s.as_ref()).unwrap();
    // meta: 9 triples, alpha: 2, beta: 1
    assert_eq!(st.triples, 12, "triples");
    assert_eq!(st.graphs, 3, "named graphs");
    // distinct IRIs as subject|object across all graphs:
    // alpha/beta project IRIs, smc:Project, ex:a1, ex:a2, ex:a3, ex:b1 = 7
    assert_eq!(st.nodes, 7, "distinct IRI nodes");
}

#[test]
fn project_with_declared_but_empty_graph_reports_zero() {
    let s: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    s.load_rdf(
        b"@prefix smc: <https://sparql-mcp.dev/ns#> .\n\
          <urn:project:gamma> a smc:Project ; smc:projectId \"gamma\" ; smc:label \"Gamma\" ;\n\
            smc:namedGraph <urn:project:gamma> .",
        LoadOpts {
            graph_iri: Some("urn:meta".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let ps = collect_project_stats(s.as_ref()).unwrap();
    assert_eq!(ps.len(), 1);
    assert_eq!(ps[0].id, "gamma");
    assert_eq!(ps[0].triples, 0);
    assert_eq!(ps[0].nodes, 0);
}

#[test]
fn project_stats_sorted_with_counts_and_description_fallback() {
    let s = seeded();
    let ps = collect_project_stats(s.as_ref()).unwrap();
    assert_eq!(ps.len(), 2);

    assert_eq!(ps[0].id, "alpha");
    assert_eq!(ps[0].label, "Alpha");
    assert_eq!(ps[0].description, "First project");
    assert_eq!(ps[0].triples, 2);
    assert_eq!(ps[0].nodes, 3);

    assert_eq!(ps[1].id, "beta");
    assert_eq!(ps[1].label, "Beta");
    // no smc:description → falls back to label
    assert_eq!(ps[1].description, "Beta");
    assert_eq!(ps[1].triples, 1);
    assert_eq!(ps[1].nodes, 1);
}
