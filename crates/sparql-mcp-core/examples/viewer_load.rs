//! Load generator for the viewer: an in-memory store behind the viewer
//! side-channel, fed `ontology/1-smc.ttl` and then typed `smc:Project`
//! instances in batches. Needs no agent and never touches RocksDB.
//!
//! ```text
//! cargo run --release -p sparql-mcp --example viewer_load -- --bind 127.0.0.1:7878 --count 5000
//! sparql-mcp-viewer --connect 127.0.0.1:7878
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::object;
use sparql_mcp::application::tools::{ontology, sparql};
use sparql_mcp::application::BroadcastEventSink;
use sparql_mcp::domain::SparqlStore;
use sparql_mcp::infrastructure::{spawn_viewer_channel, OxigraphAdapter};

const ACTIVE_GRAPH: &str = "urn:project:viewer-load";
const SMC: &str = "https://sparql-mcp.dev/ns#";

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    bind: SocketAddr,
    /// Total instances to insert.
    #[arg(long, default_value_t = 5000)]
    count: usize,
    #[arg(long, default_value_t = 100)]
    batch: usize,
    #[arg(long, default_value_t = 50)]
    interval_ms: u64,
    /// Seconds to wait before inserting, so a viewer can connect first and
    /// watch the instances arrive.
    #[arg(long, default_value_t = 3)]
    delay_secs: u64,
    /// After inserting, remove this many instances with a pattern-based
    /// `DELETE … WHERE` — the server can only report it as a coarse
    /// `changed` event, which makes the viewer resync (CUJ 3).
    #[arg(long, default_value_t = 0)]
    pattern_delete: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory()?);
    let active_graph = Arc::new(RwLock::new(ACTIVE_GRAPH.to_string()));
    let (tx, rx) = tokio::sync::broadcast::channel(1024);
    let sink = BroadcastEventSink(tx);

    tokio::spawn(spawn_viewer_channel(
        args.bind,
        store.clone(),
        active_graph.clone(),
        rx,
    ));

    let ttl_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ontology/1-smc.ttl");
    let turtle = std::fs::read_to_string(&ttl_path)
        .with_context(|| format!("reading {}", ttl_path.display()))?;
    ontology::load_ontology(
        &store,
        &object!({ "turtle": turtle, "graph_iri": ACTIVE_GRAPH }),
        &active_graph,
        &sink,
    )
    .map_err(|e| anyhow::anyhow!("load_ontology: {e:?}"))?;
    println!("serving {ACTIVE_GRAPH} on {} — ontology loaded", args.bind);

    tokio::time::sleep(Duration::from_secs(args.delay_secs)).await;

    let mut inserted = 0;
    while inserted < args.count {
        let n = args.batch.min(args.count - inserted);
        let triples: String = (inserted..inserted + n)
            .map(|i| {
                format!(
                    "<urn:viewer-load:p{i}> a <{SMC}Project> ; <{SMC}label> \"Project {i}\" .\n"
                )
            })
            .collect();
        let update = format!("INSERT DATA {{ GRAPH <{ACTIVE_GRAPH}> {{ {triples} }} }}");
        sparql::update_sparql(&store, &object!({ "update": update }), &active_graph, &sink)
            .map_err(|e| anyhow::anyhow!("update_sparql: {e:?}"))?;
        inserted += n;
        tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
    println!("inserted {inserted} instances");

    let doomed = args.pattern_delete.min(inserted);
    if doomed > 0 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let targets: Vec<String> = (0..doomed)
            .map(|i| format!("<urn:viewer-load:p{i}>"))
            .collect();
        let update = format!(
            "DELETE {{ GRAPH <{ACTIVE_GRAPH}> {{ ?s ?p ?o }} }} \
             WHERE {{ GRAPH <{ACTIVE_GRAPH}> {{ ?s ?p ?o . FILTER(?s IN ({})) }} }}",
            targets.join(", ")
        );
        sparql::update_sparql(&store, &object!({ "update": update }), &active_graph, &sink)
            .map_err(|e| anyhow::anyhow!("update_sparql: {e:?}"))?;
        println!("pattern-deleted {doomed} instances (coarse `changed` event)");
    }
    println!("serving until Ctrl-C");

    tokio::signal::ctrl_c().await?;
    Ok(())
}
