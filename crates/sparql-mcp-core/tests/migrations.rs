//! Integration tests — schema migration runner (application::migrations).

use std::sync::Arc;

use sparql_mcp::application::migrations::{
    apply, current_version, embedded, pending, validate, Migration,
};
use sparql_mcp::domain::{QueryResult, SparqlStore};
use sparql_mcp::infrastructure::OxigraphAdapter;

fn store() -> Arc<dyn SparqlStore> {
    Arc::new(OxigraphAdapter::open_in_memory().unwrap())
}

const M1: Migration = Migration {
    version: 1,
    name: "insert_marker",
    sparql: "INSERT DATA { GRAPH <urn:meta> { <urn:x:a> <urn:x:p> \"1\" } }",
};
const M2: Migration = Migration {
    version: 2,
    name: "insert_marker2",
    sparql: "INSERT DATA { GRAPH <urn:meta> { <urn:x:b> <urn:x:p> \"2\" } }",
};

fn ask(s: &dyn SparqlStore, q: &str) -> bool {
    matches!(s.query(q).unwrap(), QueryResult::Boolean(true))
}

#[test]
fn validate_rejects_non_contiguous_and_duplicates() {
    assert!(validate(&[M1, M2]).is_ok());
    let gap = [
        M1,
        Migration {
            version: 3,
            ..M2
        },
    ];
    assert!(validate(&gap).is_err(), "gap 1->3 must be rejected");
    let dup = [M1, Migration { name: "x", ..M1 }];
    assert!(validate(&dup).is_err(), "duplicate version must be rejected");
}

#[test]
fn fresh_store_is_version_zero() {
    let s = store();
    assert_eq!(current_version(s.as_ref()).unwrap(), 0);
    let p = pending(s.as_ref(), &[M1, M2]).unwrap();
    assert_eq!(p.len(), 2);
}

#[test]
fn apply_runs_all_then_is_idempotent() {
    let s = store();
    let applied = apply(s.as_ref(), &[M1, M2]).unwrap();
    assert_eq!(applied, vec![1, 2]);
    assert_eq!(current_version(s.as_ref()).unwrap(), 2);
    // the migration effects landed
    assert!(ask(s.as_ref(), "ASK { GRAPH <urn:meta> { <urn:x:a> ?p ?o } }"));
    assert!(ask(s.as_ref(), "ASK { GRAPH <urn:meta> { <urn:x:b> ?p ?o } }"));
    // second run applies nothing
    let again = apply(s.as_ref(), &[M1, M2]).unwrap();
    assert!(again.is_empty(), "expected no pending on second apply");
    assert_eq!(pending(s.as_ref(), &[M1, M2]).unwrap().len(), 0);
}

#[test]
fn checksum_drift_is_detected() {
    let s = store();
    apply(s.as_ref(), &[M1]).unwrap();
    // tamper the recorded checksum of migration 1
    s.update(
        "PREFIX smc: <https://sparql-mcp.dev/ns#>\n\
         DELETE { GRAPH <urn:meta> { <urn:meta:migration:0001> smc:checksum ?c } }\n\
         INSERT { GRAPH <urn:meta> { <urn:meta:migration:0001> smc:checksum \"sha256:deadbeef\" } }\n\
         WHERE  { GRAPH <urn:meta> { <urn:meta:migration:0001> smc:checksum ?c } }",
    )
    .unwrap();
    // re-applying the same set must now refuse due to drift
    assert!(apply(s.as_ref(), &[M1]).is_err(), "tampered checksum must abort");
}

#[test]
fn embedded_set_is_valid() {
    // the migrations shipped in the binary must always validate
    validate(embedded()).unwrap();
    assert!(!embedded().is_empty());
}
