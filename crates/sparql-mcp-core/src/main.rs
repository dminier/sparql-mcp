use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use sparql_mcp::cbm::{db as cbm_db, turtle as cbm_turtle};
use sparql_mcp::config::{Config, McpServer, DEFAULT_CONFIG_FILE, DEFAULT_MCP_JSON_FILE};
use sparql_mcp::domain::{InputFormat, LoadOpts, SparqlStore};
use sparql_mcp::infrastructure::{FsDocStore, OxigraphAdapter};
use sparql_mcp::mcp::{client, importer, server::SparqlMcpServer};

#[derive(Parser)]
#[command(
    name = "sparql-mcp",
    about = "Generic semantic knowledge-management MCP server"
)]
struct Cli {
    #[arg(long, default_value = DEFAULT_CONFIG_FILE, global = true)]
    config: PathBuf,
    #[arg(long, default_value = DEFAULT_MCP_JSON_FILE, global = true)]
    mcp_json: PathBuf,
    #[arg(long, global = true)]
    store: Option<PathBuf>,
    #[arg(long, global = true)]
    ontology: Option<PathBuf>,
    /// Docs root for write_doc (Docusaurus front/docs/).
    #[arg(long, global = true)]
    docs: Option<PathBuf>,
    /// Active project graph IRI (e.g. "urn:project:my-project").
    #[arg(long, default_value = "urn:project:default", global = true)]
    active_graph: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List tools exposed by the configured MCP server.
    Tools(McpArgs),
    /// Import application surface via MCP into the RDF store.
    Import {
        #[command(flatten)]
        mcp: McpArgs,
        #[arg(long)]
        app: String,
        #[arg(long = "pattern", default_values_t = default_patterns())]
        patterns: Vec<String>,
    },
    /// Print basic store statistics.
    Stats,
    /// Re-parse the ontology directory into the store (idempotent).
    ReloadOntology,
    /// Import a CBM (codebase-memory-mcp) SQLite database into the RDF store.
    CodeImport {
        /// Path to the CBM SQLite database (graph.db).
        #[arg(long)]
        db: PathBuf,
        /// Project name inside the DB (auto-detected if only one).
        #[arg(long)]
        cbm_project: Option<String>,
        /// Named graph IRI to import into (default: urn:project:<project-slug>).
        #[arg(long)]
        graph: Option<String>,
        /// Write Turtle to this file instead of loading into the store.
        #[arg(long)]
        output_ttl: Option<PathBuf>,
        /// List projects available in the DB and exit.
        #[arg(long)]
        list_projects: bool,
    },
    /// Load an RDF file (Turtle/NTriples/RDF-XML) directly into the store.
    LoadFile {
        /// Path to the RDF file to load.
        #[arg(long)]
        path: PathBuf,
        /// Named graph IRI (default: active graph).
        #[arg(long)]
        graph: Option<String>,
        /// RDF format: turtle, ntriples, rdfxml (auto-detected from extension if omitted).
        #[arg(long)]
        format: Option<String>,
        /// Re-load even if file was already imported (bypass SHA-256 check).
        #[arg(long)]
        force: bool,
    },
    /// Run as an MCP stdio server.
    Serve,
}

#[derive(clap::Args)]
struct McpArgs {
    #[arg(long)]
    mcp: Option<String>,
    #[arg(long)]
    mcp_cmd: Option<String>,
    #[arg(long = "mcp-arg")]
    mcp_args: Vec<String>,
}

fn default_patterns() -> Vec<String> {
    ["route", "handler", "endpoint", "controller"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn select_mcp(cfg: &Config, args: &McpArgs) -> Result<McpServer> {
    if let Some(cmd) = args.mcp_cmd.as_deref() {
        return Ok(McpServer {
            command: cmd.to_string(),
            args: args.mcp_args.clone(),
            env: BTreeMap::new(),
        });
    }
    let (_name, server) = cfg.resolve_mcp(args.mcp.as_deref())?;
    Ok(server.clone())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "sparql_mcp=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let mut cfg = Config::load(&cli.config)?;
    let merged = cfg.merge_mcp_json(&cli.mcp_json)?;
    if merged > 0 {
        tracing::info!(
            path = %cli.mcp_json.display(),
            added = merged,
            "merged MCP servers from .mcp.json"
        );
    }

    let store_path = cli
        .store
        .or_else(|| cfg.defaults.store.clone())
        .unwrap_or_else(|| PathBuf::from("./store"));
    let ontology_path = cli
        .ontology
        .or_else(|| cfg.defaults.ontology.clone())
        .unwrap_or_else(|| PathBuf::from("./ontology"));
    let docs_path = cli.docs.unwrap_or_else(|| PathBuf::from("./front/docs"));

    // Handle code-import --output-ttl before opening the store (avoids lock contention).
    if let Cmd::CodeImport {
        ref db,
        ref cbm_project,
        ref graph,
        output_ttl: Some(ref out_path),
        list_projects: _,
    } = cli.cmd
    {
        let kg = cbm_db::load_graph(db, cbm_project.as_deref())?;
        tracing::info!(project = %kg.project.name, nodes = kg.nodes.len(), edges = kg.edges.len(), "loaded CBM graph");
        let turtle = cbm_turtle::graph_to_turtle(&kg);
        let slug: String = kg
            .project
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let graph_iri = graph
            .clone()
            .unwrap_or_else(|| format!("urn:project:{slug}"));
        std::fs::write(out_path, turtle.as_bytes())?;
        println!(
            "exported project '{}': {} nodes, {} edges → {} (graph: <{}>)",
            kg.project.name,
            kg.nodes.len(),
            kg.edges.len(),
            out_path.display(),
            graph_iri
        );
        return Ok(());
    }

    let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open(&store_path)?);
    store.load_ontology_dir(&ontology_path)?;

    match cli.cmd {
        Cmd::Tools(mcp) => {
            let server = select_mcp(&cfg, &mcp)?;
            let svc = client::spawn_stdio(&server.command, &server.args, &server.env).await?;
            for t in importer::list_tools(&svc).await? {
                println!("{t}");
            }
        }
        Cmd::Import { mcp, app, patterns } => {
            let server = select_mcp(&cfg, &mcp)?;
            let svc = client::spawn_stdio(&server.command, &server.args, &server.env).await?;
            let refs: Vec<&str> = patterns.iter().map(String::as_str).collect();
            let endpoints = importer::discover_endpoints(&svc, &refs).await?;
            importer::write_endpoints_sparql(&store, &app, &endpoints)?;
            println!(
                "imported {} endpoint(s); triples: {}",
                endpoints.len(),
                store.triple_count()?
            );
        }
        Cmd::Stats => {
            println!("triples: {}", store.triple_count()?);
        }
        Cmd::CodeImport {
            db,
            cbm_project,
            graph,
            output_ttl,
            list_projects,
        } => {
            if list_projects {
                for p in cbm_db::list_projects(&db)? {
                    println!("{p}");
                }
                return Ok(());
            }
            let kg = cbm_db::load_graph(&db, cbm_project.as_deref())?;
            tracing::info!(
                project = %kg.project.name,
                nodes = kg.nodes.len(),
                edges = kg.edges.len(),
                "loaded CBM graph"
            );
            let turtle = cbm_turtle::graph_to_turtle(&kg);
            let slug: String = kg
                .project
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let graph_iri = graph.unwrap_or_else(|| format!("urn:project:{slug}"));

            if let Some(out_path) = output_ttl {
                std::fs::write(&out_path, turtle.as_bytes())?;
                println!(
                    "exported project '{}': {} nodes, {} edges → {} (graph: <{}>)",
                    kg.project.name,
                    kg.nodes.len(),
                    kg.edges.len(),
                    out_path.display(),
                    graph_iri
                );
            } else {
                let opts = LoadOpts {
                    format: InputFormat::Turtle,
                    graph_iri: Some(graph_iri.clone()),
                    base_iri: None,
                };
                let triples = store.load_rdf(turtle.as_bytes(), opts)?;
                println!(
                    "imported project '{}': {} nodes, {} edges → {} triples in <{}>",
                    kg.project.name,
                    kg.nodes.len(),
                    kg.edges.len(),
                    triples,
                    graph_iri
                );
            }
        }
        Cmd::LoadFile {
            path,
            graph,
            format,
            force,
        } => {
            let fmt = format
                .as_deref()
                .and_then(InputFormat::from_label)
                .unwrap_or_else(|| InputFormat::guess_from_path(&path.to_string_lossy()));
            let graph_iri = graph.or_else(|| Some(cli.active_graph.clone()));
            let opts = LoadOpts {
                format: fmt,
                graph_iri,
                base_iri: None,
            };
            let result = store.load_rdf_file(&path, opts, force)?;
            println!("{result:?}");
        }
        Cmd::ReloadOntology => {
            let n = store.load_ontology_dir(&ontology_path)?;
            println!(
                "reloaded {n} ontology file(s); triples: {}",
                store.triple_count()?
            );
        }
        Cmd::Serve => {
            let doc_store = Arc::new(FsDocStore::new(docs_path));
            let srv = SparqlMcpServer::new(store, doc_store, ontology_path, cli.active_graph);
            srv.serve_stdio().await?;
        }
    }

    Ok(())
}
