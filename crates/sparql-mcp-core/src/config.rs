//! Configuration loaded from `sparql-mcp.toml` at the repository root.
//!
//! The config declares MCP servers that hkb can launch as child processes
//! (stdio transport), plus a few default paths. CLI flags override the
//! values read here.
//!
//! Example `sparql-mcp.toml`:
//!
//! ```toml
//! [defaults]
//! mcp_server = "semantic_code"
//! store      = "./store"
//! ontology   = "./ontology"
//!
//! [mcp.semantic_code]
//! command = "npx"
//! args    = ["-y", "@nka11/semantic-code-mcp"]
//!
//! [mcp.semantic_code.env]
//! # OPTIONAL_VAR = "value"
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

pub const DEFAULT_CONFIG_FILE: &str = "sparql-mcp.toml";
pub const DEFAULT_MCP_JSON_FILE: &str = ".mcp.json";

/// Standard MCP stdio config format, compatible with Claude Code /
/// Claude Desktop `.mcp.json` files:
///
/// ```json
/// {
///   "mcpServers": {
///     "semantic_code": {
///       "command": "npx",
///       "args": ["-y", "@nka11/semantic-code-mcp"],
///       "env": {}
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpJson {
    #[serde(rename = "mcpServers", default)]
    pub servers: BTreeMap<String, McpServer>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,

    /// Named MCP servers, keyed by a short identifier used on the CLI.
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServer>,

    /// Google Drive sync configuration.
    #[serde(default)]
    pub gdrive: Option<GDriveConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Defaults {
    /// Identifier of the MCP server selected when `--mcp` is not passed.
    pub mcp_server: Option<String>,
    pub store: Option<PathBuf>,
    pub ontology: Option<PathBuf>,
    /// v0.2 scaffold — when `true`, each project opens its own store under
    /// `$SPARQL_MCP_HOME/projects/<slug>/store/`. Not yet wired end-to-end.
    #[serde(default)]
    pub per_project_store: bool,
}

fn default_backup_retain() -> usize { 5 }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Deserialize)]
pub struct McpServer {
    /// Executable to launch (e.g. `npx`, an absolute path, ...).
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GDriveConfig {
    #[serde(default)]
    pub enabled: bool,
    pub folder_id: Option<String>,
    #[serde(default = "default_backup_retain")]
    pub backup_retain: usize,
    #[serde(default = "default_true")]
    pub sync_on_render: bool,
}

impl Config {
    /// Read the config from `path`. Missing file yields an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Self = toml::from_str(&text)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        if cfg.defaults.per_project_store {
            tracing::warn!(
                "per_project_store = true: scaffold only (v0.2 feature), flag has no effect yet"
            );
        }
        Ok(cfg)
    }

    /// Merge MCP servers from a standard `.mcp.json` file into this config.
    ///
    /// Servers already declared in `sparql-mcp.toml` take precedence; `.mcp.json`
    /// only fills in names that are not yet defined. Missing file is a
    /// no-op.
    pub fn merge_mcp_json(&mut self, path: &Path) -> Result<usize> {
        if !path.exists() {
            return Ok(0);
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading MCP config file {}", path.display()))?;
        let parsed: McpJson = serde_json::from_str(&text)
            .with_context(|| format!("parsing MCP config file {}", path.display()))?;
        let mut added = 0usize;
        for (name, server) in parsed.servers {
            self.mcp.entry(name).or_insert_with(|| {
                added += 1;
                server
            });
        }
        Ok(added)
    }

    /// Resolve the MCP server to use, given an optional explicit name.
    ///
    /// Falls back to `defaults.mcp_server` and, if exactly one server is
    /// declared, to that one.
    pub fn resolve_mcp(&self, name: Option<&str>) -> Result<(&str, &McpServer)> {
        let key = match name {
            Some(n) => n.to_string(),
            None => match (&self.defaults.mcp_server, self.mcp.len()) {
                (Some(n), _) => n.clone(),
                (None, 1) => self.mcp.keys().next().unwrap().clone(),
                (None, 0) => {
                    return Err(anyhow!(
                        "no MCP server configured; add one under [mcp.<name>] in sparql-mcp.toml \
                         or pass --mcp-cmd/--mcp-arg on the CLI"
                    ))
                }
                (None, _) => {
                    return Err(anyhow!(
                        "multiple MCP servers declared; pick one with --mcp <name> or set \
                         defaults.mcp_server in sparql-mcp.toml"
                    ))
                }
            },
        };
        let server = self
            .mcp
            .get(&key)
            .ok_or_else(|| anyhow!("unknown MCP server '{key}' — not found in sparql-mcp.toml"))?;
        // SAFETY: key is present in the map, so we can safely return a
        // reference to its stored form.
        let stored_key = self.mcp.get_key_value(&key).unwrap().0.as_str();
        Ok((stored_key, server))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdrive_config_deserializes() {
        let toml = r#"
[gdrive]
enabled       = true
folder_id     = "1AbCdEf_testFolder"
backup_retain = 3
sync_on_render = false
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let gd = cfg.gdrive.expect("gdrive section present");
        assert!(gd.enabled);
        assert_eq!(gd.folder_id.as_deref(), Some("1AbCdEf_testFolder"));
        assert_eq!(gd.backup_retain, 3);
        assert!(!gd.sync_on_render);
    }

    #[test]
    fn gdrive_config_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.gdrive.is_none());
    }

    #[test]
    fn gdrive_config_default_values() {
        let toml = "[gdrive]\nenabled = true\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let gd = cfg.gdrive.unwrap();
        assert_eq!(gd.backup_retain, 5);
        assert!(gd.sync_on_render);
        assert!(gd.folder_id.is_none());
    }
}
