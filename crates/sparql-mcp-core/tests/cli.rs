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
