//! Protected graphs and update classification
//! (`docs/designs/sparql-mcp-write-path-hardening.md`, *Guards*).
//!
//! [`classify_update`] works on the operations as spargebra parses them.
//! spargebra rewrites `ADD`, `COPY` and `MOVE` into `DROP` and delete/insert
//! operations, so those forms are covered without matching them by name.
//!
//! The classifier never tries to decide whether a `WHERE` clause could bind a
//! protected graph. A template whose graph is a variable (`GRAPH ?g`) counts
//! as touching every graph, protected ones included, and is refused as a
//! whole.

use std::collections::BTreeSet;

use spargebra::algebra::GraphTarget;
use spargebra::term::{GraphName, GraphNamePattern, GroundTermPattern, NamedNodePattern};
use spargebra::{GraphUpdateOperation, Update};

use crate::application::limits::ToolError;

/// Graphs only the core's own code may write.
pub const PROTECTED_GRAPHS: [&str; 2] = ["urn:meta", "urn:sparql-mcp:jobs"];

/// Graph namespaces only the core's own code may write.
pub const PROTECTED_PREFIXES: [&str; 2] = ["urn:hkb:meta:", "urn:sparql-mcp:staging:"];

/// IRIs agents may not put in payloads: the core mints them (skolem IRIs, job
/// and staging graphs).
pub const RESERVED_IRI_PREFIX: &str = "urn:sparql-mcp:";

/// The default graph in [`UpdateClass::graphs`].
pub const DEFAULT_GRAPH: &str = "";

pub fn is_protected(graph: &str) -> bool {
    PROTECTED_GRAPHS.contains(&graph) || PROTECTED_PREFIXES.iter().any(|p| graph.starts_with(p))
}

pub fn is_reserved_iri(iri: &str) -> bool {
    iri.starts_with(RESERVED_IRI_PREFIX)
}

/// What an update does, without running it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UpdateClass {
    /// One of `insert_data`, `delete_data`, `delete_insert`, `load`, `clear`,
    /// `create`, `drop`, in order.
    pub operations: Vec<&'static str>,
    /// The constant graphs the update names; [`DEFAULT_GRAPH`] is the default
    /// graph.
    pub graphs: BTreeSet<String>,
    /// Some operation targets a graph chosen at run time (`GRAPH ?g`, or
    /// `CLEAR`/`DROP` of `NAMED` or `ALL`).
    pub all_graphs: bool,
    /// `DROP`, `CLEAR`, or a delete template of three variables.
    pub destructive: bool,
    /// A delete template whose graph is a variable.
    pub deletes_from_any_graph: bool,
    /// `CLEAR`/`DROP` of `NAMED` or `ALL`.
    pub wipes_many_graphs: bool,
    /// The update contains a `LOAD`.
    pub loads: bool,
}

/// Parse and classify. A syntax error is a `parse_error`.
pub fn classify_sparql(sparql: &str) -> Result<UpdateClass, ToolError> {
    let parsed = spargebra::SparqlParser::new()
        .parse_update(sparql)
        .map_err(|e| ToolError::ParseError {
            message: e.to_string(),
            line: None,
            column: None,
        })?;
    Ok(classify_update(&parsed))
}

pub fn classify_update(update: &Update) -> UpdateClass {
    let mut c = UpdateClass::default();
    let target = |c: &mut UpdateClass, g: &GraphName| match g {
        GraphName::NamedNode(n) => {
            c.graphs.insert(n.as_str().to_string());
        }
        GraphName::DefaultGraph => {
            c.graphs.insert(DEFAULT_GRAPH.to_string());
        }
    };
    let pattern = |c: &mut UpdateClass, g: &GraphNamePattern, deleting: bool| match g {
        GraphNamePattern::NamedNode(n) => {
            c.graphs.insert(n.as_str().to_string());
        }
        GraphNamePattern::DefaultGraph => {
            c.graphs.insert(DEFAULT_GRAPH.to_string());
        }
        GraphNamePattern::Variable(_) => {
            c.all_graphs = true;
            if deleting {
                c.deletes_from_any_graph = true;
            }
        }
    };

    for op in &update.operations {
        match op {
            GraphUpdateOperation::InsertData { data } => {
                c.operations.push("insert_data");
                for q in data {
                    target(&mut c, &q.graph_name);
                }
            }
            GraphUpdateOperation::DeleteData { data } => {
                c.operations.push("delete_data");
                for q in data {
                    target(&mut c, &q.graph_name);
                }
            }
            GraphUpdateOperation::DeleteInsert { delete, insert, .. } => {
                c.operations.push("delete_insert");
                for q in delete {
                    pattern(&mut c, &q.graph_name, true);
                    let all_variable = matches!(q.subject, GroundTermPattern::Variable(_))
                        && matches!(q.predicate, NamedNodePattern::Variable(_))
                        && matches!(q.object, GroundTermPattern::Variable(_));
                    if all_variable {
                        c.destructive = true;
                    }
                }
                for q in insert {
                    pattern(&mut c, &q.graph_name, false);
                }
            }
            GraphUpdateOperation::Load { .. } => {
                c.operations.push("load");
                c.loads = true;
            }
            GraphUpdateOperation::Clear { graph, .. } => {
                c.operations.push("clear");
                c.destructive = true;
                graph_target(&mut c, graph);
            }
            GraphUpdateOperation::Drop { graph, .. } => {
                c.operations.push("drop");
                c.destructive = true;
                graph_target(&mut c, graph);
            }
            GraphUpdateOperation::Create { graph, .. } => {
                c.operations.push("create");
                c.graphs.insert(graph.as_str().to_string());
            }
        }
    }
    c
}

fn graph_target(c: &mut UpdateClass, target: &GraphTarget) {
    match target {
        GraphTarget::NamedNode(n) => {
            c.graphs.insert(n.as_str().to_string());
        }
        GraphTarget::DefaultGraph => {
            c.graphs.insert(DEFAULT_GRAPH.to_string());
        }
        GraphTarget::NamedGraphs | GraphTarget::AllGraphs => {
            c.all_graphs = true;
            c.wipes_many_graphs = true;
        }
    }
}

/// Decide whether an update may run for a non-privileged caller.
///
/// Always refused, whatever `allow_destructive`:
/// *   `LOAD` (the HTTP client is not compiled in, and agents have no reason
///     to fetch);
/// *   any write to a protected graph;
/// *   `CLEAR`/`DROP` of `NAMED` or `ALL`, which would reach protected graphs;
/// *   a delete template whose graph is a variable, for the same reason.
///
/// Refused unless `allow_destructive`: `DROP`, `CLEAR`, a delete template of
/// three variables, and an insert template whose graph is a variable.
pub fn check_update(class: &UpdateClass, allow_destructive: bool) -> Result<(), ToolError> {
    if class.loads {
        return Err(ToolError::LoadForbidden);
    }
    if let Some(graph) = class.graphs.iter().find(|g| is_protected(g)) {
        return Err(ToolError::GraphProtected {
            graph: graph.clone(),
        });
    }
    if class.wipes_many_graphs || class.deletes_from_any_graph {
        return Err(ToolError::GraphProtected {
            graph: "(any graph, protected ones included)".to_string(),
        });
    }
    if (class.destructive || class.all_graphs) && !allow_destructive {
        return Err(ToolError::DestructiveUpdateForbidden);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(sparql: &str, allow: bool) -> Result<(), ToolError> {
        check_update(&classify_sparql(sparql).unwrap(), allow)
    }

    fn code(r: Result<(), ToolError>) -> &'static str {
        r.err().map_or("ok", |e| e.code())
    }

    #[test]
    fn protected_graph_names() {
        assert!(is_protected("urn:meta"));
        assert!(is_protected("urn:sparql-mcp:jobs"));
        assert!(is_protected("urn:hkb:meta:loaded-files"));
        assert!(is_protected("urn:sparql-mcp:staging:01JD"));
        assert!(!is_protected("urn:project:p"));
        assert!(!is_protected(DEFAULT_GRAPH));
        assert!(is_reserved_iri("urn:sparql-mcp:genid:x"));
        assert!(!is_reserved_iri("urn:project:p"));
    }

    #[test]
    fn ordinary_ground_writes_are_allowed() {
        assert_eq!(
            code(verdict("INSERT DATA { <urn:a> <urn:p> <urn:b> }", false)),
            "ok"
        );
        assert_eq!(
            code(verdict(
                "INSERT DATA { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }",
                false
            )),
            "ok"
        );
        assert_eq!(
            code(verdict(
                "DELETE DATA { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } }",
                false
            )),
            "ok"
        );
        assert_eq!(
            code(verdict(
                "DELETE { ?s <urn:p> ?o } INSERT { ?s <urn:q> ?o } WHERE { ?s <urn:p> ?o }",
                false
            )),
            "ok",
            "a pattern update with a constant predicate is not destructive"
        );
    }

    #[test]
    fn protected_graphs_are_refused_whatever_the_setting() {
        for update in [
            "INSERT DATA { GRAPH <urn:meta> { <urn:a> <urn:p> <urn:b> } }",
            "DELETE DATA { GRAPH <urn:hkb:meta:loaded-files> { <urn:a> <urn:p> <urn:b> } }",
            "CREATE GRAPH <urn:sparql-mcp:jobs>",
            "CLEAR GRAPH <urn:meta>",
            "DROP GRAPH <urn:sparql-mcp:staging:x>",
            "WITH <urn:meta> DELETE { ?s ?p ?o } WHERE { ?s ?p ?o }",
            "INSERT { GRAPH <urn:meta> { ?s ?p ?o } } WHERE { ?s ?p ?o }",
        ] {
            assert_eq!(code(verdict(update, false)), "graph_protected", "{update}");
            assert_eq!(code(verdict(update, true)), "graph_protected", "{update}");
        }
    }

    #[test]
    fn add_copy_and_move_are_covered_through_their_rewritten_form() {
        for update in [
            "ADD <urn:project:p> TO <urn:meta>",
            "COPY <urn:project:p> TO <urn:meta>",
            "MOVE <urn:project:p> TO <urn:meta>",
            "MOVE <urn:meta> TO <urn:project:p>",
        ] {
            assert_eq!(code(verdict(update, true)), "graph_protected", "{update}");
        }
        // Between ordinary graphs they are destructive (COPY/MOVE clear the
        // destination first), so they need the setting.
        assert_eq!(
            code(verdict("MOVE <urn:project:a> TO <urn:project:b>", false)),
            "destructive_update_forbidden"
        );
        assert_eq!(
            code(verdict("MOVE <urn:project:a> TO <urn:project:b>", true)),
            "ok"
        );
    }

    #[test]
    fn drop_and_clear_need_the_setting() {
        for update in [
            "DROP GRAPH <urn:project:p>",
            "CLEAR GRAPH <urn:project:p>",
            "CLEAR DEFAULT",
            "DROP SILENT GRAPH <urn:project:p>",
        ] {
            assert_eq!(
                code(verdict(update, false)),
                "destructive_update_forbidden",
                "{update}"
            );
            assert_eq!(code(verdict(update, true)), "ok", "{update}");
        }
    }

    #[test]
    fn dropping_all_or_named_graphs_is_never_allowed() {
        for update in ["DROP ALL", "CLEAR ALL", "DROP NAMED", "CLEAR NAMED"] {
            assert_eq!(code(verdict(update, false)), "graph_protected", "{update}");
            assert_eq!(code(verdict(update, true)), "graph_protected", "{update}");
        }
    }

    #[test]
    fn a_delete_template_of_three_variables_is_destructive() {
        for update in [
            "DELETE WHERE { ?s ?p ?o }",
            "DELETE { ?s ?p ?o } WHERE { ?s a <urn:X> }",
        ] {
            assert_eq!(
                code(verdict(update, false)),
                "destructive_update_forbidden",
                "{update}"
            );
            assert_eq!(code(verdict(update, true)), "ok", "{update}");
        }
    }

    #[test]
    fn a_variable_graph_counts_as_every_graph() {
        // Deleting from a variable graph can reach a protected one.
        for update in [
            "DELETE { GRAPH ?g { ?s <urn:p> ?o } } WHERE { GRAPH ?g { ?s <urn:p> ?o } }",
            "DELETE WHERE { GRAPH ?g { ?s ?p ?o } }",
        ] {
            assert_eq!(code(verdict(update, false)), "graph_protected", "{update}");
            assert_eq!(code(verdict(update, true)), "graph_protected", "{update}");
        }
        // Inserting into a variable graph needs the setting.
        let insert = "INSERT { GRAPH ?g { ?s <urn:q> ?o } } WHERE { GRAPH ?g { ?s <urn:p> ?o } }";
        assert_eq!(code(verdict(insert, false)), "destructive_update_forbidden");
        assert_eq!(code(verdict(insert, true)), "ok");
    }

    #[test]
    fn load_is_always_refused() {
        for update in [
            "LOAD <http://example.com/data.ttl>",
            "LOAD SILENT <http://example.com/data.ttl> INTO GRAPH <urn:project:p>",
        ] {
            assert_eq!(code(verdict(update, true)), "load_forbidden", "{update}");
        }
    }

    #[test]
    fn classification_reports_operations_and_graphs() {
        let c = classify_sparql(
            "INSERT DATA { GRAPH <urn:project:p> { <urn:a> <urn:p> <urn:b> } } ; \
             DROP SILENT GRAPH <urn:project:q>",
        )
        .unwrap();
        assert_eq!(c.operations, vec!["insert_data", "drop"]);
        assert!(c.graphs.contains("urn:project:p") && c.graphs.contains("urn:project:q"));
        assert!(c.destructive);
    }

    #[test]
    fn a_syntax_error_is_a_parse_error() {
        let err = classify_sparql("INSERT DATA {").unwrap_err();
        assert_eq!(err.code(), "parse_error");
    }
}
