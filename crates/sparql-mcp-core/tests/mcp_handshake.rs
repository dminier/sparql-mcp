//! End-to-end smoke test of the STDIO MCP transport.
//!
//! Spawns the `sparql-mcp serve` binary as a child process, performs the
//! standard MCP `initialize` + `tools/list` handshake, and asserts the
//! server advertises the core tools. Guards against regressions that
//! would break `claude`/`codex` client spawning.

use std::time::Duration;

use serde_json::{json, Value};
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

async fn send(child: &mut Child, msg: &Value) -> anyhow::Result<()> {
    let stdin = child.stdin.as_mut().expect("stdin");
    let line = format!("{}\n", serde_json::to_string(msg)?);
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

async fn recv(reader: &mut BufReader<tokio::process::ChildStdout>) -> anyhow::Result<Value> {
    let mut line = String::new();
    timeout(Duration::from_secs(5), reader.read_line(&mut line)).await??;
    Ok(serde_json::from_str(line.trim())?)
}

#[tokio::test]
async fn initialize_and_list_tools() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let store = tmp.path().join("store");

    let mut child = Command::new(env!("CARGO_BIN_EXE_sparql-mcp"))
        .args([
            "--store",
            store.to_str().unwrap(),
            "--ontology",
            tmp.path().to_str().unwrap(),
            "--docs",
            tmp.path().to_str().unwrap(),
            "serve",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    // 1. initialize
    send(
        &mut child,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "handshake-test", "version": "0"}
            }
        }),
    )
    .await?;
    let init = recv(&mut reader).await?;
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "sparql-mcp");

    // Per MCP spec, the client sends `notifications/initialized` after init.
    send(
        &mut child,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await?;

    // 2. tools/list
    send(
        &mut child,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .await?;
    let list = recv(&mut reader).await?;
    let tools = list["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    for expected in [
        "query_sparql",
        "update_sparql",
        "load_ontology",
        "export_graph",
        "project_create",
        "stats",
    ] {
        assert!(
            names.contains(&expected),
            "tools/list missing '{expected}'; got {names:?}"
        );
    }

    // Clean shutdown — the server exits on stdin close.
    drop(child.stdin.take());
    let _ = timeout(Duration::from_secs(3), child.wait()).await;
    Ok(())
}
