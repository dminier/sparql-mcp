//! Minimal `ToolPlugin` example — exposes a single `hello_echo` MCP tool.
//!
//! To use in your own binary:
//!
//! ```ignore
//! use sparql_mcp::mcp::server::SparqlMcpServer;
//! use hello_plugin::HelloPlugin;
//!
//! let server = SparqlMcpServer::new(store, doc_store, ontology_dir, active_graph)
//!     .with_plugins(vec![Box::new(HelloPlugin)]);
//! server.serve_stdio().await?;
//! ```

use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::json;

use sparql_mcp::plugin::{PluginContext, ToolPlugin};

pub struct HelloPlugin;

impl ToolPlugin for HelloPlugin {
    fn id(&self) -> &'static str {
        "hello"
    }

    fn tools(&self) -> Vec<Tool> {
        let schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to echo back." }
            },
            "required": ["text"]
        }))
        .unwrap();
        // Mutate-from-default so we stay forward-compatible with new
        // optional Tool fields added by upstream rmcp.
        let mut t = Tool::default();
        t.name = "hello_echo".into();
        t.description = Some("Echo the provided text back to the caller.".into());
        t.input_schema = Arc::new(schema);
        vec![t]
    }

    fn call(
        &self,
        name: &str,
        args: &JsonObject,
        _ctx: &PluginContext,
    ) -> Option<Result<CallToolResult, McpError>> {
        if name != "hello_echo" {
            return None;
        }
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return Some(Err(McpError::invalid_params(
                    "hello_echo requires a string `text` argument",
                    None,
                )));
            }
        };
        Some(Ok(CallToolResult::success(vec![Content::text(format!(
            "hello, {text}"
        ))])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::object;
    use sparql_mcp::domain::{DocStore, SparqlStore};
    use sparql_mcp::infrastructure::{FsDocStore, OxigraphAdapter};

    fn ctx() -> PluginContext {
        let store: Arc<dyn SparqlStore> = Arc::new(OxigraphAdapter::open_in_memory().unwrap());
        let doc_store: Arc<dyn DocStore> =
            Arc::new(FsDocStore::new(std::env::temp_dir().join("hello-plugin")));
        PluginContext {
            store,
            doc_store,
            active_graph: "urn:project:default".into(),
        }
    }

    #[test]
    fn echo_returns_greeting() {
        let p = HelloPlugin;
        let args = object!({"text": "world"});
        let out = p.call("hello_echo", &args, &ctx()).unwrap().unwrap();
        let txt = out.content[0].as_text().unwrap().text.as_str();
        assert_eq!(txt, "hello, world");
    }

    #[test]
    fn ignores_unknown_tools() {
        let p = HelloPlugin;
        assert!(p.call("nope", &object!({}), &ctx()).is_none());
    }
}
