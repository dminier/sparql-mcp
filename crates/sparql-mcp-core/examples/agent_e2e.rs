//! A scripted test agent: an MCP client that drives the **real**
//! `sparql-mcp serve` process over stdio, exactly like Claude Code would.
//!
//! It plays a short scenario (build a small knowledge graph, then try the
//! operations the write guards must refuse) and checks every step. It also
//! connects a plain TCP client to the viewer side-channel and verifies that
//! the writes arrive as events, so the full path is covered without a window.
//! Start `sparql-mcp-viewer` first to watch it happen.
//!
//! ```text
//! cargo build -p sparql-mcp
//! cargo run -p sparql-mcp-viewer -- --connect 127.0.0.1:7878     # optional, to watch
//! cargo run -p sparql-mcp --example agent_e2e -- --pause-ms 800 --hold-secs 60
//! ```
//!
//! Exit code 0 when every check passed, 1 otherwise.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use rmcp::model::CallToolRequestParams;
use serde_json::{json, Value};
use sparql_mcp::mcp::client::{spawn_stdio, McpService};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[derive(Parser)]
struct Args {
    /// The server binary. Defaults to `target/debug/sparql-mcp[.exe]`.
    #[arg(long)]
    server: Option<PathBuf>,
    /// Viewer side-channel address the server is told to bind.
    #[arg(long, default_value = "127.0.0.1:7878")]
    bind: SocketAddr,
    /// Instances to create, in batches of 10.
    #[arg(long, default_value_t = 60)]
    instances: usize,
    /// Pause between steps, so a viewer can be watched.
    #[arg(long, default_value_t = 600)]
    pause_ms: u64,
    /// Keep the server (and the viewer connection) up this long at the end.
    #[arg(long, default_value_t = 0)]
    hold_secs: u64,
    /// After the scenario, show only this class (and its subclasses) in the
    /// viewer, using what exists today: copy that sub-graph into its own
    /// project and `project_switch` to it. Example: `--show-only Database`.
    #[arg(long)]
    show_only: Option<String>,
}

const GRAPH: &str = "urn:project:e2e";
const NS: &str = "https://example.org/e2e#";

struct Checks {
    passed: usize,
    failed: Vec<String>,
}

impl Checks {
    fn check(&mut self, name: &str, ok: bool, detail: impl FnOnce() -> String) {
        if ok {
            self.passed += 1;
            println!("  ok    {name}");
        } else {
            let msg = format!("{name}: {}", detail());
            println!("  FAIL  {msg}");
            self.failed.push(msg);
        }
    }
}

/// Call a tool; return `(is_error, parsed body)`.
async fn call(svc: &McpService, tool: &str, args: Value) -> Result<(bool, Value)> {
    let mut params = CallToolRequestParams::default();
    params.name = tool.to_string().into();
    params.arguments = args.as_object().cloned();
    let res = svc
        .call_tool(params)
        .await
        .with_context(|| format!("calling {tool}"))?;
    let is_error = res.is_error.unwrap_or(false);
    let text = res
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok((is_error, body))
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 140 {
        format!("{}…", &s[..140])
    } else {
        s
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let pause = Duration::from_millis(args.pause_ms);

    // ── A throwaway project directory with a config that enables the viewer.
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("sparql-mcp.toml");
    std::fs::write(
        &config,
        format!("[viewer]\nenabled = true\nbind = \"{}\"\n", args.bind),
    )?;
    std::fs::create_dir_all(dir.path().join("ontology"))?;

    let server = args.server.unwrap_or_else(|| {
        let exe = if cfg!(windows) {
            "sparql-mcp.exe"
        } else {
            "sparql-mcp"
        };
        PathBuf::from("target").join("debug").join(exe)
    });
    if !server.exists() {
        bail!(
            "{} not found; run `cargo build -p sparql-mcp` first",
            server.display()
        );
    }
    let server_args: Vec<String> = [
        "--config",
        config.to_str().unwrap(),
        "--store",
        dir.path().join("store").to_str().unwrap(),
        "--ontology",
        dir.path().join("ontology").to_str().unwrap(),
        "--docs",
        dir.path().join("docs").to_str().unwrap(),
        "--active-graph",
        GRAPH,
        "serve",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    println!("starting {} …", server.display());
    let svc = spawn_stdio(server.to_str().unwrap(), &server_args, &BTreeMap::new()).await?;
    let mut checks = Checks {
        passed: 0,
        failed: Vec::new(),
    };

    // ── A plain client of the viewer side-channel: counts what the server pushes.
    let events: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = events.clone();
    let bind = args.bind;
    let listener = tokio::spawn(async move {
        for _ in 0..50 {
            if let Ok(mut stream) = TcpStream::connect(bind).await {
                let _ = stream.write_all(b"{\"cmd\":\"snapshot\"}\n").await;
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        sink.lock().unwrap().push(v);
                    }
                }
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });

    // ── 1. The server is up and exposes the tools.
    println!("\n[1] handshake");
    let tools = svc.list_all_tools().await?;
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    checks.check(
        "tools listed",
        names.contains(&"update_sparql".to_string()),
        || format!("{names:?}"),
    );

    // ── 2. Create and switch to a project.
    println!("\n[2] project");
    let (err, body) = call(
        &svc,
        "project_create",
        json!({ "id": "e2e", "label": "E2E demo" }),
    )
    .await?;
    checks.check("project_create", !err, || short(&body));
    let (err, body) = call(&svc, "project_switch", json!({ "id": "e2e" })).await?;
    checks.check(
        "project_switch",
        !err && body.to_string().contains(GRAPH),
        || short(&body),
    );
    tokio::time::sleep(pause).await;

    // ── 3. A small ontology: classes and subclass links.
    println!("\n[3] ontology");
    let ontology = format!(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix e: <{NS}> .\n\
         e:Service a owl:Class ; rdfs:label \"Service\" .\n\
         e:Database a owl:Class ; rdfs:label \"Database\" .\n\
         e:Queue a owl:Class ; rdfs:label \"Queue\" .\n\
         e:Cache a owl:Class ; rdfs:subClassOf e:Database ; rdfs:label \"Cache\" .\n"
    );
    let (err, body) = call(&svc, "load_ontology", json!({ "turtle": ontology })).await?;
    checks.check(
        "load_ontology into the active graph",
        !err && body["graph_iri"] == GRAPH && body["inserted_new"] == 9,
        || short(&body),
    );
    tokio::time::sleep(pause).await;

    // ── 4. Instances, in batches (the viewer grows the graph batch by batch).
    println!("\n[4] instances");
    let kinds = ["Service", "Database", "Queue", "Cache"];
    let mut made = 0;
    while made < args.instances {
        let upto = (made + 10).min(args.instances);
        let mut triples = String::new();
        for i in made..upto {
            let kind = kinds[i % kinds.len()];
            triples.push_str(&format!(
                "<{NS}n{i}> a <{NS}{kind}> ; <http://www.w3.org/2000/01/rdf-schema#label> \"{kind} {i}\" . \
                 <{NS}n{i}> <{NS}dependsOn> <{NS}n{}> . ",
                i / 2
            ));
        }
        let update = format!("INSERT DATA {{ GRAPH <{GRAPH}> {{ {triples} }} }}");
        let (err, body) = call(&svc, "update_sparql", json!({ "update": update })).await?;
        checks.check(
            &format!("batch {made}-{upto}"),
            !err && body["inserted_new"].as_u64().unwrap_or(0) > 0,
            || short(&body),
        );
        made = upto;
        tokio::time::sleep(pause).await;
    }
    let (err, body) = call(&svc, "query_sparql", json!({ "query":
        format!("SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {{ GRAPH <{GRAPH}> {{ ?s a ?c . FILTER(STRSTARTS(STR(?s), \"{NS}n\")) }} }}") })).await?;
    checks.check(
        "instances queryable",
        !err && body.to_string().contains(&args.instances.to_string()),
        || short(&body),
    );

    // ── 5. A pattern-based update: the server can only report it as `changed`.
    println!("\n[5] pattern delete (the viewer resyncs)");
    let del = format!("DELETE {{ GRAPH <{GRAPH}> {{ ?s <{NS}dependsOn> ?o }} }} WHERE {{ GRAPH <{GRAPH}> {{ ?s <{NS}dependsOn> ?o . FILTER(?s = <{NS}n3>) }} }}");
    let (err, body) = call(&svc, "update_sparql", json!({ "update": del })).await?;
    checks.check(
        "pattern delete",
        !err && body["triples_after"].is_number(),
        || short(&body),
    );
    tokio::time::sleep(pause).await;

    // ── 6. What the write guards must refuse (each is a tool result, not a crash).
    println!("\n[6] guards");
    let refused = |is_err: bool, body: &Value, code: &str| is_err && body["code"] == code;

    let (err, body) = call(&svc, "update_sparql", json!({ "update": "DROP ALL" })).await?;
    checks.check(
        "DROP ALL refused",
        refused(err, &body, "graph_protected"),
        || short(&body),
    );

    let (err, body) = call(
        &svc,
        "update_sparql",
        json!({ "update": format!("DROP GRAPH <{GRAPH}>") }),
    )
    .await?;
    checks.check(
        "DROP GRAPH needs allow_destructive_updates",
        refused(err, &body, "destructive_update_forbidden"),
        || short(&body),
    );

    let (err, body) = call(
        &svc,
        "update_sparql",
        json!({ "update":
        "INSERT DATA { GRAPH <urn:meta> { <urn:x> <urn:p> <urn:o> } }" }),
    )
    .await?;
    checks.check(
        "the registry graph is protected",
        refused(err, &body, "graph_protected"),
        || short(&body),
    );

    let (err, body) = call(
        &svc,
        "update_sparql",
        json!({ "update": "LOAD <http://example.com/x.ttl>" }),
    )
    .await?;
    checks.check(
        "LOAD refused",
        refused(err, &body, "load_forbidden"),
        || short(&body),
    );

    let huge = format!(
        "INSERT DATA {{ <urn:s> <urn:p> \"{}\" }}",
        "x".repeat(2 * 1024 * 1024)
    );
    let (err, body) = call(&svc, "update_sparql", json!({ "update": huge })).await?;
    checks.check(
        "a 2 MiB update is refused",
        refused(err, &body, "limit_exceeded") && body["limit_key"] == "max_update_bytes",
        || short(&body),
    );

    let (err, body) = call(
        &svc,
        "update_sparql",
        json!({ "update": format!("DROP GRAPH <{GRAPH}>"), "dry_run": true }),
    )
    .await?;
    checks.check(
        "dry_run reports the refusal without running",
        !err && body["refused"]["code"] == "destructive_update_forbidden",
        || short(&body),
    );

    let (err, body) = call(&svc, "update_sparql", json!({ "update": "INSERT DATA {" })).await?;
    checks.check(
        "a syntax error is a parse_error",
        refused(err, &body, "parse_error"),
        || short(&body),
    );

    // Nothing the guards refused changed the data.
    let (err, body) = call(
        &svc,
        "query_sparql",
        json!({ "query":
        format!("ASK {{ GRAPH <{GRAPH}> {{ <{NS}Service> ?p ?o }} }}") }),
    )
    .await?;
    checks.check(
        "the graph survived",
        !err && body.to_string().contains("true"),
        || short(&body),
    );

    // ── 7. Did the viewer side-channel see the writes?
    println!("\n[7] viewer side-channel");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let seen = events.lock().unwrap().clone();
    let count = |key: &str, val: &str| seen.iter().filter(|e| e[key] == val).count();
    checks.check(
        "snapshot received",
        seen.iter().any(|e| e.get("snapshot").is_some()),
        || format!("{} lines", seen.len()),
    );
    let inserts: Vec<&Value> = seen.iter().filter(|e| e["event"] == "insert").collect();
    checks.check(
        "insert events name the active graph",
        !inserts.is_empty() && inserts.iter().all(|e| e["graph"] == GRAPH),
        || format!("{} inserts", inserts.len()),
    );
    checks.check(
        "the pattern delete became a changed event",
        count("event", "changed") >= 1,
        || format!("{} changed", count("event", "changed")),
    );
    println!(
        "  events seen: {} insert, {} delete, {} changed",
        count("event", "insert"),
        count("event", "delete"),
        count("event", "changed")
    );

    // ── 8. Show only one class in the viewer, driven entirely through MCP.
    if let Some(class) = &args.show_only {
        println!("\n[8] show only {class}");
        let view = "urn:project:e2e-view";
        let (err, body) = call(
            &svc,
            "project_create",
            json!({ "id": "e2e-view", "label": format!("only {class}") }),
        )
        .await?;
        checks.check("view project created", !err, || short(&body));

        let sub = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
        let owl_class = "http://www.w3.org/2002/07/owl#Class";
        let t = format!("<{NS}{class}>");
        // The class, its subclasses, and their instances, with only the edges
        // between them: an edge to anything else would draw a stray node.
        let copy = format!(
            "INSERT {{ GRAPH <{view}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{GRAPH}> {{ ?s ?p ?o . \
             FILTER( EXISTS {{ ?s <{sub}>* {t} }} || EXISTS {{ ?s a ?c . ?c <{sub}>* {t} }} ) \
             FILTER( isLiteral(?o) || ?o = <{owl_class}> || EXISTS {{ ?o <{sub}>* {t} }} \
                     || EXISTS {{ ?o a ?d . ?d <{sub}>* {t} }} ) }} }}"
        );
        let (err, body) = call(&svc, "update_sparql", json!({ "update": copy })).await?;
        checks.check("sub-graph copied", !err, || short(&body));
        let (err, body) = call(&svc, "project_switch", json!({ "id": "e2e-view" })).await?;
        checks.check(
            "switched to the view project",
            !err && body.to_string().contains(view),
            || short(&body),
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // What the viewer was just sent: the last snapshot.
        let seen = events.lock().unwrap().clone();
        let snap = seen.iter().rev().find_map(|e| e.get("snapshot")).cloned();
        let snap = snap.unwrap_or(Value::Null);
        let allowed: &[&str] = match class.as_str() {
            "Database" => &["Database", "Cache"],
            other => &[other],
        };
        let mut shown = 0;
        let mut strays: Vec<String> = Vec::new();
        let mut subjects = std::collections::BTreeSet::new();
        for t in snap["triples"].as_array().into_iter().flatten() {
            if let Some(s) = t[0].as_str() {
                subjects.insert(s.to_string());
            }
        }
        for s in &subjects {
            if let Some(i) = s
                .strip_prefix(&format!("<{NS}n"))
                .and_then(|r| r.strip_suffix('>'))
                .and_then(|r| r.parse::<usize>().ok())
            {
                if allowed.contains(&kinds[i % kinds.len()]) {
                    shown += 1;
                } else {
                    strays.push(s.clone());
                }
            }
        }
        let expected = (0..args.instances)
            .filter(|i| allowed.contains(&kinds[i % kinds.len()]))
            .count();
        checks.check(
            "the viewer got a snapshot of the view graph",
            snap["graph"] == view,
            || format!("graph = {}", snap["graph"]),
        );
        checks.check(
            &format!("it holds only {class} instances"),
            strays.is_empty(),
            || format!("strays: {strays:?}"),
        );
        checks.check(
            &format!("and all {expected} of them"),
            shown == expected,
            || format!("shown {shown}, expected {expected}"),
        );
        println!(
            "  the viewer now shows {shown} {class} instances (of {} in the whole graph)",
            args.instances
        );
    }

    if args.hold_secs > 0 {
        println!(
            "\nholding the server for {} s (Ctrl-C to stop) …",
            args.hold_secs
        );
        tokio::time::sleep(Duration::from_secs(args.hold_secs)).await;
        if args.show_only.is_some() {
            let (err, _) = call(&svc, "project_switch", json!({ "id": "e2e" })).await?;
            println!("switched back to the whole graph (error: {err})");
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
    }
    listener.abort();
    let _ = svc.cancel().await;

    println!(
        "\n{} checks passed, {} failed",
        checks.passed,
        checks.failed.len()
    );
    for f in &checks.failed {
        println!("  - {f}");
    }
    std::process::exit(if checks.failed.is_empty() { 0 } else { 1 });
}
