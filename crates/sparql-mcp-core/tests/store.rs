//! Integration tests — OxigraphAdapter (SparqlStore port).
//!
//! All tests use an in-memory store: fast, isolated, no disk state.

use std::sync::Arc;

use sparql_mcp::domain::{InputFormat, LoadOpts, QueryResult, SparqlStore};
use sparql_mcp::infrastructure::OxigraphAdapter;
use tempfile::TempDir;

fn store() -> Arc<dyn SparqlStore> {
    Arc::new(OxigraphAdapter::open_in_memory().unwrap())
}

const TURTLE_FOO: &str = r#"
@prefix ex: <http://example.org/> .
ex:Alice a ex:Person ; ex:name "Alice" .
ex:Bob   a ex:Person ; ex:name "Bob" .
"#;

// ── load_rdf ─────────────────────────────────────────────────────────────────

#[test]
fn load_turtle_default_graph_adds_triples() {
    let s = store();
    let n = s
        .load_rdf(TURTLE_FOO.as_bytes(), LoadOpts::default())
        .unwrap();
    assert!(n >= 4, "expected ≥4 triples, got {n}");
    assert_eq!(s.triple_count().unwrap(), n);
}

#[test]
fn load_turtle_into_named_graph() {
    let s = store();
    let opts = LoadOpts {
        graph_iri: Some("urn:test:g1".into()),
        ..Default::default()
    };
    let n = s.load_rdf(TURTLE_FOO.as_bytes(), opts).unwrap();
    assert!(n >= 4);
    // Named graph appears in list
    let graphs = s.list_graphs().unwrap();
    assert!(
        graphs.iter().any(|g| g == "urn:test:g1"),
        "graph not listed: {graphs:?}"
    );
}

#[test]
fn load_ntriples() {
    let nt = "<http://a.org/s> <http://a.org/p> <http://a.org/o> .\n";
    let opts = LoadOpts {
        format: InputFormat::NTriples,
        ..Default::default()
    };
    let n = store().load_rdf(nt.as_bytes(), opts).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn load_rdf_invalid_turtle_returns_error() {
    let bad = b"this is not turtle @@@@";
    let result = store().load_rdf(bad, LoadOpts::default());
    assert!(result.is_err(), "expected parse error");
}

// ── query ─────────────────────────────────────────────────────────────────────

#[test]
fn select_on_empty_store_returns_empty_solutions() {
    let s = store();
    let r = s.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o }").unwrap();
    let QueryResult::Solutions(sol) = r else {
        panic!("expected Solutions")
    };
    assert!(sol.rows.is_empty());
}

#[test]
fn select_finds_loaded_data() {
    let s = store();
    s.load_rdf(TURTLE_FOO.as_bytes(), LoadOpts::default())
        .unwrap();
    let r = s
        .query("PREFIX ex: <http://example.org/> SELECT ?n WHERE { ?x a ex:Person ; ex:name ?n } ORDER BY ?n")
        .unwrap();
    let QueryResult::Solutions(sol) = r else {
        panic!("expected Solutions")
    };
    assert_eq!(sol.rows.len(), 2);
    let names: Vec<String> = sol
        .rows
        .iter()
        .map(|row| row["n"].as_value_str().to_string())
        .collect();
    assert_eq!(names, ["Alice", "Bob"]);
}

#[test]
fn ask_true_and_false() {
    let s = store();
    s.load_rdf(TURTLE_FOO.as_bytes(), LoadOpts::default())
        .unwrap();
    let t = s
        .query("PREFIX ex: <http://example.org/> ASK { ex:Alice a ex:Person }")
        .unwrap();
    assert!(matches!(t, QueryResult::Boolean(true)));

    let f = s
        .query("PREFIX ex: <http://example.org/> ASK { ex:Alice a ex:Robot }")
        .unwrap();
    assert!(matches!(f, QueryResult::Boolean(false)));
}

#[test]
fn select_scoped_to_named_graph() {
    let s = store();
    let in_graph = LoadOpts {
        graph_iri: Some("urn:g:a".into()),
        ..Default::default()
    };
    s.load_rdf(TURTLE_FOO.as_bytes(), in_graph).unwrap();

    // Query against graph a — finds data
    let r = s
        .query("SELECT ?x WHERE { GRAPH <urn:g:a> { ?x a <http://example.org/Person> } }")
        .unwrap();
    let QueryResult::Solutions(sol) = r else {
        panic!()
    };
    assert_eq!(sol.rows.len(), 2);

    // Query against a different graph — no results
    let r = s
        .query("SELECT ?x WHERE { GRAPH <urn:g:b> { ?x a <http://example.org/Person> } }")
        .unwrap();
    let QueryResult::Solutions(sol) = r else {
        panic!()
    };
    assert!(sol.rows.is_empty());
}

// ── update ────────────────────────────────────────────────────────────────────

#[test]
fn insert_data_then_select() {
    let s = store();
    s.update("INSERT DATA { <urn:s> <urn:p> <urn:o> . }")
        .unwrap();
    let r = s.query("SELECT ?o WHERE { <urn:s> <urn:p> ?o }").unwrap();
    let QueryResult::Solutions(sol) = r else {
        panic!()
    };
    assert_eq!(sol.rows.len(), 1);
}

#[test]
fn delete_removes_triple() {
    let s = store();
    s.update("INSERT DATA { <urn:s> <urn:p> <urn:o> . }")
        .unwrap();
    s.update("DELETE DATA { <urn:s> <urn:p> <urn:o> . }")
        .unwrap();
    assert_eq!(s.triple_count().unwrap(), 0);
}

#[test]
fn update_invalid_sparql_returns_error() {
    let result = store().update("NOT VALID SPARQL @@");
    assert!(result.is_err());
}

// ── list_graphs / triple_count ────────────────────────────────────────────────

#[test]
fn list_graphs_empty_initially() {
    assert!(store().list_graphs().unwrap().is_empty());
}

#[test]
fn triple_count_aggregates_all_graphs() {
    let s = store();
    s.load_rdf(
        TURTLE_FOO.as_bytes(),
        LoadOpts {
            graph_iri: Some("urn:g1".into()),
            ..Default::default()
        },
    )
    .unwrap();
    s.load_rdf(
        TURTLE_FOO.as_bytes(),
        LoadOpts {
            graph_iri: Some("urn:g2".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let count = s.triple_count().unwrap();
    assert!(
        count >= 8,
        "expected ≥8 triples across two graphs, got {count}"
    );
}

// ── load_rdf_file / SHA-256 idempotence ───────────────────────────────────────

#[test]
fn load_rdf_file_idempotent() {
    use std::io::Write;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("data.ttl");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(TURTLE_FOO.as_bytes())
        .unwrap();

    let s = store();
    let opts = LoadOpts {
        graph_iri: Some("urn:g".into()),
        ..Default::default()
    };
    let r1 = s.load_rdf_file(&path, opts.clone(), false).unwrap();
    let n1 = r1.triples_after - r1.triples_before;
    assert!(n1 > 0);

    // Second load — same hash → skip
    let r2 = s.load_rdf_file(&path, opts, false).unwrap();
    assert_eq!(
        r2.triples_after - r2.triples_before,
        0,
        "expected 0 triples on idempotent re-load"
    );
    assert!(r2.skipped, "expected skipped=true on re-load");
}

#[test]
fn load_rdf_file_force_reloads() {
    use std::io::Write;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("data.ttl");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(TURTLE_FOO.as_bytes())
        .unwrap();

    let s = store();
    let opts = LoadOpts {
        graph_iri: Some("urn:g".into()),
        ..Default::default()
    };
    s.load_rdf_file(&path, opts.clone(), false).unwrap();
    let r2 = s.load_rdf_file(&path, opts, true).unwrap();
    assert!(!r2.skipped, "force=true should re-load");
    // triples are deduplicated — delta can be 0; what matters is skipped=false
}

// ── export_graph ──────────────────────────────────────────────────────────────

#[test]
fn export_graph_produces_valid_turtle() {
    let dir = TempDir::new().unwrap();
    let s = store();
    let opts = LoadOpts {
        graph_iri: Some("urn:export-test".into()),
        ..Default::default()
    };
    s.load_rdf(TURTLE_FOO.as_bytes(), opts).unwrap();

    let out = dir.path().join("export.ttl");
    let n = s.export_graph(Some("urn:export-test"), &out).unwrap();
    assert!(n > 0);
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains("Alice") || content.contains("example.org"));
}
