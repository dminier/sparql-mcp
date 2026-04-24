//! sparql-mcp MCP stdio server — thin dispatcher.
//!
//! This file contains only wiring: `ServerHandler` implementation, tool
//! registration, and routing.  All business logic lives in `crate::application`.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

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

use crate::application::tools::{doc, export, ontology, project, recording, sparql};
use crate::domain::{DocStore, SparqlStore};
use crate::plugin::{PluginContext, ToolPlugin};

// ── Server struct ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SparqlMcpServer {
    store: Arc<dyn SparqlStore>,
    doc_store: Arc<dyn DocStore>,
    ontology_dir: PathBuf,
    active_graph: String,
    plugins: Arc<Vec<Box<dyn ToolPlugin>>>,
}

impl SparqlMcpServer {
    pub fn new(
        store: Arc<dyn SparqlStore>,
        doc_store: Arc<dyn DocStore>,
        ontology_dir: PathBuf,
        active_graph: String,
    ) -> Self {
        Self {
            store,
            doc_store,
            ontology_dir,
            active_graph,
            plugins: Arc::new(Vec::new()),
        }
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
            active_graph: self.active_graph.clone(),
        }
    }

    // ── Core tool dispatch ────────────────────────────────────────────────────

    fn dispatch_core(
        &self,
        name: &str,
        args: &JsonObject,
    ) -> Option<Result<CallToolResult, McpError>> {
        let result = match name {
            "query_sparql" => sparql::query_sparql(&self.store, args),
            "update_sparql" => sparql::update_sparql(&self.store, args),
            "load_ontology" => ontology::load_ontology(&self.store, args),
            "load_ontology_file" => ontology::load_ontology_file(&self.store, args),
            "export_graph" => export::export_graph(&self.store, args),
            "list_graphs" => export::list_graphs(&self.store),
            "stats" => export::stats(&self.store, &self.ontology_dir),
            "project_create" => project::project_create(&self.store, args),
            "project_list" => project::project_list(&self.store),
            "project_switch" => project::project_switch(&self.store, args),
            "write_doc" => doc::write_doc(&self.doc_store, args),
            "import_recording_navigations" => {
                recording::import_recording_navigations(&self.store, args)
            }
            "import_recording_network" => recording::import_recording_network(&self.store, args),
            "materialize_recording" => recording::materialize_recording(&self.store, args),
            _ => return None,
        };
        Some(result)
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
        let mut tools: Vec<Tool> = vec![
            sparql::tool_query_sparql_def(),
            sparql::tool_update_sparql_def(),
            ontology::tool_load_ontology_def(),
            ontology::tool_load_ontology_file_def(),
            export::tool_export_graph_def(),
            export::tool_list_graphs_def(),
            export::tool_stats_def(),
            project::tool_project_create_def(),
            project::tool_project_list_def(),
            project::tool_project_switch_def(),
            doc::tool_write_doc_def(),
            recording::tool_import_navigations_def(),
            recording::tool_import_network_def(),
            recording::tool_materialize_recording_def(),
        ];

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
        if let Some(result) = self.dispatch_core(name, args) {
            return result;
        }

        // Plugin dispatch — first match wins.
        let ctx = self.plugin_ctx();
        for plugin in self.plugins.iter() {
            if let Some(result) = plugin.call(name, args, &ctx) {
                return result;
            }
        }

        Err(McpError::invalid_params(
            format!("unknown tool: {name}"),
            None,
        ))
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
