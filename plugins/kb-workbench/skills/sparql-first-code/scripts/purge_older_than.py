#!/usr/bin/env python3
"""List/print SPARQL operations to purge code ingestions older than N days.

This script does NOT call MCP tools itself — Claude orchestrates that. The
script produces:
  1. a SELECT query (--list) to find candidates,
  2. for each named graph the user confirms, a DROP GRAPH + UPDATE pair that
     removes triples but keeps the meta record (annotated with cbm:purgedAt).

Usage:
  purge_older_than.py --days 30 --list           # SELECT to run via query_sparql
  purge_older_than.py --drop <graph_iri>         # DROP+annotate update_sparql
"""
from __future__ import annotations
import argparse, datetime as dt, sys

CBM = "http://codebase-memory.dev/ontology#"

def now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

def list_query(days: int) -> str:
    cutoff = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ")
    return (
        f"PREFIX cbm: <{CBM}>\n"
        "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n"
        "SELECT ?g ?repo ?prov ?ingestedAt ?nodes ?edges WHERE {\n"
        "  GRAPH <urn:cbm:meta> {\n"
        "    ?g a cbm:CodeIngestion ;\n"
        "       cbm:repo ?repo ;\n"
        "       cbm:provenance ?prov ;\n"
        "       cbm:ingestedAt ?ingestedAt ;\n"
        "       cbm:nodeCount ?nodes ;\n"
        "       cbm:edgeCount ?edges .\n"
        "    FILTER NOT EXISTS { ?g cbm:purgedAt ?_ }\n"
        f'    FILTER (?ingestedAt < "{cutoff}"^^xsd:dateTime)\n'
        "  }\n"
        "} ORDER BY ?ingestedAt\n"
    )

def drop_update(graph_iri: str) -> str:
    return (
        f"PREFIX cbm: <{CBM}>\n"
        "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n"
        f"DROP GRAPH <{graph_iri}> ;\n"
        "INSERT DATA { GRAPH <urn:cbm:meta> {\n"
        f'  <{graph_iri}> cbm:purgedAt "{now_iso()}"^^xsd:dateTime .\n'
        "} }\n"
    )

def main() -> None:
    p = argparse.ArgumentParser()
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--list", action="store_true")
    g.add_argument("--drop", metavar="GRAPH_IRI")
    p.add_argument("--days", type=int, default=30)
    a = p.parse_args()
    sys.stdout.write(list_query(a.days) if a.list else drop_update(a.drop))

if __name__ == "__main__":
    main()
