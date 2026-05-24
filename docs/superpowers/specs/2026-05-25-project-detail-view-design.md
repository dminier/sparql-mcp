# Project Detail View (TUI) — Design Spec

**Date:** 2026-05-25
**Status:** approved
**Scope:** A tabbed detail screen shown when a project is selected in the TUI.

## Context

The TUI lists projects with global/per-project counts. Selecting a project should
open a detail screen so the user can read the project's identity, the ontologies
(schemas) it uses with inheritance, and quality metrics — each metric explained
and interpreted.

## Goals

Three tabs, switchable with Tab / ←→ / 1·2·3; Esc or q returns to the list.

1. **Detail** (default): project name + full description (wrapped to the terminal).
2. **Ontologies**: classes/schemas used in the project graph — label, comment,
   instance count, and inheritance (`rdfs:subClassOf` → super-classes). Class
   metadata is read as a UNION of the project graph and the default graph
   (inheritance triples live inside project graphs in practice).
3. **Metrics**: each with a one-line *explanation* (what it measures) and an
   *interpretation* (how to read this project's value):
   - Triples — total RDF statements.
   - Nodes — distinct IRIs (subject or object).
   - Classes — distinct `rdf:type` classes instantiated.
   - Predicates — distinct properties used.
   - Density — triples / node (description richness).
   - Typed ratio — share of subjects carrying an `rdf:type` (schema conformance).
   - Literal ratio — share of object positions that are literals (data vs links).

## Non-goals

- Editing; drill-down into individual instances; charts.

## Design

Pure, unit-tested data layer in `application/detail.rs`:

- `RawMetrics { triples, nodes, classes, predicates, subjects, typed_subjects, literal_objects }`
  via single scalar SPARQL counts scoped to the project graph.
- `build_metrics(&RawMetrics) -> Vec<Metric>` — derives values + interpretation text.
- `collect_classes(store, graph) -> Vec<OntologyClassUse>` — used classes (grouped),
  joined with subClassOf / label / comment maps (UNION of project + default graph).
- `collect_project_detail(...)` ties identity + classes + metrics together.

The project graph IRI is validated (`validate_graph_iri`) before any interpolation —
no whitespace or IRI-delimiter characters — keeping the injection surface closed.

TUI: a `Screen` enum (`List` / `Detail { idx, tab }`). Enter on a selected row
computes the detail on demand (read-only queries) and switches screen.

## Testing

`tests/detail.rs`: seed an in-memory store with a small ontology (labels +
subClassOf) and a typed project graph; assert exact `RawMetrics`, class instance
counts, resolved super-classes, and that every metric carries non-empty
explanation + interpretation.
