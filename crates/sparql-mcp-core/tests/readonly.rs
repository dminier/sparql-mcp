//! Read-only store open — resilience for the TUI viewer while a writer holds
//! the RocksDB lock. Here we assert the deterministic part: a read-only handle
//! reads data written by a prior read-write handle.

use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::OxigraphAdapter;
use tempfile::TempDir;

#[test]
fn open_read_only_reads_existing_store() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("store");
    {
        let rw = OxigraphAdapter::open(&path).unwrap();
        rw.update("INSERT DATA { GRAPH <urn:g> { <urn:a> <urn:p> <urn:b> } }")
            .unwrap();
    } // drop closes the writer and releases the lock

    let ro = OxigraphAdapter::open_read_only(&path).unwrap();
    assert!(
        ro.triple_count().unwrap() >= 1,
        "read-only must see the data"
    );
    assert!(ro.list_graphs().unwrap().iter().any(|g| g == "urn:g"));
}
