//! KB archive (the "knowledge-base container"): export -> import round-trip.

use std::sync::Arc;

use sparql_mcp::application::archive::{
    export_archive, import_archive, list_archives, read_manifest,
};
use sparql_mcp::domain::{LoadOpts, SparqlStore};
use sparql_mcp::infrastructure::OxigraphAdapter;
use tempfile::TempDir;

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
    load(
        "@prefix smc: <https://sparql-mcp.dev/ns#> .\n<urn:project:m> a smc:Project ; smc:projectId \"m\" ; smc:label \"M\" .",
        "urn:meta",
    );
    load(
        "@prefix ex: <http://e/> .\nex:a ex:p ex:b . ex:b ex:p ex:c .",
        "urn:project:m",
    );
    s
}

#[test]
fn export_then_import_round_trips() {
    let dir = TempDir::new().unwrap();
    let src = seeded();
    let before = src.triple_count().unwrap();

    let zip = dir.path().join("kb-latest.zip");
    let info = export_archive(src.as_ref(), &zip, Some("nightly")).unwrap();
    assert!(zip.exists());
    assert!(info.graphs >= 2, "exported {} graphs", info.graphs);

    // manifest carries the tag and the graph list
    let m = read_manifest(&zip).unwrap();
    assert_eq!(m.tag.as_deref(), Some("nightly"));
    assert!(m.graphs.iter().any(|g| g.iri == "urn:project:m"));

    // fresh store, import, counts match
    let dst: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
    let report = import_archive(dst.as_ref(), &zip).unwrap();
    assert_eq!(report.graphs, info.graphs);
    assert_eq!(dst.triple_count().unwrap(), before);
}

#[test]
fn list_archives_reports_zips_in_dir() {
    let dir = TempDir::new().unwrap();
    let src = seeded();
    export_archive(src.as_ref(), &dir.path().join("latest.zip"), None).unwrap();
    export_archive(
        src.as_ref(),
        &dir.path().join("kb-tag-20260525.zip"),
        Some("tag"),
    )
    .unwrap();
    let mut entries = list_archives(dir.path()).unwrap();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| e.tag.as_deref() == Some("tag")));
}
