//! Knowledge-base archive — a portable, shareable "container" for the store.
//!
//! An archive is a zip holding `manifest.json` plus one Turtle file per named
//! graph under `graphs/`. It can be exported, shared, and re-imported into any
//! store, so a whole KB travels as a single file.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::domain::{InputFormat, LoadOpts, SparqlStore};

const MANIFEST: &str = "manifest.json";
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEntry {
    pub iri: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub tool_version: String,
    pub created: String,
    pub tag: Option<String>,
    pub graphs: Vec<GraphEntry>,
}

pub struct ArchiveInfo {
    pub path: PathBuf,
    pub graphs: usize,
    pub bytes: u64,
}

pub struct ImportReport {
    pub graphs: usize,
    pub triples: u64,
}

pub struct ArchiveEntry {
    pub path: PathBuf,
    pub tag: Option<String>,
    pub created: String,
    pub graphs: usize,
}

/// Export every named graph in `store` into a zip archive at `out`.
pub fn export_archive(
    store: &dyn SparqlStore,
    out: &Path,
    tag: Option<&str>,
) -> Result<ArchiveInfo> {
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let graphs = store.list_graphs()?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp =
        std::env::temp_dir().join(format!("sparql-mcp-export-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&tmp)?;

    let file = File::create(out).with_context(|| format!("creating archive {}", out.display()))?;
    let mut zip = ZipWriter::new(file);
    let opts: SimpleFileOptions =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut entries = Vec::with_capacity(graphs.len());
    for (i, g) in graphs.iter().enumerate() {
        let fname = format!("graphs/g{i:04}.ttl");
        let ttl_path = tmp.join(format!("g{i:04}.ttl"));
        store.export_graph(Some(g), &ttl_path)?;
        let bytes = fs::read(&ttl_path)?;
        zip.start_file(&fname, opts)?;
        zip.write_all(&bytes)?;
        entries.push(GraphEntry {
            iri: g.clone(),
            file: fname,
        });
    }

    let manifest = Manifest {
        format: "sparql-mcp-kb/1".to_string(),
        tool_version: TOOL_VERSION.to_string(),
        created: now_iso(),
        tag: tag.map(str::to_string),
        graphs: entries,
    };
    zip.start_file(MANIFEST, opts)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    zip.finish()?;
    let _ = fs::remove_dir_all(&tmp);

    let graphs = manifest.graphs.len();
    let bytes = fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    Ok(ArchiveInfo {
        path: out.to_path_buf(),
        graphs,
        bytes,
    })
}

/// Import an archive back into `store`, loading each graph from its Turtle file.
pub fn import_archive(store: &dyn SparqlStore, zip_path: &Path) -> Result<ImportReport> {
    let manifest = read_manifest(zip_path)?;
    let mut archive = open_zip(zip_path)?;
    let mut triples = 0u64;
    for g in &manifest.graphs {
        let mut buf = Vec::new();
        archive
            .by_name(&g.file)
            .with_context(|| format!("missing {} in archive", g.file))?
            .read_to_end(&mut buf)?;
        triples += store.load_rdf(
            &buf,
            LoadOpts {
                format: InputFormat::Turtle,
                graph_iri: Some(g.iri.clone()),
                base_iri: None,
            },
        )?;
    }
    Ok(ImportReport {
        graphs: manifest.graphs.len(),
        triples,
    })
}

/// Parse just the manifest from an archive without loading any data.
pub fn read_manifest(zip_path: &Path) -> Result<Manifest> {
    let mut archive = open_zip(zip_path)?;
    let mut s = String::new();
    archive
        .by_name(MANIFEST)
        .with_context(|| format!("{} has no {MANIFEST}", zip_path.display()))?
        .read_to_string(&mut s)?;
    serde_json::from_str(&s).context("parsing manifest")
}

/// List KB archives (`*.zip` with a readable manifest) in `dir`.
pub fn list_archives(dir: &Path) -> Result<Vec<ArchiveEntry>> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(out), // no backups dir yet
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("zip") {
            continue;
        }
        if let Ok(m) = read_manifest(&path) {
            out.push(ArchiveEntry {
                path,
                tag: m.tag,
                created: m.created,
                graphs: m.graphs.len(),
            });
        }
    }
    Ok(out)
}

/// Default archive path: `latest.zip` for the daily snapshot, or
/// `kb-<tag>-<YYYYMMDDHHMMSS>.zip` for a tagged version.
pub fn default_path(dir: &Path, tag: Option<&str>) -> PathBuf {
    match tag {
        None => dir.join("latest.zip"),
        Some(t) => {
            let stamp: String = now_iso().chars().filter(|c| c.is_ascii_digit()).collect();
            let safe: String = t
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            dir.join(format!("kb-{safe}-{stamp}.zip"))
        }
    }
}

fn open_zip(zip_path: &Path) -> Result<ZipArchive<File>> {
    let file = File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    ZipArchive::new(file).with_context(|| format!("reading zip {}", zip_path.display()))
}

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
