//! CBM graph → Turtle serialiser.
//!
//! Streams triples into a `String` buffer that can be fed directly to
//! `SparqlStore::load_rdf()` — no temp file, no subprocess.

use super::model::{Edge, KnowledgeGraph, Node};
use std::path::Path;

const CBM_ONT: &str = "http://codebase-memory.dev/ontology#";
const CBM_INST: &str = "http://codebase-memory.dev/instance/";

/// Options controlling the Turtle export. `with_source` slurps every File
/// node's content (resolved against the CBM project's `root_path`) and emits
/// `cbm:sourceCode` + `cbm:sourceBytes`. Files larger than `max_source_bytes`
/// are skipped silently — caller can grep the log for `skipped_big`.
#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    pub with_source: bool,
    pub max_source_bytes: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            with_source: false,
            max_source_bytes: 500_000,
        }
    }
}

pub fn graph_to_turtle(kg: &KnowledgeGraph) -> String {
    graph_to_turtle_with(kg, ExportOptions::default())
}

pub fn graph_to_turtle_with(kg: &KnowledgeGraph, opts: ExportOptions) -> String {
    let slug = project_slug(&kg.project.name);
    let inst_base = format!("{CBM_INST}{slug}#");

    let mut buf = String::with_capacity(kg.nodes.len() * 256 + kg.edges.len() * 80);

    buf.push_str(&format!(
        "@prefix cbm:  <{CBM_ONT}> .\n\
         @prefix inst: <{inst_base}> .\n\
         @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .\n\n"
    ));

    let root = Path::new(&kg.project.root_path);
    let mut emitted_src = 0usize;
    let mut skipped_big = 0usize;
    let mut skipped_missing = 0usize;

    for node in &kg.nodes {
        emit_node(&mut buf, node);
        if opts.with_source && node.label == "File" && !node.file_path.is_empty() {
            let abs = root.join(&node.file_path);
            match std::fs::read(&abs) {
                Ok(bytes) if bytes.len() <= opts.max_source_bytes => {
                    let text = String::from_utf8_lossy(&bytes);
                    buf.push_str(&format!(
                        "{} cbm:sourceCode \"{}\" ;\n    cbm:sourceBytes \"{}\"^^xsd:integer .\n",
                        node_ref(node.id),
                        escape(&text),
                        bytes.len()
                    ));
                    emitted_src += 1;
                }
                Ok(_) => skipped_big += 1,
                Err(_) => skipped_missing += 1,
            }
        }
    }

    for edge in &kg.edges {
        emit_edge(&mut buf, edge);
    }

    if opts.with_source {
        tracing::info!(
            emitted = emitted_src,
            skipped_big,
            skipped_missing,
            "source bodies attached"
        );
    }

    buf
}

// ── node ─────────────────────────────────────────────────────────────────────

fn emit_node(buf: &mut String, n: &Node) {
    let uri = node_ref(n.id);
    buf.push_str(&format!("{uri}\n    a cbm:{} ;\n", n.label));

    push_str_prop(buf, "cbm:name", &n.name);
    push_str_prop(buf, "cbm:qualifiedName", &n.qualified_name);
    if !n.file_path.is_empty() {
        push_str_prop(buf, "cbm:filePath", &n.file_path);
    }
    if n.start_line > 0 {
        push_int_prop(buf, "cbm:startLine", n.start_line);
    }
    if n.end_line > 0 {
        push_int_prop(buf, "cbm:endLine", n.end_line);
    }

    let p = &n.properties;
    if let Some(v) = p.complexity {
        push_int_prop(buf, "cbm:complexity", v);
    }
    if let Some(v) = p.lines {
        push_int_prop(buf, "cbm:lines", v);
    }
    if let Some(v) = p.is_exported {
        push_bool_prop(buf, "cbm:isExported", v);
    }
    if let Some(v) = p.is_test {
        push_bool_prop(buf, "cbm:isTest", v);
    }
    if let Some(v) = p.is_entry_point {
        push_bool_prop(buf, "cbm:isEntryPoint", v);
    }
    if let Some(ref v) = p.signature {
        push_str_prop(buf, "cbm:signature", v);
    }
    if let Some(ref v) = p.docstring {
        push_str_prop(buf, "cbm:docstring", v);
    }
    if let Some(ref v) = p.extension {
        push_str_prop(buf, "cbm:extension", v);
    }

    // Replace last " ;\n" with " .\n"
    if buf.ends_with(" ;\n") {
        let len = buf.len();
        buf.truncate(len - 3);
        buf.push_str(" .\n\n");
    } else {
        buf.push_str("    .\n\n");
    }
}

// ── edge ─────────────────────────────────────────────────────────────────────

fn emit_edge(buf: &mut String, e: &Edge) {
    let prop = edge_type_to_property(&e.edge_type);
    let src = node_ref(e.source_id);
    let tgt = node_ref(e.target_id);
    buf.push_str(&format!("{src} cbm:{prop} {tgt} .\n"));
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn node_ref(id: i64) -> String {
    format!("inst:node_{id}")
}

fn project_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn edge_type_to_property(edge_type: &str) -> String {
    let mut parts = edge_type.split('_');
    let first = parts.next().unwrap_or("").to_lowercase();
    let rest: String = parts
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
            }
        })
        .collect();
    first + &rest
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn push_str_prop(buf: &mut String, pred: &str, val: &str) {
    buf.push_str(&format!("    {pred} \"{}\" ;\n", escape(val)));
}

fn push_int_prop(buf: &mut String, pred: &str, val: i32) {
    buf.push_str(&format!("    {pred} \"{val}\"^^xsd:integer ;\n"));
}

fn push_bool_prop(buf: &mut String, pred: &str, val: bool) {
    buf.push_str(&format!(
        "    {pred} \"{}\"^^xsd:boolean ;\n",
        if val { "true" } else { "false" }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_property_names() {
        assert_eq!(edge_type_to_property("CALLS"), "calls");
        assert_eq!(edge_type_to_property("CONTAINS_FILE"), "containsFile");
        assert_eq!(
            edge_type_to_property("FILE_CHANGES_WITH"),
            "fileChangesWith"
        );
        assert_eq!(edge_type_to_property("DEFINES_METHOD"), "definesMethod");
    }

    #[test]
    fn escape_special_chars() {
        assert_eq!(escape("say \"hi\""), r#"say \"hi\""#);
        assert_eq!(escape("line1\nline2"), r"line1\nline2");
    }
}
