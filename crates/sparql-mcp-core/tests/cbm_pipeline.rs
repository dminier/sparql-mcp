//! Integration tests — full CBM import pipeline.
//!
//! Builds an in-memory SQLite with the CBM schema, inserts sample nodes and
//! edges, runs the full pipeline:
//!   cbm_db::load_graph → cbm_turtle::graph_to_turtle → store.load_rdf
//! then verifies the results with SPARQL SELECT.

use std::sync::Arc;

use rusqlite::{params, Connection};
use sparql_mcp::cbm::{db as cbm_db, turtle as cbm_turtle};
use sparql_mcp::domain::{LoadOpts, QueryResult, SparqlStore};
use sparql_mcp::infrastructure::OxigraphAdapter;
use tempfile::NamedTempFile;

// ── SQLite fixture ────────────────────────────────────────────────────────────

fn create_cbm_schema(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE projects (
            name TEXT PRIMARY KEY,
            indexed_at TEXT NOT NULL,
            root_path TEXT NOT NULL
        );
        CREATE TABLE nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project TEXT NOT NULL,
            label TEXT NOT NULL,
            name TEXT NOT NULL,
            qualified_name TEXT NOT NULL,
            file_path TEXT DEFAULT '',
            start_line INTEGER DEFAULT 0,
            end_line INTEGER DEFAULT 0,
            properties TEXT DEFAULT '{}'
        );
        CREATE TABLE edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project TEXT NOT NULL,
            source_id INTEGER NOT NULL,
            target_id INTEGER NOT NULL,
            type TEXT NOT NULL,
            properties TEXT DEFAULT '{}'
        );",
    )
    .unwrap();
}

/// Write an in-memory SQLite to a temp file and return the path.
/// (cbm_db::load_graph takes a &Path, not a Connection.)
fn cbm_fixture() -> NamedTempFile {
    let conn = Connection::open_in_memory().unwrap();
    create_cbm_schema(&conn);

    conn.execute(
        "INSERT INTO projects (name, indexed_at, root_path) VALUES (?1, ?2, ?3)",
        params!["test-repo", "2024-01-01T00:00:00Z", "/tmp/test-repo"],
    )
    .unwrap();

    // Nodes
    let node_ids: Vec<i64> = [
        (
            "Function",
            "main",
            "test_repo.main",
            "src/main.rs",
            1,
            20,
            r#"{"complexity":5,"lines":20,"is_exported":true,"is_entry_point":true}"#,
        ),
        (
            "Function",
            "helper",
            "test_repo.helper",
            "src/main.rs",
            22,
            35,
            r#"{"complexity":2,"lines":14,"is_exported":false}"#,
        ),
        (
            "Class",
            "Config",
            "test_repo.Config",
            "src/config.rs",
            1,
            50,
            r#"{"lines":50,"is_exported":true}"#,
        ),
        (
            "Method",
            "load",
            "test_repo.Config.load",
            "src/config.rs",
            10,
            30,
            r#"{"complexity":3,"lines":21}"#,
        ),
        ("File", "main.rs", "src/main.rs", "src/main.rs", 0, 0, "{}"),
    ]
    .iter()
    .map(|(label, name, qn, fp, sl, el, props)| {
        conn.execute(
            "INSERT INTO nodes (project, label, name, qualified_name, file_path, \
             start_line, end_line, properties) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params!["test-repo", label, name, qn, fp, sl, el, props],
        )
        .unwrap();
        conn.last_insert_rowid()
    })
    .collect();

    // Edges: main CALLS helper, Config DEFINES_METHOD load, File CONTAINS main
    for (src, tgt, etype) in [
        (node_ids[0], node_ids[1], "CALLS"),
        (node_ids[2], node_ids[3], "DEFINES_METHOD"),
        (node_ids[4], node_ids[0], "CONTAINS"),
    ] {
        conn.execute(
            "INSERT INTO edges (project, source_id, target_id, type) VALUES (?1,?2,?3,?4)",
            params!["test-repo", src, tgt, etype],
        )
        .unwrap();
    }

    // Persist to temp file so cbm_db::load_graph can open it by path
    let tmp = NamedTempFile::new().unwrap();
    {
        // Use SQLite backup API via rusqlite
        let mut dest = Connection::open(tmp.path()).unwrap();
        let backup = rusqlite::backup::Backup::new(&conn, &mut dest).unwrap();
        backup
            .run_to_completion(5, std::time::Duration::ZERO, None)
            .unwrap();
    }
    tmp
}

// ── pipeline tests ────────────────────────────────────────────────────────────

#[test]
fn load_graph_reads_nodes_and_edges() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    assert_eq!(kg.project.name, "test-repo");
    assert_eq!(kg.nodes.len(), 5);
    assert_eq!(kg.edges.len(), 3);
}

#[test]
fn load_graph_auto_detects_single_project() {
    let tmp = cbm_fixture();
    // project_name = None → auto-detect
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    assert_eq!(kg.project.name, "test-repo");
}

#[test]
fn load_graph_explicit_project_name() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), Some("test-repo")).unwrap();
    assert_eq!(kg.nodes.len(), 5);
}

#[test]
fn load_graph_unknown_project_returns_error() {
    let tmp = cbm_fixture();
    let result = cbm_db::load_graph(tmp.path(), Some("no-such-project"));
    assert!(result.is_err());
}

#[test]
fn list_projects_returns_project_names() {
    let tmp = cbm_fixture();
    let names = cbm_db::list_projects(tmp.path()).unwrap();
    assert_eq!(names, vec!["test-repo"]);
}

#[test]
fn graph_to_turtle_contains_expected_triples() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    let ttl = cbm_turtle::graph_to_turtle(&kg);

    // Classes are emitted
    assert!(
        ttl.contains("a cbm:Function"),
        "missing Function class: {ttl}"
    );
    assert!(ttl.contains("a cbm:Class"), "missing Class: {ttl}");
    assert!(ttl.contains("a cbm:Method"), "missing Method: {ttl}");

    // Properties
    assert!(ttl.contains("cbm:name"), "missing name prop");
    assert!(ttl.contains("cbm:qualifiedName"), "missing qualifiedName");
    assert!(ttl.contains("cbm:isExported"), "missing isExported");
    assert!(ttl.contains("cbm:isEntryPoint"), "missing isEntryPoint");
    assert!(ttl.contains("cbm:complexity"), "missing complexity");

    // Edge predicates
    assert!(ttl.contains("cbm:calls"), "missing calls edge");
    assert!(
        ttl.contains("cbm:definesMethod"),
        "missing definesMethod edge"
    );
    assert!(ttl.contains("cbm:contains"), "missing contains edge");
}

#[test]
fn full_pipeline_into_oxigraph() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    let ttl = cbm_turtle::graph_to_turtle(&kg);

    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let graph_iri = "urn:project:test-repo";
    let opts = LoadOpts {
        graph_iri: Some(graph_iri.into()),
        ..Default::default()
    };
    let n = store.load_rdf(ttl.as_bytes(), opts).unwrap();
    assert!(n > 0, "no triples imported");

    // Named graph appears
    let graphs = store.list_graphs().unwrap();
    assert!(graphs.iter().any(|g| g == graph_iri), "{graphs:?}");
}

#[test]
fn sparql_query_finds_functions_after_import() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    let ttl = cbm_turtle::graph_to_turtle(&kg);

    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    store
        .load_rdf(
            ttl.as_bytes(),
            LoadOpts {
                graph_iri: Some("urn:project:test-repo".into()),
                ..Default::default()
            },
        )
        .unwrap();

    let r = store
        .query(
            "PREFIX cbm: <http://codebase-memory.dev/ontology#>
             SELECT ?name WHERE {
               GRAPH <urn:project:test-repo> {
                 ?f a cbm:Function ; cbm:name ?name .
               }
             } ORDER BY ?name",
        )
        .unwrap();

    let QueryResult::Solutions(sol) = r else {
        panic!("expected Solutions")
    };
    let names: Vec<String> = sol
        .rows
        .iter()
        .map(|row| row["name"].as_value_str().to_string())
        .collect();
    assert_eq!(names, vec!["helper", "main"]);
}

#[test]
fn sparql_query_follows_calls_edge() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    let ttl = cbm_turtle::graph_to_turtle(&kg);

    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    store
        .load_rdf(
            ttl.as_bytes(),
            LoadOpts {
                graph_iri: Some("urn:project:test-repo".into()),
                ..Default::default()
            },
        )
        .unwrap();

    // main CALLS helper — query the edge
    let r = store
        .query(
            "PREFIX cbm: <http://codebase-memory.dev/ontology#>
             SELECT ?caller ?callee WHERE {
               GRAPH <urn:project:test-repo> {
                 ?caller cbm:calls ?callee ;
                         cbm:name ?cn .
                 ?callee cbm:name ?en .
                 FILTER(?cn = \"main\")
               }
             }",
        )
        .unwrap();

    let QueryResult::Solutions(sol) = r else {
        panic!()
    };
    assert_eq!(
        sol.rows.len(),
        1,
        "expected exactly one CALLS edge from main"
    );
}

#[test]
fn node_properties_preserved_in_sparql() {
    let tmp = cbm_fixture();
    let kg = cbm_db::load_graph(tmp.path(), None).unwrap();
    let ttl = cbm_turtle::graph_to_turtle(&kg);

    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    store
        .load_rdf(
            ttl.as_bytes(),
            LoadOpts {
                graph_iri: Some("urn:project:test-repo".into()),
                ..Default::default()
            },
        )
        .unwrap();

    let r = store
        .query(
            "PREFIX cbm: <http://codebase-memory.dev/ontology#>
             SELECT ?complexity WHERE {
               GRAPH <urn:project:test-repo> {
                 ?f cbm:name \"main\" ; cbm:complexity ?complexity .
               }
             }",
        )
        .unwrap();
    let QueryResult::Solutions(sol) = r else {
        panic!()
    };
    assert_eq!(sol.rows.len(), 1);
    assert_eq!(sol.rows[0]["complexity"].as_value_str(), "5");
}
