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

    /// Live 3D viewer side-channel configuration.
    #[serde(default)]
    pub viewer: Option<ViewerConfig>,

    /// Write-path limits and guards. The whole section is optional.
    #[serde(default)]
    pub ingest: IngestConfig,
}

/// `[ingest]` — write-path hardening settings (see
/// `docs/designs/sparql-mcp-write-path-hardening.md`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IngestConfig {
    /// How long a write waits for the write gate before failing with
    /// `write_busy`, in seconds.
    pub write_wait_timeout_secs: u64,
    /// Allow `DROP`, `CLEAR`, and delete templates of three variables in
    /// `update_sparql`. Writes to protected graphs and `DROP ALL` stay refused.
    pub allow_destructive_updates: bool,
    /// Largest `update_sparql` request, in bytes.
    pub max_update_bytes: usize,
    /// Largest inline RDF payload (`load_ontology`), in bytes.
    pub max_batch_bytes: usize,
    /// Largest file `load_ontology_file` loads in one transaction, in bytes.
    pub max_load_file_bytes: u64,
    /// Directories that file reads (`load_ontology_file`) and writes
    /// (`export_graph`) are confined to. Unset keeps the old behaviour (any
    /// path) and logs a warning. Each root must exist.
    pub file_roots: Option<Vec<PathBuf>>,
    /// Most triples in one `ingest_batch` payload.
    pub max_batch_triples: usize,
    /// Most batches in one ingestion job.
    pub max_job_batches: u32,
    /// Most triples in one staged job (its commit is one in-memory
    /// transaction). Provisional: confirmed by the bench.
    pub max_job_triples_staged: u64,
    /// Most triples in one direct or file job.
    pub max_job_triples_direct: u64,
    /// Most triples a staged `replace` commit may rewrite (target size plus
    /// staging size). Provisional.
    pub max_replace_triples: u64,
    /// Most jobs that are open, running or interrupted at once.
    pub max_open_jobs: usize,
    /// An open job idle for longer than this is expired, in seconds.
    pub job_idle_ttl_secs: u64,
    /// Finished jobs' ledger entries are deleted this long after they end,
    /// in seconds.
    pub job_retention_secs: u64,
    /// Largest file `ingest_file` accepts, in bytes.
    pub max_file_bytes: u64,
    /// Time limit for one `ingest_validate` check, in seconds.
    pub validate_timeout_secs: u64,
    /// Longest IRI in a payload, in bytes.
    pub max_iri_bytes: usize,
    /// Longest literal in a payload, in bytes (measured on its N-Triples form).
    pub max_literal_bytes: usize,
    /// Most triples in one `Insert`/`Delete` event.
    pub event_max_triples: usize,
    /// More triples than this to one graph are reported as one `Changed`.
    pub event_changed_threshold: usize,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            write_wait_timeout_secs: 30,
            allow_destructive_updates: false,
            max_update_bytes: 1_048_576,
            max_batch_bytes: 1_048_576,
            max_load_file_bytes: 67_108_864,
            file_roots: None,
            max_batch_triples: 10_000,
            max_job_batches: 10_000,
            max_job_triples_staged: 500_000,
            max_job_triples_direct: 10_000_000,
            max_replace_triples: 1_000_000,
            max_open_jobs: 4,
            job_idle_ttl_secs: 3_600,
            job_retention_secs: 604_800,
            max_file_bytes: 2_147_483_648,
            validate_timeout_secs: 10,
            max_iri_bytes: 4_096,
            max_literal_bytes: 65_536,
            event_max_triples: 5_000,
            event_changed_threshold: 50_000,
        }
    }
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

#[derive(Debug, Clone, Deserialize)]
pub struct McpServer {
    /// Executable to launch (e.g. `npx`, an absolute path, ...).
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn default_backup_retain() -> usize {
    5
}
fn default_true() -> bool {
    true
}

fn default_viewer_bind() -> String {
    "127.0.0.1:7878".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_viewer_bind")]
    pub bind: String,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_viewer_bind(),
        }
    }
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

    #[test]
    fn viewer_config_deserializes() {
        let toml = "[viewer]\nenabled = true\nbind = \"127.0.0.1:9000\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let viewer = cfg.viewer.expect("viewer section present");
        assert!(viewer.enabled);
        assert_eq!(viewer.bind, "127.0.0.1:9000");
    }

    #[test]
    fn viewer_config_defaults_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.viewer.is_none());
    }

    #[test]
    fn viewer_config_default_bind() {
        let toml = "[viewer]\nenabled = true\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let viewer = cfg.viewer.unwrap();
        assert_eq!(viewer.bind, "127.0.0.1:7878");
    }

    #[test]
    fn ingest_section_absent_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.ingest.write_wait_timeout_secs, 30);
    }

    #[test]
    fn ingest_section_partial_and_explicit() {
        let cfg: Config = toml::from_str("[ingest]\n").unwrap();
        assert_eq!(cfg.ingest.write_wait_timeout_secs, 30);
        let cfg: Config = toml::from_str("[ingest]\nwrite_wait_timeout_secs = 5\n").unwrap();
        assert_eq!(cfg.ingest.write_wait_timeout_secs, 5);
        assert_eq!(cfg.ingest.max_update_bytes, 1_048_576);
        assert!(!cfg.ingest.allow_destructive_updates);
        let cfg: Config = toml::from_str(
            "[ingest]
allow_destructive_updates = true
max_update_bytes = 10
",
        )
        .unwrap();
        assert!(cfg.ingest.allow_destructive_updates);
        assert_eq!(cfg.ingest.max_update_bytes, 10);
        assert!(cfg.ingest.file_roots.is_none());
        // TOML literal strings (single quotes) keep Windows backslashes.
        let cfg: Config = toml::from_str(
            r"[ingest]
file_roots = ['C:\data\rdf', 'D:/dumps']
",
        )
        .unwrap();
        let roots = cfg.ingest.file_roots.unwrap();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[1], PathBuf::from("D:/dumps"));
    }
}
