#!/usr/bin/env python3
"""Build a SPARQL INSERT DATA statement that records a code ingestion.

Two modes:
  - git:    --git-meta git_meta.json --node-count N --edge-count M --graph <iri>
  - manual: --provenance <tag> --note "<text>" --repo <slug> --repo-path <p>
            --node-count N --edge-count M --graph <iri>

Prints the full SPARQL update string on stdout. Pipe into update_sparql.
"""
from __future__ import annotations
import argparse, datetime as dt, json, sys

CBM = "http://codebase-memory.dev/ontology#"

def lit(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'

def now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

def build(args: argparse.Namespace) -> str:
    triples = [
        f"<{args.graph}> a cbm:CodeIngestion",
        f"  ; cbm:repo {lit(args.repo)}",
        f"  ; cbm:repoPath {lit(args.repo_path)}",
        f'  ; cbm:ingestedAt "{now_iso()}"^^xsd:dateTime',
        f"  ; cbm:provenance {lit(args.provenance)}",
        f'  ; cbm:nodeCount "{args.node_count}"^^xsd:integer',
        f'  ; cbm:edgeCount "{args.edge_count}"^^xsd:integer',
    ]
    if args.note:
        triples.append(f"  ; cbm:provenanceNote {lit(args.note)}")
    if args.git_meta:
        g = json.load(open(args.git_meta))
        for k, p in [
            ("gitBranch", "gitBranch"), ("gitCommit", "gitCommit"),
            ("gitCommitShort", "gitCommitShort"), ("gitCommitMsg", "gitCommitMsg"),
            ("gitRemote", "gitRemote"),
        ]:
            if g.get(k):
                triples.append(f"  ; cbm:{p} {lit(g[k])}")
        if g.get("gitCommitDate"):
            triples.append(f'  ; cbm:gitCommitDate "{g["gitCommitDate"]}"^^xsd:dateTime')
        if "gitDirty" in g:
            v = "true" if g["gitDirty"] else "false"
            triples.append(f'  ; cbm:gitDirty "{v}"^^xsd:boolean')
    return (
        f"PREFIX cbm: <{CBM}>\n"
        "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n"
        "INSERT DATA { GRAPH <urn:cbm:meta> {\n"
        + "\n".join(triples) + " .\n} }\n"
    )

def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--graph", required=True)
    p.add_argument("--repo", required=True)
    p.add_argument("--repo-path", required=True)
    p.add_argument("--provenance", required=True,
                   help="git | playwright-import | har-import | manual-archive | decompiled-bundle | …")
    p.add_argument("--note", default="", help="free-text provenance description (REQUIRED when not git)")
    p.add_argument("--git-meta", help="path to JSON from git_meta.sh (only when provenance=git)")
    p.add_argument("--node-count", type=int, required=True)
    p.add_argument("--edge-count", type=int, required=True)
    args = p.parse_args()
    if args.provenance != "git" and not args.note:
        sys.exit("provenanceNote is required when provenance != git")
    sys.stdout.write(build(args))

if __name__ == "__main__":
    main()
