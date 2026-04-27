#!/usr/bin/env python3
"""Convert a codebase-memory-mcp graph dump (JSON) to Turtle for sparql-mcp.

Mirrors the vocabulary in sparql-mcp's crates/sparql-mcp-core/src/cbm/turtle.rs
(emit_node, emit_edge, edge_type_to_property).

stdin:  JSON {"nodes": [...], "edges": [...]} as produced by query_graph
argv:   <graph_iri>          IRI used only for header comment; loader pins it
stdout: Turtle text
"""
from __future__ import annotations
import json, re, sys
from urllib.parse import quote

CBM = "http://codebase-memory.dev/ontology#"
INST = "http://codebase-memory.dev/instance/"

PREAMBLE = f"""@prefix cbm: <{CBM}> .
@prefix inst: <{INST}> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
"""

def iri(qn: str) -> str:
    return f"inst:{quote(qn, safe='')}"

def lit(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r") + '"'

def edge_prop(t: str) -> str:
    # Map CBM edge_type to a SPARQL-friendly property name. Mirrors the Rust
    # edge_type_to_property: keep alnum + underscore; uppercase tokens kept.
    safe = re.sub(r"[^A-Za-z0-9_]", "_", t)
    return f"cbm:{safe}"

def emit_node(n: dict, out: list[str]) -> None:
    qn = n.get("qualifiedName") or n.get("qualified_name") or n.get("qn") or n["id"]
    label = n.get("label", "Node")
    out.append(f"{iri(qn)} a cbm:{label} ;")
    parts = [f"  cbm:qualifiedName {lit(qn)}"]
    for k in ("name", "filePath", "extension", "docstring"):
        if n.get(k):
            parts.append(f"  cbm:{k} {lit(str(n[k]))}")
    for k in ("startLine", "endLine", "complexity"):
        if n.get(k) is not None:
            parts.append(f'  cbm:{k} "{int(n[k])}"^^xsd:integer')
    out.append(" ;\n".join(parts) + " .\n")

def emit_edge(e: dict, out: list[str]) -> None:
    src = e.get("source") or e.get("src") or e["from"]
    dst = e.get("target") or e.get("dst") or e["to"]
    t = e.get("type") or e.get("edge_type") or e.get("relationship", "RELATED")
    out.append(f"{iri(src)} {edge_prop(t)} {iri(dst)} .\n")

def main() -> None:
    data = json.load(sys.stdin)
    out = [PREAMBLE]
    if len(sys.argv) > 1:
        out.append(f"# graph: {sys.argv[1]}\n")
    for n in data.get("nodes", []):
        emit_node(n, out)
    for e in data.get("edges", []):
        emit_edge(e, out)
    sys.stdout.write("".join(out))

if __name__ == "__main__":
    main()
