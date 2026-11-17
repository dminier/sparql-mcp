use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "sparql-mcp-viewer",
    about = "Live 3D graph viewer for sparql-mcp"
)]
struct Cli {
    /// Connect directly to this address, bypassing config lookup.
    #[arg(long)]
    connect: Option<String>,
    /// `sparql-mcp.toml` to read `[viewer] bind` from. Defaults to
    /// `sparql_mcp::config::DEFAULT_CONFIG_FILE` if that file exists.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Timeout for the initial TCP connect, in seconds.
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,
    /// Run with no window or renderer.
    #[arg(long)]
    headless: bool,
}

fn resolve_addr(cli: &Cli) -> Result<SocketAddr> {
    if let Some(addr) = &cli.connect {
        return addr
            .parse()
            .with_context(|| format!("invalid --connect address {addr:?}"));
    }

    let (config_path, explicit) = match &cli.config {
        Some(path) => (path.clone(), true),
        None => (
            PathBuf::from(sparql_mcp::config::DEFAULT_CONFIG_FILE),
            false,
        ),
    };
    if explicit && !config_path.exists() {
        anyhow::bail!("--config file not found: {}", config_path.display());
    }
    if config_path.exists() {
        let cfg = sparql_mcp::config::Config::load(&config_path)?;
        if let Some(viewer) = cfg.viewer {
            if !viewer.enabled {
                tracing::warn!(
                    path = %config_path.display(),
                    "[viewer] enabled = false; connecting anyway"
                );
            }
            return viewer
                .bind
                .parse()
                .with_context(|| format!("invalid [viewer] bind address {:?}", viewer.bind));
        }
    }

    Ok("127.0.0.1:7878".parse().expect("valid default address"))
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "sparql_mcp_viewer=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let addr = match resolve_addr(&cli) {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let connect_timeout = Duration::from_secs(cli.connect_timeout);

    if !cli.headless {
        eprintln!("error: windowed mode is not implemented yet (Phase 3) — pass --headless");
        return ExitCode::FAILURE;
    }

    match sparql_mcp_viewer::headless::run(addr, connect_timeout) {
        bevy::app::AppExit::Success => ExitCode::SUCCESS,
        bevy::app::AppExit::Error(code) => ExitCode::from(code.get()),
    }
}
