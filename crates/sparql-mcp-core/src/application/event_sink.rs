//! Viewer side-channel event model.
//!
//! `update_sparql`, `load_ontology` and `load_ontology_file` report what
//! they changed through an [`EventSink`], so `infrastructure::viewer_channel`
//! can forward it to connected viewer clients without those tool functions
//! knowing anything about TCP or JSON wire format.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::StoredQuad;

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

/// Bounds on what one write puts on the broadcast channel (1024 slots), so a
/// large load cannot hold unbounded memory for a slow viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventLimits {
    /// At most this many triples in one `Insert`/`Delete` event.
    pub max_triples: usize,
    /// More triples than this to one graph are reported as a single
    /// `Changed` event instead, and the viewer resyncs from a snapshot.
    pub changed_threshold: usize,
}

impl Default for EventLimits {
    fn default() -> Self {
        Self {
            max_triples: 5_000,
            changed_threshold: 50_000,
        }
    }
}

/// Whether the quads passed to [`emit_quads`] were inserted or deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuadChange {
    Inserted,
    Deleted,
}

/// Report `quads` to `sink`, one stream of events per graph they went to.
///
/// *   Each event names the graph its triples are in. Quads in the default
///     graph are reported with `graph = ""`, which the viewer never shows.
/// *   A graph's triples are split into events of at most
///     `limits.max_triples`.
/// *   Above `limits.changed_threshold` triples to one graph, a single
///     `Changed` event is sent instead. Its counts are the size of this
///     change (`0 → n` for an insert, `n → 0` for a delete), not graph sizes:
///     the viewer only uses it to resync and to label the activity feed.
pub fn emit_quads(
    sink: &dyn EventSink,
    change: QuadChange,
    quads: &[StoredQuad],
    limits: EventLimits,
) {
    let mut by_graph: BTreeMap<String, Vec<[String; 3]>> = BTreeMap::new();
    for q in quads {
        by_graph
            .entry(q.g.clone().unwrap_or_default())
            .or_default()
            .push(q.triple());
    }
    for (graph, triples) in by_graph {
        if triples.len() > limits.changed_threshold {
            let n = triples.len() as u64;
            let (triples_before, triples_after) = match change {
                QuadChange::Inserted => (0, n),
                QuadChange::Deleted => (n, 0),
            };
            sink.emit(StoreEvent::Changed {
                graph,
                triples_before,
                triples_after,
            });
            continue;
        }
        for chunk in triples.chunks(limits.max_triples.max(1)) {
            let kinds = classify_subjects(chunk);
            let triples = chunk.to_vec();
            sink.emit(match change {
                QuadChange::Inserted => StoreEvent::Insert {
                    graph: graph.clone(),
                    triples,
                    kinds,
                },
                QuadChange::Deleted => StoreEvent::Delete {
                    graph: graph.clone(),
                    triples,
                    kinds,
                },
            });
        }
    }
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

    struct Collect(std::sync::Mutex<Vec<StoreEvent>>);
    impl EventSink for Collect {
        fn emit(&self, event: StoreEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn quad(n: usize, g: Option<&str>) -> StoredQuad {
        StoredQuad {
            s: format!("<urn:s:{n}>"),
            p: "<urn:p>".into(),
            o: "<urn:o>".into(),
            g: g.map(str::to_string),
        }
    }

    #[test]
    fn events_name_the_graph_each_quad_went_to() {
        let sink = Collect(Default::default());
        let quads = vec![
            quad(1, Some("urn:project:a")),
            quad(2, Some("urn:project:b")),
            quad(3, None),
        ];
        emit_quads(&sink, QuadChange::Inserted, &quads, EventLimits::default());
        let events = sink.0.lock().unwrap();
        let mut graphs: Vec<_> = events
            .iter()
            .map(|e| match e {
                StoreEvent::Insert { graph, triples, .. } => (graph.clone(), triples.len()),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        graphs.sort();
        assert_eq!(
            graphs,
            vec![
                (String::new(), 1), // default graph
                ("urn:project:a".to_string(), 1),
                ("urn:project:b".to_string(), 1),
            ]
        );
    }

    #[test]
    fn a_large_graph_is_split_into_bounded_events() {
        let sink = Collect(Default::default());
        let quads: Vec<_> = (0..25).map(|n| quad(n, Some("urn:project:a"))).collect();
        let limits = EventLimits {
            max_triples: 10,
            changed_threshold: 100,
        };
        emit_quads(&sink, QuadChange::Deleted, &quads, limits);
        let sizes: Vec<_> = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                StoreEvent::Delete { triples, .. } => triples.len(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(sizes, vec![10, 10, 5]);
    }

    #[test]
    fn above_the_threshold_one_changed_event_replaces_the_stream() {
        let sink = Collect(Default::default());
        let quads: Vec<_> = (0..30).map(|n| quad(n, Some("urn:project:a"))).collect();
        let limits = EventLimits {
            max_triples: 10,
            changed_threshold: 20,
        };
        emit_quads(&sink, QuadChange::Inserted, &quads, limits);
        let events = sink.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            StoreEvent::Changed { graph, triples_before: 0, triples_after: 30 } if graph == "urn:project:a"
        ));
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
