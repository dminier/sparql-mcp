//! Viewer side-channel event model.
//!
//! `update_sparql`, `load_ontology` and `load_ontology_file` report what
//! they changed through an [`EventSink`], so `infrastructure::viewer_channel`
//! can forward it to connected viewer clients without those tool functions
//! knowing anything about TCP or JSON wire format.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Full IRIs that mark a subject as a class (schema-level) rather than an
/// instance. Shared by `classify_subjects` here and the viewer's own
/// `graph.rs` classification, so the two sides never disagree.
pub const CLASS_TYPE_IRIS: [&str; 5] = [
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2000/01/rdf-schema#Class",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property",
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
];

/// N-Triples-encoded form of `rdf:type`, to compare directly against an
/// already-encoded predicate string (see the Required Changes note on
/// term encoding).
const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectKind {
    Class,
    Instance,
}

/// Classify every subject appearing in `triples`: `Class` if any triple in
/// the batch types it (via `rdf:type`) as one of [`CLASS_TYPE_IRIS`],
/// `Instance` otherwise. One shared implementation so the snapshot path
/// (`infrastructure::viewer_channel`) and the event path
/// (`application::tools::sparql` / `::ontology`) never classify
/// differently.
pub fn classify_subjects(triples: &[[String; 3]]) -> BTreeMap<String, SubjectKind> {
    let mut kinds: BTreeMap<String, SubjectKind> = BTreeMap::new();
    for [s, p, o] in triples {
        kinds.entry(s.clone()).or_insert(SubjectKind::Instance);
        if p == RDF_TYPE && CLASS_TYPE_IRIS.iter().any(|iri| object_is_iri(o, iri)) {
            kinds.insert(s.clone(), SubjectKind::Class);
        }
    }
    kinds
}

/// `o` is the N-Triples encoding of an IRI term (`<iri>`); compare against
/// a bare IRI from [`CLASS_TYPE_IRIS`].
fn object_is_iri(o: &str, iri: &str) -> bool {
    o.strip_prefix('<').and_then(|s| s.strip_suffix('>')) == Some(iri)
}

/// A snapshot of one named graph's triples, sent on connect (and again
/// after `project_switch` or on resync). Wrapped as `{"snapshot": …}` on
/// the wire by `infrastructure::viewer_channel` — see the Required
/// Changes note on wire types for why this isn't tagged the same way as
/// [`StoreEvent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub graph: String,
    pub triples: Vec<[String; 3]>,
    pub kinds: BTreeMap<String, SubjectKind>,
}

/// One change to the active project graph, emitted once per committed
/// write. Internally tagged (`{"event": "insert", …}`) so a decoder can
/// tell a `Snapshot` line (`{"snapshot": …}`) from a `StoreEvent` line by
/// top-level key alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum StoreEvent {
    Insert {
        graph: String,
        triples: Vec<[String; 3]>,
        kinds: BTreeMap<String, SubjectKind>,
    },
    Delete {
        graph: String,
        triples: Vec<[String; 3]>,
        kinds: BTreeMap<String, SubjectKind>,
    },
    /// Coarse fallback for a pattern-based update whose literal triples
    /// aren't cheaply knowable (see `application::tools::sparql`).
    Changed {
        graph: String,
        triples_before: u64,
        triples_after: u64,
    },
}

/// Receives [`StoreEvent`]s from the application layer. Every write path
/// always has a sink to call — no `Option` check — via [`NullEventSink`]
/// when the `[viewer]` side-channel is disabled.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: StoreEvent);
}

/// Pushes every event onto a `tokio::sync::broadcast` channel. Never
/// blocks the write path: if there are currently no receivers (no viewer
/// connected), the send is a cheap no-op rather than an error.
pub struct BroadcastEventSink(pub tokio::sync::broadcast::Sender<StoreEvent>);

impl EventSink for BroadcastEventSink {
    fn emit(&self, event: StoreEvent) {
        let _ = self.0.send(event);
    }
}

/// Used when `[viewer] enabled` is `false` or absent — today's behavior,
/// unchanged: every write path still has a sink to call, it just does
/// nothing.
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: StoreEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_class_and_instance() {
        let triples = vec![
            [
                "<urn:Person>".to_string(),
                RDF_TYPE.to_string(),
                "<http://www.w3.org/2002/07/owl#Class>".to_string(),
            ],
            [
                "<urn:alice>".to_string(),
                RDF_TYPE.to_string(),
                "<urn:Person>".to_string(),
            ],
        ];
        let kinds = classify_subjects(&triples);
        assert_eq!(kinds.get("<urn:Person>"), Some(&SubjectKind::Class));
        assert_eq!(kinds.get("<urn:alice>"), Some(&SubjectKind::Instance));
    }

    #[test]
    fn store_event_wire_tag() {
        let ev = StoreEvent::Changed {
            graph: "urn:project:default".into(),
            triples_before: 1,
            triples_after: 2,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""event":"changed""#), "{json}");
        let back: StoreEvent = serde_json::from_str(&json).unwrap();
        matches!(back, StoreEvent::Changed { .. });
    }
}
