//! sparql-mcp MCP stdio server — thin dispatcher.
//!
//! This file contains only wiring: `ServerHandler` implementation, tool
//! registration, and routing.  All business logic lives in `crate::application`.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, Result};
use rmcp::model::{
    Annotated, CallToolRequestParams, CallToolResult, Implementation, InitializeResult, JsonObject,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, RawResource,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
    ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer, RunningService};
use rmcp::transport::stdio;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};

use crate::application::ingest::IngestService;
use crate::application::limits::{clear_captured, take_captured};
#[cfg(feature = "recording")]
use crate::application::tools::recording;
use crate::application::tools::{cbm, doc, export, gdrive, ingest, ontology, project, sparql};
use crate::application::{EventSink, NullEventSink, PrivilegedStore};
use crate::config::IngestConfig;
use crate::domain::{DocStore, SparqlStore};
use crate::plugin::{PluginContext, ToolPlugin};

// ── Server struct ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SparqlMcpServer {
    /// Write-gated store handle: what every tool and plugin receives.
    store: Arc<dyn SparqlStore>,
    /// Handle for core modules that own the registry and ledger graphs
    /// (`project_create`, recording imports).
    privileged: Arc<PrivilegedStore>,
    /// The batch ingestion jobs (`ingest_*` tools).
    ingest: Arc<IngestService>,
    /// The `[ingest]` settings the service was built from.
    ingest_cfg: IngestConfig,
    doc_store: Arc<dyn DocStore>,
    ontology_dir: PathBuf,
    /// Shared with `infrastructure::viewer_channel`: `project_switch`
    /// writes here, and the viewer channel re-snapshots when it changes
    /// (Required Change 5 in the viewer design).
    active_graph: Arc<RwLock<String>>,
    plugins: Arc<Vec<Box<dyn ToolPlugin>>>,
    gdrive_config: Option<crate::config::GDriveConfig>,
    store_path: std::path::PathBuf,
    event_sink: Arc<dyn EventSink>,
}

impl SparqlMcpServer {
    /// `gated` is the handle every tool and plugin receives. `privileged`
    /// is for the tools that own the registry and ledger graphs. Both come
    /// from `application::build_stores`, so they share one write gate.
    pub fn new(
        gated: Arc<dyn SparqlStore>,
        privileged: Arc<PrivilegedStore>,
        doc_store: Arc<dyn DocStore>,
        ontology_dir: PathBuf,
        active_graph: Arc<RwLock<String>>,
        gdrive_config: Option<crate::config::GDriveConfig>,
        store_path: PathBuf,
    ) -> Self {
        let ingest_cfg = IngestConfig::default();
        let ingest = Arc::new(IngestService::new(
            privileged.clone(),
            active_graph.clone(),
            ingest_cfg.clone(),
            Arc::new(NullEventSink),
        ));
        Self {
            store: gated,
            privileged,
            ingest,
            ingest_cfg,
            doc_store,
            ontology_dir,
            active_graph,
            plugins: Arc::new(Vec::new()),
            gdrive_config,
            store_path,
            event_sink: Arc::new(NullEventSink),
        }
    }

    /// The ingestion service, for tests and tools that need to run its
    /// maintenance with a chosen clock.
    #[doc(hidden)]
    pub fn ingest_service(&self) -> &Arc<IngestService> {
        &self.ingest
    }

    /// Start expiring idle ingestion jobs and purging old ones. Call once the
    /// server is fully configured; keep the handle for as long as the server
    /// runs, because dropping it stops the maintenance.
    pub fn start_ingest_maintenance(&self) -> crate::application::ingest::Maintenance {
        self.ingest.start_maintenance()
    }

    /// Use the `[ingest]` settings for the ingestion jobs' limits.
    pub fn with_ingest_config(mut self, cfg: IngestConfig) -> Self {
        self.ingest_cfg = cfg;
        self.rebuild_ingest();
        self
    }

    pub fn with_event_sink(mut self, event_sink: Arc<dyn EventSink>) -> Self {
        self.event_sink = event_sink;
        self.rebuild_ingest();
        self
    }

    /// The ingestion service holds the settings and the event sink, so it is
    /// rebuilt when either changes.
    fn rebuild_ingest(&mut self) {
        self.ingest = Arc::new(IngestService::new(
            self.privileged.clone(),
            self.active_graph.clone(),
            self.ingest_cfg.clone(),
            self.event_sink.clone(),
        ));
    }

    pub fn with_plugins(mut self, plugins: Vec<Box<dyn ToolPlugin>>) -> Self {
        self.plugins = Arc::new(plugins);
        self
    }

    pub async fn serve_stdio(self) -> Result<()> {
        let service: RunningService<RoleServer, Self> = self
            .serve(stdio())
            .await
            .context("starting MCP stdio server")?;
        service.waiting().await?;
        Ok(())
    }

    fn plugin_ctx(&self) -> PluginContext {
        PluginContext {
            store: self.store.clone(),
            doc_store: self.doc_store.clone(),
            active_graph: self
                .active_graph
                .read()
                .expect("active_graph lock poisoned")
                .clone(),
        }
    }

    // ── Core tool dispatch ────────────────────────────────────────────────────

    /// Run a core tool on a blocking thread, so store work (reads and
    /// writes) never occupies a Tokio worker. Returns `None` for a name that
    /// is not a core tool.
    ///
    /// A refusal raised below the tool (`write_busy`, …) comes back as an
    /// `isError: true` result with a JSON payload, not a JSON-RPC error.
    #[doc(hidden)] // public only so integration tests can drive tools without an MCP peer
    pub async fn dispatch_core(
        &self,
        name: &str,
        args: &JsonObject,
    ) -> Option<Result<CallToolResult, McpError>> {
        let me = self.clone();
        let name = name.to_string();
        let args = args.clone();
        let joined = tokio::task::spawn_blocking(move || {
            clear_captured();
            let result = me.dispatch_core_blocking(&name, &args);
            refusal_as_result(result)
        })
        .await;
        match joined {
            Ok(result) => result,
            Err(e) => Some(Err(McpError::internal_error(
                format!("tool task failed: {e}"),
                None,
            ))),
        }
    }

    /// The synchronous dispatch table. Only called from a blocking thread.
    fn dispatch_core_blocking(
        &self,
        name: &str,
        args: &JsonObject,
    ) -> Option<Result<CallToolResult, McpError>> {
        let result = match name {
            "query_sparql" => sparql::query_sparql(&self.store, args),
            "update_sparql" => sparql::update_sparql(
                &self.store,
                args,
                &self.active_graph,
                self.event_sink.as_ref(),
            ),
            "load_ontology" => ontology::load_ontology(
                &self.store,
                args,
                &self.active_graph,
                self.event_sink.as_ref(),
            ),
            "load_ontology_file" => ontology::load_ontology_file(
                &self.store,
                args,
                &self.active_graph,
                self.event_sink.as_ref(),
            ),
            "cbm_load_graph" => cbm::cbm_load_graph(
                &self.store,
                args,
                &self.active_graph,
                self.event_sink.as_ref(),
            ),
            "ingest_begin" => ingest::ingest_begin(&self.ingest, args),
            "ingest_batch" => ingest::ingest_batch(&self.ingest, args),
            "ingest_commit" => ingest::ingest_commit(&self.ingest, args),
            "ingest_status" => ingest::ingest_status(&self.ingest, args),
            "ingest_abort" => ingest::ingest_abort(&self.ingest, args),
            "ingest_validate" => ingest::ingest_validate(&self.ingest, args),
            "export_graph" => export::export_graph(&self.store, args),
            "list_graphs" => export::list_graphs(&self.store),
            "stats" => export::stats(&self.store, &self.ontology_dir),
            "project_create" => {
                let privileged: Arc<dyn SparqlStore> = self.privileged.clone();
                project::project_create(&privileged, args)
            }
            "project_list" => project::project_list(&self.store),
            "project_switch" => project::project_switch(&self.active_graph, args),
            "write_doc" => doc::write_doc(&self.doc_store, args),
            "get_gdrive_config" => gdrive::get_gdrive_config(&self.gdrive_config, &self.store_path),
            // Recording imports run a `CLEAR` followed by loads: hold the gate
            // once for the whole import so other writers cannot interleave.
            #[cfg(feature = "recording")]
            "import_recording_navigations" => flatten_exclusive(
                self.privileged
                    .exclusive("import_recording_navigations", |store| {
                        recording::import_recording_navigations(store, args)
                    }),
            ),
            #[cfg(feature = "recording")]
            "import_recording_network" => flatten_exclusive(
                self.privileged
                    .exclusive("import_recording_network", |store| {
                        recording::import_recording_network(store, args)
                    }),
            ),
            #[cfg(feature = "recording")]
            "materialize_recording" => {
                flatten_exclusive(self.privileged.exclusive("materialize_recording", |store| {
                    recording::materialize_recording(store, args)
                }))
            }
            _ => return None,
        };
        Some(result)
    }
}

/// Turn a refusal raised below the tool into an `isError` tool result.
/// `result` is what the tool returned; if it failed and a `ToolError` was
/// recorded on this thread meanwhile, that error wins.
fn refusal_as_result(
    result: Option<Result<CallToolResult, McpError>>,
) -> Option<Result<CallToolResult, McpError>> {
    match result {
        Some(Err(e)) => match take_captured() {
            Some(refusal) => Some(Ok(refusal.to_result())),
            None => Some(Err(e)),
        },
        other => other,
    }
}

/// Flatten the result of `PrivilegedStore::exclusive`: the outer error is the
/// gate failing (`write_busy`), the inner one is the tool's own.
#[cfg(feature = "recording")]
fn flatten_exclusive(
    outcome: anyhow::Result<Result<CallToolResult, McpError>>,
) -> Result<CallToolResult, McpError> {
    match outcome {
        Ok(tool_result) => tool_result,
        Err(gate_error) => Err(McpError::internal_error(gate_error.to_string(), None)),
    }
}

// ── ServerHandler ─────────────────────────────────────────────────────────────

impl ServerHandler for SparqlMcpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("sparql-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "sparql-mcp knowledge base. Use query_sparql / update_sparql for SPARQL 1.1, \
             load_ontology to push Turtle, project_* to manage project isolation, \
             write_doc to produce Docusaurus pages.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools: Vec<Tool> = core_tool_defs();

        let _ctx = self.plugin_ctx();
        for plugin in self.plugins.iter() {
            tools.extend(plugin.tools());
        }

        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let empty = JsonObject::new();
        let args = request.arguments.as_ref().unwrap_or(&empty);
        let name = request.name.as_ref();

        // Core tools take priority.
        if let Some(result) = self.dispatch_core(name, args).await {
            return result;
        }

        // Plugin dispatch — first match wins. Plugins call the (gated) store
        // synchronously, so the whole loop runs on one blocking thread.
        let plugins = self.plugins.clone();
        let ctx = self.plugin_ctx();
        let (plugin_name, plugin_args) = (name.to_string(), args.clone());
        let joined = tokio::task::spawn_blocking(move || {
            clear_captured();
            let result = plugins
                .iter()
                .find_map(|plugin| plugin.call(&plugin_name, &plugin_args, &ctx));
            refusal_as_result(result)
        })
        .await;
        match joined {
            Ok(Some(result)) => result,
            Ok(None) => Err(McpError::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(
                format!("plugin task failed: {e}"),
                None,
            )),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = list_ttl_resources(&self.ontology_dir)
            .map_err(|e| McpError::internal_error(format!("listing ontology: {e}"), None))?;
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let path = parse_file_uri(&request.uri)
            .ok_or_else(|| McpError::invalid_params("uri must be file://…", None))?;

        // Path traversal protection.
        let root = self
            .ontology_dir
            .canonicalize()
            .unwrap_or_else(|_| self.ontology_dir.clone());
        let canonical = path
            .canonicalize()
            .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
        if !canonical.starts_with(&root) {
            return Err(McpError::invalid_params(
                "resource outside the ontology directory",
                None,
            ));
        }

        let text = fs::read_to_string(&canonical)
            .map_err(|e| McpError::internal_error(format!("read: {e}"), None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::TextResourceContents {
                uri: request.uri,
                mime_type: Some("text/turtle".into()),
                text,
                meta: None,
            },
        ]))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_file_uri(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(PathBuf::from)
}

fn list_ttl_resources(dir: &std::path::Path) -> anyhow::Result<Vec<Resource>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut resources = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ttl") {
            continue;
        }
        let uri = format!("file://{}", path.display());
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ontology.ttl".to_string());
        let size = fs::metadata(&path).ok().map(|m| m.len() as u32);
        resources.push(Annotated::new(
            RawResource {
                uri,
                name,
                title: None,
                description: Some("sparql-mcp ontology Turtle file".into()),
                mime_type: Some("text/turtle".into()),
                size,
                icons: None,
                meta: None,
            },
            None,
        ));
    }
    resources.sort_by(|a, b| a.raw.name.cmp(&b.raw.name));
    Ok(resources)
}

/// All MCP tool definitions served by the core (no plugins).
///
/// Kept next to `SparqlMcpServer::dispatch_core` so the two lists stay in
/// sync — a unit test cross-checks that every declared tool has a dispatch
/// arm.
pub(crate) fn core_tool_defs() -> Vec<rmcp::model::Tool> {
    #[allow(unused_mut)]
    let mut defs = vec![
        crate::application::tools::sparql::tool_query_sparql_def(),
        crate::application::tools::sparql::tool_update_sparql_def(),
        crate::application::tools::ontology::tool_load_ontology_def(),
        crate::application::tools::ontology::tool_load_ontology_file_def(),
        crate::application::tools::cbm::tool_cbm_load_graph_def(),
        crate::application::tools::export::tool_export_graph_def(),
        crate::application::tools::export::tool_list_graphs_def(),
        crate::application::tools::export::tool_stats_def(),
        crate::application::tools::project::tool_project_create_def(),
        crate::application::tools::project::tool_project_list_def(),
        crate::application::tools::project::tool_project_switch_def(),
        crate::application::tools::doc::tool_write_doc_def(),
        crate::application::tools::gdrive::tool_get_gdrive_config_def(),
        crate::application::tools::ingest::tool_ingest_begin_def(),
        crate::application::tools::ingest::tool_ingest_batch_def(),
        crate::application::tools::ingest::tool_ingest_commit_def(),
        crate::application::tools::ingest::tool_ingest_status_def(),
        crate::application::tools::ingest::tool_ingest_abort_def(),
        crate::application::tools::ingest::tool_ingest_validate_def(),
    ];
    #[cfg(feature = "recording")]
    {
        defs.push(crate::application::tools::recording::tool_import_navigations_def());
        defs.push(crate::application::tools::recording::tool_import_network_def());
        defs.push(crate::application::tools::recording::tool_materialize_recording_def());
    }
    defs
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::domain::SparqlStore;
    use crate::infrastructure::{FsDocStore, OxigraphAdapter};
    use rmcp::object;
    use std::sync::Arc;

    fn build_server() -> SparqlMcpServer {
        let raw: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
        let gate = crate::application::WriteGate::new(std::time::Duration::from_secs(5));
        let (gated, privileged) = crate::application::build_stores(raw, gate);
        let doc_store = Arc::new(FsDocStore::new(std::env::temp_dir().join("smcp-disp")));
        SparqlMcpServer::new(
            gated,
            privileged,
            doc_store,
            std::path::PathBuf::from("./ontology"),
            Arc::new(RwLock::new("urn:project:default".into())),
            None,
            std::path::PathBuf::from("/tmp/smcp-test-store"),
        )
    }

    #[tokio::test]
    async fn every_declared_tool_has_a_dispatch_arm() {
        let server = build_server();
        let empty = object!({});
        let mut missing: Vec<String> = Vec::new();
        for t in core_tool_defs() {
            let name = t.name.as_ref();
            if server.dispatch_core(name, &empty).await.is_none() {
                missing.push(name.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "these tools are listed by list_tools but have no dispatch_core arm: {missing:?}"
        );
    }
}
