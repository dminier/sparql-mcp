//! Project detail collectors (application::detail).

use std::sync::Arc;

use sparql_mcp::application::detail::{
    build_metrics, collect_classes, collect_raw_metrics, RawMetrics,
};
use sparql_mcp::domain::{LoadOpts, SparqlStore};
use sparql_mcp::infrastructure::OxigraphAdapter;

const ONTOLOGY: &str = r#"
@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:Animal rdfs:label "Animal" .
ex:Dog rdfs:subClassOf ex:Animal ; rdfs:label "Dog" ; rdfs:comment "A domestic dog" .
"#;

// 4 triples; IRI nodes: rex, fido, Dog = 3; subjects rex,fido; typed both; 1 literal
const PROJECT: &str = r#"
@prefix ex: <http://example.org/> .
ex:rex a ex:Dog ; ex:name "Rex" ; ex:friend ex:fido .
ex:fido a ex:Dog .
"#;

fn seeded() -> Arc<dyn SparqlStore> {
    let s: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    // ontology into the default graph
    s.load_rdf(ONTOLOGY.as_bytes(), LoadOpts::default())
        .unwrap();
    s.load_rdf(
        PROJECT.as_bytes(),
        LoadOpts {
            graph_iri: Some("urn:project:t".into()),
            ..Default::default()
        },
    )
    .unwrap();
    s
}

#[test]
fn raw_metrics_are_exact() {
    let s = seeded();
    let m = collect_raw_metrics(s.as_ref(), "urn:project:t").unwrap();
    assert_eq!(
        m,
        RawMetrics {
            triples: 4,
            nodes: 3,
            classes: 1,
            predicates: 3,
            subjects: 2,
            typed_subjects: 2,
            literal_objects: 1,
        }
    );
}

#[test]
fn build_metrics_all_have_explanation_and_interpretation() {
    let raw = RawMetrics {
        triples: 4,
        nodes: 3,
        classes: 1,
        predicates: 3,
        subjects: 2,
        typed_subjects: 2,
        literal_objects: 1,
    };
    let ms = build_metrics(&raw);
    assert_eq!(ms.len(), 7);
    for m in &ms {
        assert!(!m.value.is_empty(), "{} value", m.name);
        assert!(!m.explanation.is_empty(), "{} explanation", m.name);
        assert!(!m.interpretation.is_empty(), "{} interpretation", m.name);
    }
    // typed ratio is 100% here
    let typed = ms.iter().find(|m| m.name == "Typed ratio").unwrap();
    assert!(typed.value.contains("100"));
}

#[test]
fn classes_resolve_instances_and_inheritance() {
    let s = seeded();
    let classes = collect_classes(s.as_ref(), "urn:project:t").unwrap();
    assert_eq!(classes.len(), 1);
    let dog = &classes[0];
    assert_eq!(dog.iri, "http://example.org/Dog");
    assert_eq!(dog.label, "Dog");
    assert_eq!(dog.comment, "A domestic dog");
    assert_eq!(dog.instances, 2);
    assert_eq!(
        dog.super_classes,
        vec!["http://example.org/Animal".to_string()]
    );
}

#[test]
fn rejects_unsafe_graph_iri() {
    let s = seeded();
    assert!(collect_raw_metrics(s.as_ref(), "urn:bad> }").is_err());
}
