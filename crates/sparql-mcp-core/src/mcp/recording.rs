//! Build Turtle payloads from a Playwright recording session directory
//! (`output/recordings/<ts>/`).
//!
//! Each kind of stream (navigations, network) is loaded into its own named
//! graph so that re-imports are idempotent via `clear_graph`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

const NS: &str = "https://sparql-mcp.dev/ns/rec#";

pub struct MaterializeStats {
    pub files_written: usize,
    pub overwrites: usize,
    pub skipped_no_host: usize,
    pub missing_body: usize,
    pub out_dir: PathBuf,
}

pub struct BodyTarget {
    pub url: String,
    pub body_path: PathBuf,
    pub content_type: Option<String>,
}

/// Given a list of (url, body_path, content_type) tuples, mirror the bodies
/// into `out_dir/<host>/<url_path>`. Collisions are resolved by overwrite
/// (last body wins). Hardlinks are used when possible, with a fallback to copy.
pub fn materialize(out_dir: &Path, targets: &[BodyTarget]) -> Result<MaterializeStats> {
    let mut stats = MaterializeStats {
        files_written: 0,
        overwrites: 0,
        skipped_no_host: 0,
        missing_body: 0,
        out_dir: out_dir.to_path_buf(),
    };
    for t in targets {
        if !t.body_path.exists() {
            stats.missing_body += 1;
            continue;
        }
        let Some((host, rel)) = url_to_rel(&t.url, t.content_type.as_deref()) else {
            stats.skipped_no_host += 1;
            continue;
        };
        let mut dest = out_dir.join(host).join(&rel);
        // If an ancestor exists as a regular file, convert it to a directory
        // holding its prior content as `index.<ext>`.
        if let Some(parent) = dest.parent() {
            promote_file_ancestors_to_dirs(parent, t.content_type.as_deref())?;
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        // Conversely, if the destination itself already exists as a directory,
        // drop the body inside it as `index.<ext>`.
        if dest.is_dir() {
            dest = dest.join(format!(
                "index.{}",
                ext_for_content_type(t.content_type.as_deref())
            ));
        }
        let already = dest.exists();
        if already {
            fs::remove_file(&dest).ok();
        }
        if fs::hard_link(&t.body_path, &dest).is_err() {
            fs::copy(&t.body_path, &dest).with_context(|| {
                format!("copying {} -> {}", t.body_path.display(), dest.display())
            })?;
        }
        stats.files_written += 1;
        if already {
            stats.overwrites += 1;
        }
    }
    Ok(stats)
}

fn ext_for_content_type(ct: Option<&str>) -> &'static str {
    match ct
        .and_then(|s| s.split([';', ' ']).next())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("application/json") => "json",
        Some("application/javascript") | Some("text/javascript") => "js",
        Some("text/css") => "css",
        Some("text/plain") => "txt",
        Some("application/xml") | Some("text/xml") => "xml",
        Some("image/svg+xml") => "svg",
        _ => "html",
    }
}

/// Walk ancestors of `dir`; whenever one exists as a regular file, replace it
/// with a directory of the same name containing the original content as
/// `index.<ext>`. Silently succeeds if the path already is a directory.
fn promote_file_ancestors_to_dirs(dir: &Path, hint_ct: Option<&str>) -> Result<()> {
    let mut ancestors: Vec<&Path> = dir.ancestors().collect();
    ancestors.reverse();
    for a in ancestors {
        if a.as_os_str().is_empty() {
            continue;
        }
        let meta = match a.symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_file() {
            let tmp = a.with_extension("__hkb_tmp__");
            fs::rename(a, &tmp).with_context(|| format!("renaming {} aside", a.display()))?;
            fs::create_dir_all(a).with_context(|| format!("creating dir {}", a.display()))?;
            let inside = a.join(format!("index.{}", ext_for_content_type(hint_ct)));
            fs::rename(&tmp, &inside)
                .with_context(|| format!("moving previous content into {}", inside.display()))?;
        }
    }
    Ok(())
}

/// Parse an absolute http(s) URL into `(host, sanitized_relative_path)`.
/// Returns `None` if the URL has no host.
fn url_to_rel(url: &str, content_type: Option<&str>) -> Option<(String, PathBuf)> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    let tail = &after_scheme[authority_end..];
    let host = authority.split('@').next_back().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        return None;
    }

    let path_only = tail
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    let ends_with_slash = tail
        .split(['?', '#'])
        .next()
        .map(|p| p.is_empty() || p.ends_with('/'))
        .unwrap_or(true);

    let mut rel = PathBuf::new();
    for seg in path_only.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            continue;
        }
        rel.push(seg);
    }
    if rel.as_os_str().is_empty() || ends_with_slash {
        let ext = match content_type
            .and_then(|s| s.split([';', ' ']).next())
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("text/html") | Some("application/xhtml+xml") | None => "html",
            Some("application/json") => "json",
            Some("text/plain") => "txt",
            _ => "html",
        };
        rel.push(format!("index.{ext}"));
    }
    Some((host.to_string(), rel))
}

pub struct SessionPaths {
    pub dir: PathBuf,
    pub session_id: String,
    pub session_iri: String,
}

pub struct KindGraph {
    pub graph_iri: String,
    pub turtle: String,
    pub count: usize,
}

pub fn resolve_session(session_dir: &str) -> Result<SessionPaths> {
    let dir = fs::canonicalize(session_dir)
        .with_context(|| format!("resolving session_dir {session_dir}"))?;
    let session_id = dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("session_dir has no final component"))?
        .to_string();
    let session_iri = format!("urn:hkb:rec:{session_id}");
    Ok(SessionPaths {
        dir,
        session_id,
        session_iri,
    })
}

/// Emit triples describing the session itself (from `session.json`).
pub fn build_session_turtle(sp: &SessionPaths) -> Result<String> {
    let path = sp.dir.join("session.json");
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let v: Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

    let mut out = prelude();
    out.push_str(&format!("<{}> a hkb-rec:RecordingSession", sp.session_iri));
    if let Some(s) = v.get("started_at").and_then(Value::as_str) {
        if let Some(iso) = normalize_session_started_at(s) {
            out.push_str(&format!(
                " ;\n    hkb-rec:startedAt \"{iso}\"^^xsd:dateTime"
            ));
        }
    }
    if let Some(s) = v.get("ended_at").and_then(Value::as_str) {
        out.push_str(&format!(" ;\n    hkb-rec:endedAt \"{s}\"^^xsd:dateTime"));
    }
    if let Some(s) = v.get("seed").and_then(Value::as_str) {
        out.push_str(&format!(
            " ;\n    hkb-rec:seedUrl \"{}\"^^xsd:anyURI",
            esc(s)
        ));
    }
    if let Some(s) = v.get("user_agent").and_then(Value::as_str) {
        out.push_str(&format!(" ;\n    hkb-rec:userAgent {}", lit(s)));
    }
    out.push_str(&format!(
        " ;\n    hkb-rec:sessionDir {} .\n",
        lit(&sp.dir.to_string_lossy())
    ));
    Ok(out)
}

pub fn build_navigations(sp: &SessionPaths) -> Result<KindGraph> {
    let lines = read_jsonl(&sp.dir.join("navigations.jsonl"))?;
    let mut ttl = prelude();
    for (i, v) in lines.iter().enumerate() {
        let nav_iri = format!("{}/nav/{:04}", sp.session_iri, i + 1);
        ttl.push_str(&format!(
            "<{}> hkb-rec:hasNavigation <{nav_iri}> .\n",
            sp.session_iri
        ));
        ttl.push_str(&format!("<{nav_iri}> a hkb-rec:Navigation"));
        ttl.push_str(&format!(" ;\n    hkb-rec:order {}", i + 1));
        if let Some(s) = v.get("ts").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:ts \"{s}\"^^xsd:dateTime"));
        }
        if let Some(s) = v.get("url").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:url \"{}\"^^xsd:anyURI", esc(s)));
        }
        if let Some(s) = v.get("screenshot").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:screenshot {}", lit(s)));
        }
        ttl.push_str(" .\n");
    }
    Ok(KindGraph {
        graph_iri: format!("{}/g/navigations", sp.session_iri),
        turtle: ttl,
        count: lines.len(),
    })
}

pub fn build_network(sp: &SessionPaths) -> Result<KindGraph> {
    let lines = read_jsonl(&sp.dir.join("network.jsonl"))?;
    let bodies_dir = sp.dir.join("bodies");
    let mut bodies_dir_created = false;
    let mut ttl = prelude();
    for (i, v) in lines.iter().enumerate() {
        let order = i + 1;
        let ev_iri = format!("{}/net/{:05}", sp.session_iri, order);
        let content_type = response_header(v, "content-type");
        ttl.push_str(&format!(
            "<{}> hkb-rec:hasNetworkEvent <{ev_iri}> .\n",
            sp.session_iri
        ));
        ttl.push_str(&format!("<{ev_iri}> a hkb-rec:NetworkEvent"));
        ttl.push_str(&format!(" ;\n    hkb-rec:order {}", i + 1));
        if let Some(s) = v.get("ts").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:ts \"{s}\"^^xsd:dateTime"));
        }
        if let Some(s) = v.get("method").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:method {}", lit(s)));
        }
        if let Some(s) = v.get("url").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:url \"{}\"^^xsd:anyURI", esc(s)));
        }
        if let Some(n) = v.get("status").and_then(Value::as_i64) {
            ttl.push_str(&format!(" ;\n    hkb-rec:status {n}"));
        }
        if let Some(s) = v.get("status_text").and_then(Value::as_str) {
            if !s.is_empty() {
                ttl.push_str(&format!(" ;\n    hkb-rec:statusText {}", lit(s)));
            }
        }
        if let Some(s) = v.get("resource_type").and_then(Value::as_str) {
            ttl.push_str(&format!(" ;\n    hkb-rec:resourceType {}", lit(s)));
        }
        if let Some(n) = v.get("elapsed_ms").and_then(Value::as_i64) {
            ttl.push_str(&format!(" ;\n    hkb-rec:elapsedMs {n}"));
        }
        if let Some(n) = v.get("response_body_bytes").and_then(Value::as_i64) {
            ttl.push_str(&format!(" ;\n    hkb-rec:responseBodyBytes {n}"));
        }
        if let Some(b) = v.get("response_body_truncated").and_then(Value::as_bool) {
            ttl.push_str(&format!(" ;\n    hkb-rec:responseBodyTruncated {b}"));
        }
        if let Some(ct) = &content_type {
            ttl.push_str(&format!(" ;\n    hkb-rec:contentType {}", lit(ct)));
        }
        if let Some(body) = v.get("response_body").and_then(Value::as_str) {
            if !bodies_dir_created {
                fs::create_dir_all(&bodies_dir)
                    .with_context(|| format!("creating {}", bodies_dir.display()))?;
                bodies_dir_created = true;
            }
            let ext = pick_extension(
                content_type.as_deref(),
                v.get("resource_type").and_then(Value::as_str),
                v.get("url").and_then(Value::as_str),
            );
            let filename = format!("{order:05}.{ext}");
            let path = bodies_dir.join(&filename);
            fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
            ttl.push_str(&format!(
                " ;\n    hkb-rec:responseBodyPath {}",
                lit(&path.to_string_lossy())
            ));
        }
        ttl.push_str(" .\n");
    }
    Ok(KindGraph {
        graph_iri: format!("{}/g/network", sp.session_iri),
        turtle: ttl,
        count: lines.len(),
    })
}

fn response_header(v: &Value, name: &str) -> Option<String> {
    v.get("response_headers")
        .and_then(Value::as_object)
        .and_then(|m| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)))
        .and_then(|(_, val)| val.as_str())
        .map(|s| s.to_string())
}

fn pick_extension(
    content_type: Option<&str>,
    resource_type: Option<&str>,
    url: Option<&str>,
) -> &'static str {
    let mime = content_type
        .map(|s| {
            s.split([';', ' '])
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => return "html",
        "text/css" => return "css",
        "application/javascript" | "text/javascript" | "application/x-javascript" => return "js",
        "application/json" | "application/ld+json" | "application/problem+json" => return "json",
        "application/xml" | "text/xml" => return "xml",
        "image/svg+xml" => return "svg",
        "text/plain" => return "txt",
        "text/csv" => return "csv",
        "application/wasm" => return "wasm",
        _ => {}
    }
    if mime.ends_with("+json") {
        return "json";
    }
    if mime.ends_with("+xml") {
        return "xml";
    }
    match resource_type.unwrap_or("") {
        "document" => "html",
        "stylesheet" => "css",
        "script" => "js",
        "xhr" | "fetch" => "json",
        _ => {
            let url_ext = url
                .and_then(|u| u.split(['?', '#']).next())
                .and_then(|u| u.rsplit('.').next())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            match url_ext.as_str() {
                "html" | "htm" => "html",
                "css" => "css",
                "js" | "mjs" => "js",
                "json" => "json",
                "xml" => "xml",
                "svg" => "svg",
                "txt" => "txt",
                "csv" => "csv",
                "wasm" => "wasm",
                "map" => "map",
                _ => "bin",
            }
        }
    }
}

fn prelude() -> String {
    format!(
        "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         @prefix hkb-rec: <{NS}> .\n"
    )
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .with_context(|| format!("{}:{}: invalid json", path.display(), i + 1))?;
        out.push(v);
    }
    Ok(out)
}

/// Turtle-quoted string literal (short form). Uses JSON string escaping,
/// which is a strict subset of Turtle's `STRING_LITERAL_QUOTE` escapes.
fn lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Raw escape for use inside `"..."` (no surrounding quotes).
fn esc(s: &str) -> String {
    let q = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    q[1..q.len() - 1].to_string()
}

/// Session ids in `session.json` are stored as compact ISO (`20260420T131057Z`);
/// expand to `2026-04-20T13:10:57Z` so the literal validates as xsd:dateTime.
fn normalize_session_started_at(s: &str) -> Option<String> {
    if s.contains('-') {
        return Some(s.to_string());
    }
    let b = s.as_bytes();
    if b.len() == 16 && b[8] == b'T' && b[15] == b'Z' {
        let y = &s[0..4];
        let mo = &s[4..6];
        let d = &s[6..8];
        let h = &s[9..11];
        let mi = &s[11..13];
        let se = &s[13..15];
        Some(format!("{y}-{mo}-{d}T{h}:{mi}:{se}Z"))
    } else {
        None
    }
}
