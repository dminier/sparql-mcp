//! RDF-to-graph projection — plain Rust, no Bevy systems or rendering.
//! `ViewerGraph` derives Bevy's `Resource` marker only so `plugin.rs` can
//! insert it directly; that derive has no runtime behavior and every
//! function here is exercised by its unit tests with no App or window.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bevy::prelude::Resource;
use sparql_mcp::application::event_sink::{Snapshot, StoreEvent, SubjectKind, CLASS_TYPE_IRIS};

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const RDFS_SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const RDFS_LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
const SMC_LABEL: &str = "<https://sparql-mcp.dev/ns#label>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Class,
    Instance,
}

#[derive(Debug, Clone)]
pub struct Node {
    /// N-Triples-encoded term (`<iri>` or `_:id`) this node represents.
    pub term: String,
    pub kind: NodeKind,
    pub label: String,
    /// Predicate → set of N-Triples-encoded literal objects.
    pub attributes: BTreeMap<String, BTreeSet<String>>,
    /// N-Triples-encoded `rdf:type` objects.
    pub types: BTreeSet<String>,
    pub mentions: u32,
    /// `event_seq` of the last event that changed this node.
    pub last_touched: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeStyle {
    Type,
    SubClass,
    Relation,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub predicate: String,
    pub style: EdgeStyle,
}

/// Old id → new id, for IRI nodes present in both the previous graph and a
/// same-graph snapshot's new graph. Blank nodes never appear here — their
/// ids are not stable between snapshots.
pub struct Remap(pub HashMap<NodeId, NodeId>);

#[derive(Debug, Default, Clone, Copy)]
pub struct Applied {
    pub added: u32,
    pub removed: u32,
    pub touched: u32,
    pub resync: bool,
    pub ignored: bool,
}

#[derive(Resource)]
pub struct ViewerGraph {
    pub graph: String,
    pub triples: HashSet<[String; 3]>,
    pub nodes: Vec<Option<Node>>,
    pub index: HashMap<String, NodeId>,
    pub edges: Vec<Edge>,
    pub generation: u64,
    pub event_seq: u64,
}

impl ViewerGraph {
    /// An empty graph for no project in particular — the state before the
    /// first snapshot arrives.
    pub fn empty() -> Self {
        Self::empty_for(String::new())
    }

    fn empty_for(graph: String) -> Self {
        Self {
            graph,
            triples: HashSet::new(),
            nodes: Vec::new(),
            index: HashMap::new(),
            edges: Vec::new(),
            generation: 0,
            event_seq: 0,
        }
    }

    /// Build a fresh graph (new node ids) from `snapshot`. When `previous`
    /// is for the same graph, returns a [`Remap`] so the layout can carry
    /// IRI-node positions over; otherwise `None`.
    pub fn from_snapshot(
        snapshot: &Snapshot,
        previous: Option<&ViewerGraph>,
    ) -> (Self, Option<Remap>) {
        let mut graph = Self::empty_for(snapshot.graph.clone());
        graph.event_seq = previous.map_or(0, |p| p.event_seq);
        for triple in &snapshot.triples {
            graph.insert_triple(triple, Some(&snapshot.kinds));
        }
        graph.generation = previous.map_or(1, |p| p.generation + 1);

        let remap = previous.filter(|p| p.graph == snapshot.graph).map(|prev| {
            let mut map = HashMap::new();
            for (term, &old_id) in &prev.index {
                if term.starts_with("_:") {
                    continue; // blank nodes never carry over
                }
                if let Some(&new_id) = graph.index.get(term) {
                    map.insert(old_id, new_id);
                }
            }
            Remap(map)
        });

        (graph, remap)
    }

    /// Apply one [`StoreEvent`]. An event for a graph other than this
    /// one's is ignored outright (recorded in the activity feed as "other
    /// graph" by `ingest.rs`, Phase 5).
    pub fn apply_event(&mut self, event: &StoreEvent) -> Applied {
        let event_graph = match event {
            StoreEvent::Insert { graph, .. }
            | StoreEvent::Delete { graph, .. }
            | StoreEvent::Changed { graph, .. } => graph,
        };
        if event_graph != &self.graph {
            return Applied {
                ignored: true,
                ..Default::default()
            };
        }

        self.event_seq += 1;
        let mut applied = Applied::default();
        match event {
            StoreEvent::Insert { triples, kinds, .. } => {
                for t in triples {
                    if self.insert_triple(t, Some(kinds)) {
                        applied.added += 1;
                    }
                }
            }
            StoreEvent::Delete { triples, .. } => {
                for t in triples {
                    if self.remove_triple(t) {
                        applied.removed += 1;
                    }
                }
            }
            StoreEvent::Changed { .. } => applied.resync = true,
        }
        applied.touched = applied.added + applied.removed;
        if applied.added > 0 || applied.removed > 0 || applied.resync {
            self.generation += 1;
        }
        applied
    }

    fn get_or_create_node(&mut self, term: &str) -> NodeId {
        if let Some(&id) = self.index.get(term) {
            return id;
        }
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Some(Node {
            term: term.to_string(),
            kind: NodeKind::Instance,
            label: fallback_label(term),
            attributes: BTreeMap::new(),
            types: BTreeSet::new(),
            mentions: 0,
            last_touched: self.event_seq,
        }));
        self.index.insert(term.to_string(), id);
        id
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes[id.0 as usize]
            .as_mut()
            .expect("node id refers to a live node")
    }

    fn is_ref(term: &str) -> bool {
        term.starts_with('<') || term.starts_with("_:")
    }

    /// Insert one triple (set semantics: a duplicate is a no-op, returns
    /// `false`). `kinds` is the server's classification for a batch of
    /// freshly-inserted triples (snapshot or `Insert` event) — absent for
    /// a plain re-insert path there isn't one here, so it is always `Some`
    /// in practice.
    fn insert_triple(
        &mut self,
        triple: &[String; 3],
        kinds: Option<&BTreeMap<String, SubjectKind>>,
    ) -> bool {
        if !self.triples.insert(triple.clone()) {
            return false;
        }
        let [s, p, o] = triple;

        let event_seq = self.event_seq;
        let subject_id = self.get_or_create_node(s);
        {
            let node = self.node_mut(subject_id);
            node.mentions += 1;
            node.last_touched = event_seq;
        }
        if p == RDF_TYPE {
            self.node_mut(subject_id).types.insert(o.clone());
            if is_class_type_object(o) {
                self.node_mut(subject_id).kind = NodeKind::Class; // sticky
            }
        }
        if kinds.and_then(|k| k.get(s)) == Some(&SubjectKind::Class) {
            self.node_mut(subject_id).kind = NodeKind::Class; // sticky
        }

        if Self::is_ref(o) {
            let object_id = self.get_or_create_node(o);
            let node = self.node_mut(object_id);
            node.mentions += 1;
            node.last_touched = event_seq;
            self.edges.push(Edge {
                from: subject_id,
                to: object_id,
                predicate: p.clone(),
                style: edge_style(p),
            });
        } else {
            self.node_mut(subject_id)
                .attributes
                .entry(p.clone())
                .or_default()
                .insert(o.clone());
            recompute_label(self.node_mut(subject_id));
        }
        true
    }

    /// Remove one triple, matched by its exact N-Triples strings (set
    /// semantics: deleting an absent triple is a no-op, returns `false`).
    /// Each node mentioned counts down and is removed at 0.
    fn remove_triple(&mut self, triple: &[String; 3]) -> bool {
        if !self.triples.remove(triple) {
            return false;
        }
        let [s, p, o] = triple;

        if Self::is_ref(o) {
            if let (Some(&from_id), Some(&to_id)) = (self.index.get(s), self.index.get(o)) {
                self.edges
                    .retain(|e| !(e.from == from_id && e.to == to_id && e.predicate == *p));
            }
        } else if let Some(&id) = self.index.get(s) {
            let node = self.node_mut(id);
            if let Some(set) = node.attributes.get_mut(p) {
                set.remove(o);
                if set.is_empty() {
                    node.attributes.remove(p);
                }
            }
            recompute_label(self.node_mut(id));
        }

        self.decrement_mention(s);
        if Self::is_ref(o) {
            self.decrement_mention(o);
        }
        true
    }

    fn decrement_mention(&mut self, term: &str) {
        let Some(&id) = self.index.get(term) else {
            return;
        };
        let Some(node) = self.nodes[id.0 as usize].as_mut() else {
            return;
        };
        node.mentions = node.mentions.saturating_sub(1);
        if node.mentions == 0 {
            self.nodes[id.0 as usize] = None;
            self.index.remove(term);
        }
    }
}

fn recompute_label(node: &mut Node) {
    let literal = node
        .attributes
        .get(RDFS_LABEL)
        .and_then(|set| set.iter().next())
        .or_else(|| {
            node.attributes
                .get(SMC_LABEL)
                .and_then(|set| set.iter().next())
        });
    node.label = match literal {
        Some(nt) => literal_value(nt),
        None => fallback_label(&node.term),
    };
}

/// `"lex"`, `"lex"@lang` or `"lex"^^<dt>` → unescaped `lex`.
fn literal_value(nt: &str) -> String {
    let after_open = nt.strip_prefix('"').unwrap_or(nt);
    let end = find_closing_quote(after_open);
    after_open[..end]
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

fn find_closing_quote(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            return i;
        }
        i += 1;
    }
    s.len()
}

fn fallback_label(term: &str) -> String {
    if let Some(iri) = term.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        iri.rsplit(['#', '/']).next().unwrap_or(iri).to_string()
    } else if let Some(id) = term.strip_prefix("_:") {
        id.to_string()
    } else {
        term.to_string()
    }
}

fn edge_style(predicate: &str) -> EdgeStyle {
    if predicate == RDF_TYPE {
        EdgeStyle::Type
    } else if predicate == RDFS_SUBCLASS_OF {
        EdgeStyle::SubClass
    } else {
        EdgeStyle::Relation
    }
}

fn is_class_type_object(object: &str) -> bool {
    CLASS_TYPE_IRIS
        .iter()
        .any(|iri| object.strip_prefix('<').and_then(|s| s.strip_suffix('>')) == Some(*iri))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;

    fn t(s: &str, p: &str, o: &str) -> [String; 3] {
        [s.to_string(), p.to_string(), o.to_string()]
    }

    fn snapshot(
        graph: &str,
        triples: Vec<[String; 3]>,
        kinds: Map<String, SubjectKind>,
    ) -> Snapshot {
        Snapshot {
            graph: graph.to_string(),
            triples,
            kinds,
        }
    }

    fn insert_event(
        graph: &str,
        triples: Vec<[String; 3]>,
        kinds: Map<String, SubjectKind>,
    ) -> StoreEvent {
        StoreEvent::Insert {
            graph: graph.to_string(),
            triples,
            kinds,
        }
    }

    #[test]
    fn literals_become_attributes() {
        let snap = snapshot(
            "g",
            vec![t("<urn:a>", "<urn:name>", "\"Alice\"")],
            Map::new(),
        );
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        let node = g.nodes[g.index["<urn:a>"].0 as usize].as_ref().unwrap();
        assert!(node.attributes["<urn:name>"].contains("\"Alice\""));
        assert_eq!(g.nodes.len(), 1, "a literal object never creates a node");
    }

    #[test]
    fn edge_style_per_predicate() {
        let snap = snapshot(
            "g",
            vec![
                t("<urn:a>", RDF_TYPE, "<urn:C>"),
                t("<urn:a>", RDFS_SUBCLASS_OF, "<urn:B>"),
                t("<urn:a>", "<urn:knows>", "<urn:b>"),
            ],
            Map::new(),
        );
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        let styles: Vec<EdgeStyle> = g.edges.iter().map(|e| e.style).collect();
        assert!(styles.contains(&EdgeStyle::Type));
        assert!(styles.contains(&EdgeStyle::SubClass));
        assert!(styles.contains(&EdgeStyle::Relation));
    }

    #[test]
    fn classification_by_each_class_type_iri_and_from_kinds() {
        for iri in CLASS_TYPE_IRIS {
            let snap = snapshot(
                "g",
                vec![t("<urn:x>", RDF_TYPE, &format!("<{iri}>"))],
                Map::new(),
            );
            let (g, _) = ViewerGraph::from_snapshot(&snap, None);
            let node = g.nodes[g.index["<urn:x>"].0 as usize].as_ref().unwrap();
            assert_eq!(node.kind, NodeKind::Class, "failed for {iri}");
        }

        let mut kinds = Map::new();
        kinds.insert("<urn:y>".to_string(), SubjectKind::Class);
        let snap = snapshot("g", vec![t("<urn:y>", "<urn:p>", "\"v\"")], kinds);
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        let node = g.nodes[g.index["<urn:y>"].0 as usize].as_ref().unwrap();
        assert_eq!(node.kind, NodeKind::Class);
    }

    #[test]
    fn a_class_stays_a_class_across_events() {
        let snap = snapshot("g", vec![t("<urn:a>", RDF_TYPE, "<urn:Thing>")], {
            let mut k = Map::new();
            k.insert("<urn:a>".to_string(), SubjectKind::Class);
            k
        });
        let (mut g, _) = ViewerGraph::from_snapshot(&snap, None);

        g.apply_event(&insert_event(
            "g",
            vec![t("<urn:a>", "<urn:name>", "\"Thing one\"")],
            Map::new(),
        ));
        let node = g.nodes[g.index["<urn:a>"].0 as usize].as_ref().unwrap();
        assert_eq!(node.kind, NodeKind::Class);
    }

    #[test]
    fn duplicate_insert_is_a_no_op() {
        let mut g = ViewerGraph::empty();
        g.graph = "g".to_string();
        let applied1 = g.apply_event(&insert_event(
            "g",
            vec![t("<urn:a>", "<urn:p>", "<urn:b>")],
            Map::new(),
        ));
        assert_eq!(applied1.added, 1);
        let gen_after_first = g.generation;

        let applied2 = g.apply_event(&insert_event(
            "g",
            vec![t("<urn:a>", "<urn:p>", "<urn:b>")],
            Map::new(),
        ));
        assert_eq!(applied2.added, 0);
        assert_eq!(
            g.generation, gen_after_first,
            "generation changes only on change"
        );
    }

    #[test]
    fn delete_removes_orphan_nodes() {
        let mut g = ViewerGraph::empty();
        g.graph = "g".to_string();
        g.apply_event(&insert_event(
            "g",
            vec![t("<urn:a>", "<urn:p>", "<urn:b>")],
            Map::new(),
        ));
        assert_eq!(g.nodes.iter().filter(|n| n.is_some()).count(), 2);

        g.apply_event(&StoreEvent::Delete {
            graph: "g".to_string(),
            triples: vec![t("<urn:a>", "<urn:p>", "<urn:b>")],
            kinds: Map::new(),
        });
        assert_eq!(
            g.nodes.iter().filter(|n| n.is_some()).count(),
            0,
            "both nodes had mentions=1 and should be removed at 0"
        );
    }

    #[test]
    fn event_for_another_graph_is_ignored() {
        let mut g = ViewerGraph::empty();
        g.graph = "g".to_string();
        let applied = g.apply_event(&insert_event(
            "other-graph",
            vec![t("<urn:a>", "<urn:p>", "<urn:b>")],
            Map::new(),
        ));
        assert!(applied.ignored);
        assert_eq!(g.nodes.len(), 0);
        assert_eq!(
            g.event_seq, 0,
            "an ignored event does not advance event_seq"
        );
    }

    #[test]
    fn label_fallback_order() {
        // rdfs:label wins over smc:label and the IRI local name.
        let snap = snapshot(
            "g",
            vec![
                t("<urn:a>", RDFS_LABEL, "\"RDFS label\""),
                t("<urn:a>", SMC_LABEL, "\"SMC label\""),
            ],
            Map::new(),
        );
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        assert_eq!(g.nodes[0].as_ref().unwrap().label, "RDFS label");

        // smc:label wins over the local name when rdfs:label is absent.
        let snap = snapshot(
            "g",
            vec![t("<urn:b>", SMC_LABEL, "\"SMC only\"")],
            Map::new(),
        );
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        assert_eq!(g.nodes[0].as_ref().unwrap().label, "SMC only");

        // IRI local name, with no label triples at all.
        let snap = snapshot(
            "g",
            vec![t("<urn:ns#LocalName>", "<urn:p>", "\"x\"")],
            Map::new(),
        );
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        assert_eq!(g.nodes[0].as_ref().unwrap().label, "LocalName");

        // Blank-node id, when there's nothing else.
        let snap = snapshot("g", vec![t("_:b0", "<urn:p>", "\"x\"")], Map::new());
        let (g, _) = ViewerGraph::from_snapshot(&snap, None);
        assert_eq!(g.nodes[0].as_ref().unwrap().label, "b0");
    }

    #[test]
    fn generation_changes_only_on_change() {
        let mut g = ViewerGraph::empty();
        g.graph = "g".to_string();
        let gen0 = g.generation;
        g.apply_event(&StoreEvent::Delete {
            graph: "g".to_string(),
            triples: vec![t("<urn:a>", "<urn:p>", "<urn:b>")], // absent — no-op
            kinds: Map::new(),
        });
        assert_eq!(g.generation, gen0);

        g.apply_event(&insert_event(
            "g",
            vec![t("<urn:a>", "<urn:p>", "<urn:b>")],
            Map::new(),
        ));
        assert_eq!(g.generation, gen0 + 1);
    }

    #[test]
    fn same_graph_snapshot_remaps_iri_nodes_and_drops_blank_nodes() {
        let snap1 = snapshot(
            "g",
            vec![
                t("<urn:a>", "<urn:p>", "\"v\""),
                t("_:b0", "<urn:p>", "\"v\""),
            ],
            Map::new(),
        );
        let (g1, _) = ViewerGraph::from_snapshot(&snap1, None);
        let old_id = g1.index["<urn:a>"];

        let snap2 = snapshot(
            "g",
            vec![
                t("<urn:other>", "<urn:p>", "\"v\""),
                t("<urn:a>", "<urn:p>", "\"v2\""),
            ],
            Map::new(),
        );
        let (g2, remap) = ViewerGraph::from_snapshot(&snap2, Some(&g1));
        let remap = remap.expect("same graph must produce a Remap");
        let new_id = g2.index["<urn:a>"];
        assert_eq!(remap.0.get(&old_id), Some(&new_id));
        assert!(
            !remap.0.contains_key(&g1.index["_:b0"]),
            "blank nodes never carry over"
        );
    }

    #[test]
    fn different_graph_snapshot_returns_no_remap() {
        let snap1 = snapshot("g1", vec![t("<urn:a>", "<urn:p>", "\"v\"")], Map::new());
        let (g1, _) = ViewerGraph::from_snapshot(&snap1, None);

        let snap2 = snapshot("g2", vec![t("<urn:a>", "<urn:p>", "\"v\"")], Map::new());
        let (_g2, remap) = ViewerGraph::from_snapshot(&snap2, Some(&g1));
        assert!(remap.is_none());
    }

    #[test]
    fn changed_sets_resync() {
        let mut g = ViewerGraph::empty();
        g.graph = "g".to_string();
        let applied = g.apply_event(&StoreEvent::Changed {
            graph: "g".to_string(),
            triples_before: 5,
            triples_after: 3,
        });
        assert!(applied.resync);
    }
}
