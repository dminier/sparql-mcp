//! Forward-only schema/ontology migrations.
//!
//! Migrations are SPARQL Update text, **embedded in the binary** and applied in
//! version order. The applied version + a per-migration record (checksum,
//! timestamp) live in `<urn:meta>`. Migrations transform *structure only* — they
//! never carry personal data (see root `CLAUDE.md` §"Data separation").

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::domain::{QueryResult, SparqlStore};

const SMC_NS: &str = "https://sparql-mcp.dev/ns#";
const META_GRAPH: &str = "urn:meta";
const STORE_IRI: &str = "urn:meta:store";

/// A single schema migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sparql: &'static str,
}

/// Migrations shipped in this binary, in version order.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "backfill_project_description",
    sparql: include_str!("../../migrations/0001_backfill_project_description.ru"),
}];

/// The embedded migration set.
pub fn embedded() -> &'static [Migration] {
    MIGRATIONS
}

/// Validate that versions start at 1, are contiguous, and unique.
pub fn validate(migs: &[Migration]) -> Result<()> {
    for (i, m) in migs.iter().enumerate() {
        let expected = i as u32 + 1;
        if m.version != expected {
            bail!(
                "migration versions must be contiguous from 1: expected {expected}, got {} ({})",
                m.version,
                m.name
            );
        }
    }
    Ok(())
}

/// Schema version recorded in the store (0 when unset).
pub fn current_version(store: &dyn SparqlStore) -> Result<u32> {
    let q = format!(
        "PREFIX smc: <{SMC_NS}>\n\
         SELECT ?v WHERE {{ GRAPH <{META_GRAPH}> {{ <{STORE_IRI}> smc:schemaVersion ?v }} }}"
    );
    let QueryResult::Solutions(sol) = store.query(&q)? else {
        bail!("expected SELECT solutions for schema version");
    };
    match sol.rows.first() {
        None => Ok(0),
        Some(row) => {
            let var = sol.variables.first().context("no variable in version query")?;
            let term = row.get(var).context("version cell missing")?;
            term.as_value_str()
                .parse::<u32>()
                .with_context(|| format!("schema version not an integer: {}", term.as_value_str()))
        }
    }
}

/// Migrations with version greater than the current store version.
pub fn pending<'a>(store: &dyn SparqlStore, migs: &'a [Migration]) -> Result<Vec<&'a Migration>> {
    let cur = current_version(store)?;
    Ok(migs.iter().filter(|m| m.version > cur).collect())
}

/// Apply all pending migrations in order. Returns the versions applied.
///
/// Before applying, every already-applied migration's recorded checksum is
/// verified against the embedded text — a mismatch (drift/tampering) aborts.
pub fn apply(store: &dyn SparqlStore, migs: &[Migration]) -> Result<Vec<u32>> {
    validate(migs)?;
    verify_applied(store, migs)?;

    let mut applied = Vec::new();
    for m in pending(store, migs)? {
        store
            .update(m.sparql)
            .with_context(|| format!("running migration {:04} ({})", m.version, m.name))?;
        record_applied(store, m)?;
        set_version(store, m.version)?;
        applied.push(m.version);
    }
    Ok(applied)
}

fn checksum(sparql: &str) -> String {
    let mut h = Sha256::new();
    h.update(sparql.as_bytes());
    format!("sha256:{:x}", h.finalize())
}

fn verify_applied(store: &dyn SparqlStore, migs: &[Migration]) -> Result<()> {
    let cur = current_version(store)?;
    for m in migs.iter().filter(|m| m.version <= cur) {
        let recorded = recorded_checksum(store, m.version)?;
        let expected = checksum(m.sparql);
        match recorded {
            Some(c) if c == expected => {}
            Some(c) => bail!(
                "migration {:04} ({}) checksum drift: recorded {c}, embedded {expected}",
                m.version,
                m.name
            ),
            None => bail!(
                "migration {:04} ({}) marked applied (version {cur}) but no checksum recorded",
                m.version,
                m.name
            ),
        }
    }
    Ok(())
}

fn recorded_checksum(store: &dyn SparqlStore, version: u32) -> Result<Option<String>> {
    let q = format!(
        "PREFIX smc: <{SMC_NS}>\n\
         SELECT ?c WHERE {{ GRAPH <{META_GRAPH}> {{ <urn:meta:migration:{version:04}> smc:checksum ?c }} }}"
    );
    let QueryResult::Solutions(sol) = store.query(&q)? else {
        bail!("expected SELECT solutions for checksum");
    };
    Ok(sol.rows.first().and_then(|row| {
        sol.variables
            .first()
            .and_then(|v| row.get(v))
            .map(|t| t.as_value_str().to_string())
    }))
}

fn record_applied(store: &dyn SparqlStore, m: &Migration) -> Result<()> {
    let iri = format!("urn:meta:migration:{:04}", m.version);
    let sum = checksum(m.sparql);
    let now = now_iso();
    let update = format!(
        "PREFIX smc: <{SMC_NS}>\n\
         PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
         INSERT DATA {{ GRAPH <{META_GRAPH}> {{\n\
             <{iri}> a smc:Migration ;\n\
                 smc:version {ver} ;\n\
                 smc:name \"{name}\" ;\n\
                 smc:checksum \"{sum}\" ;\n\
                 smc:appliedAt \"{now}\"^^xsd:dateTime .\n\
         }} }}",
        ver = m.version,
        name = m.name,
    );
    store.update(&update).context("recording migration")
}

fn set_version(store: &dyn SparqlStore, version: u32) -> Result<()> {
    let update = format!(
        "PREFIX smc: <{SMC_NS}>\n\
         DELETE {{ GRAPH <{META_GRAPH}> {{ <{STORE_IRI}> smc:schemaVersion ?v }} }}\n\
         INSERT {{ GRAPH <{META_GRAPH}> {{ <{STORE_IRI}> a smc:Store ; smc:schemaVersion {version} }} }}\n\
         WHERE  {{ OPTIONAL {{ GRAPH <{META_GRAPH}> {{ <{STORE_IRI}> smc:schemaVersion ?v }} }} }}"
    );
    store.update(&update).context("bumping schema version")
}

// Minimal UTC ISO-8601 timestamp (xsd:dateTime), no external time crate.
fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (sec, min, hour) = (secs % 60, (secs / 60) % 60, (secs / 3600) % 24);
    let days = secs / 86400;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let mut y = yoe + era * 400;
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}
