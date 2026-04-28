//! get_gdrive_config tool — returns GDrive sync configuration and store path.

use std::path::PathBuf;

use rmcp::model::{CallToolResult, Content, Tool};
use rmcp::ErrorData as McpError;
use serde_json::json;

use crate::application::tools::sparql::make_tool;
use crate::config::GDriveConfig;

pub fn tool_get_gdrive_config_def() -> Tool {
    make_tool(
        "get_gdrive_config",
        "Return the GDrive sync configuration (folder_id, backup_retain, sync_on_render) \
         and the local store path. Used by the kb sync skill to orchestrate push/pull.",
        json!({ "type": "object", "properties": {} }),
    )
}

pub fn get_gdrive_config(
    gdrive: &Option<GDriveConfig>,
    store_path: &PathBuf,
) -> Result<CallToolResult, McpError> {
    let payload = match gdrive {
        Some(gd) => json!({
            "enabled": gd.enabled,
            "folder_id": gd.folder_id,
            "backup_retain": gd.backup_retain,
            "sync_on_render": gd.sync_on_render,
            "store_path": store_path.to_string_lossy(),
        }),
        None => json!({
            "enabled": false,
            "folder_id": null,
            "backup_retain": 5,
            "sync_on_render": false,
            "store_path": store_path.to_string_lossy(),
        }),
    };
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&payload).unwrap(),
    )]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::RawContent;

    #[test]
    fn returns_config_when_present() {
        let gd = Some(GDriveConfig {
            enabled: true,
            folder_id: Some("abc123".to_string()),
            backup_retain: 3,
            sync_on_render: false,
        });
        let result = get_gdrive_config(&gd, &PathBuf::from("/tmp/store")).unwrap();
        let text = match &result.content[0].raw {
            RawContent::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["folder_id"], "abc123");
        assert_eq!(v["backup_retain"], 3);
        assert_eq!(v["store_path"], "/tmp/store");
    }

    #[test]
    fn returns_defaults_when_absent() {
        let result = get_gdrive_config(&None, &PathBuf::from("/tmp/store")).unwrap();
        let text = match &result.content[0].raw {
            RawContent::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["enabled"], false);
        assert_eq!(v["backup_retain"], 5);
    }
}
