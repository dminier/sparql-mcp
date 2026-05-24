//! CLI integration tests — exercise the actual binary for the `migrate` and
//! `stats` subcommands (dispatch + read-only store open) against a temp store.

use std::process::Command;

use tempfile::TempDir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sparql-mcp"))
}

fn run(args: &[&str]) -> (String, String, bool) {
    let out = bin().args(args).output().expect("spawn binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn migrate_applies_then_is_idempotent_then_stats() {
    let dir = TempDir::new().unwrap();
    let store = dir.path().join("store");
    let store = store.to_str().unwrap();

    // Fresh store: status reports version 0 with a pending migration.
    let (so, _se, ok) = run(&["--store", store, "migrate", "--status"]);
    assert!(ok, "migrate --status should succeed");
    assert!(so.contains("schema version: 0"), "got: {so}");
    assert!(so.contains("pending: 0001"), "got: {so}");

    // Dry-run does not change the version.
    let (so, _se, ok) = run(&["--store", store, "migrate", "--dry-run"]);
    assert!(ok);
    assert!(so.contains("would apply: 0001"), "got: {so}");

    // Apply.
    let (so, _se, ok) = run(&["--store", store, "migrate"]);
    assert!(ok);
    assert!(so.contains("applied: [1]"), "got: {so}");

    // Idempotent.
    let (so, _se, ok) = run(&["--store", store, "migrate"]);
    assert!(ok);
    assert!(so.contains("up to date"), "got: {so}");

    // stats opens the same store (read-only path) and prints a triple count.
    let (so, _se, ok) = run(&["--store", store, "stats"]);
    assert!(ok, "stats should succeed");
    assert!(so.starts_with("triples:"), "got: {so}");
}

#[test]
fn kb_export_list_import_round_trip() {
    let dir = TempDir::new().unwrap();
    let store_a = dir.path().join("a");
    let backups = dir.path().join("backups");
    let ttl = dir.path().join("seed.ttl");
    std::fs::write(
        &ttl,
        "@prefix ex: <http://e/> .\nex:a ex:p ex:b . ex:b ex:p ex:c .\n",
    )
    .unwrap();

    // seed store A
    let (_o, _e, ok) = run(&[
        "--store",
        store_a.to_str().unwrap(),
        "load-file",
        "--path",
        ttl.to_str().unwrap(),
        "--graph",
        "urn:project:m",
    ]);
    assert!(ok);

    // export to a tagged archive under `backups`
    let zip = backups.join("kb.zip");
    let (so, _e, ok) = run(&[
        "--store",
        store_a.to_str().unwrap(),
        "kb-export",
        "--tag",
        "snap",
        "--out",
        zip.to_str().unwrap(),
    ]);
    assert!(ok, "kb-export failed: {so}");
    assert!(zip.exists());

    // kb-list (uses SPARQL_MCP_HOME -> backups dir) shows the archive
    let out = bin()
        .env("SPARQL_MCP_HOME", dir.path())
        .args(["kb-list"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(listing.contains("snap"), "kb-list: {listing}");

    // import into a fresh store B
    let store_b = dir.path().join("b");
    let (so, _e, ok) = run(&[
        "--store",
        store_b.to_str().unwrap(),
        "kb-import",
        "--path",
        zip.to_str().unwrap(),
    ]);
    assert!(ok, "kb-import failed: {so}");
    assert!(so.contains("imported"), "got: {so}");

    // store B now has the project graph triples
    let (so, _e, ok) = run(&["--store", store_b.to_str().unwrap(), "stats"]);
    assert!(ok);
    assert!(so.starts_with("triples:"), "got: {so}");
}
