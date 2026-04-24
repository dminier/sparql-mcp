//! MCP client helpers: spawn a stdio-based MCP server as a child process
//! and complete the initialisation handshake.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rmcp::service::RunningService;
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::{RoleClient, ServiceExt};
use tokio::process::Command;

pub type McpService = RunningService<RoleClient, ()>;

/// Spawn an MCP server as a child process over stdio and perform the
/// client-side initialisation handshake.
pub async fn spawn_stdio(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> Result<McpService> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let transport = TokioChildProcess::new(cmd).context("spawning MCP server process")?;
    let service = ().serve(transport).await.context("MCP client handshake")?;
    Ok(service)
}
