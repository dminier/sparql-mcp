//! write_doc tool — write agent-generated markdown to the Docusaurus front.

use std::sync::Arc;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::json;

use crate::domain::DocStore;

use super::sparql::{make_tool, require_str};

pub fn tool_write_doc_def() -> Tool {
    make_tool(
        "write_doc",
        "Write agent-generated markdown to the Docusaurus docs root (hot-reload). \
         `path` is relative to the docs root, e.g. 'architecture/attack-surface.md'. \
         Path traversal is rejected.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path inside the docs root."
                },
                "content": {
                    "type": "string",
                    "description": "Markdown content to write."
                }
            },
            "required": ["path", "content"]
        }),
    )
}

pub fn write_doc(
    doc_store: &Arc<dyn DocStore>,
    args: &JsonObject,
) -> Result<CallToolResult, McpError> {
    let path = require_str(args, "path")?;
    let content = require_str(args, "content")?;

    let dest = doc_store
        .write(path, content)
        .map_err(|e| McpError::invalid_params(format!("write_doc: {e}"), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&json!({
            "status": "ok",
            "path": dest.to_string_lossy(),
            "bytes": content.len(),
        }))
        .unwrap(),
    )]))
}
