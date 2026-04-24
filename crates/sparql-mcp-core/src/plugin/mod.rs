//! Plugin trait — extension point for domain-specific tool sets.
//!
//! Each plugin (domain-specific extension) implements `ToolPlugin` and is
//! registered in `sparql-mcp.toml`.  The MCP server dispatcher calls every
//! registered plugin in order until one handles the tool.

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use rmcp::ErrorData as McpError;

use crate::domain::{DocStore, SparqlStore};

/// Context passed to every plugin call — the only coupling between the plugin
/// and the infrastructure is through the domain port traits.
pub struct PluginContext {
    pub store: Arc<dyn SparqlStore>,
    pub doc_store: Arc<dyn DocStore>,
    /// Active named graph IRI (e.g. `"urn:project:<slug>"`).
    pub active_graph: String,
}

/// A self-contained group of MCP tools.
///
/// Plugins are registered at startup and queried in registration order.
/// The first plugin that returns `Some(result)` from `call` wins.
pub trait ToolPlugin: Send + Sync {
    /// Stable identifier, e.g. `"my-domain"`.
    fn id(&self) -> &'static str;

    /// Tool definitions advertised to MCP clients.
    fn tools(&self) -> Vec<Tool>;

    /// Dispatch a tool call.  Return `None` if this plugin does not handle
    /// the named tool — the dispatcher will try the next plugin.
    fn call(
        &self,
        name: &str,
        args: &JsonObject,
        ctx: &PluginContext,
    ) -> Option<Result<CallToolResult, McpError>>;
}
