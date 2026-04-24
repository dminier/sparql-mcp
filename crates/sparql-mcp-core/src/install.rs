//! `sparql-mcp install` — auto-configure MCP entries in detected agent clients.
//!
//! Patches each agent's user-level config with a STDIO entry pointing at the
//! currently running binary. Always merges non-destructively (reads, mutates
//! only our key, writes back), taking a `.bak` side-copy first.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// A single detected + patchable agent.
struct Agent {
    /// Human-readable name used in output.
    name: &'static str,
    /// Path of the config file we'll patch.
    config_path: PathBuf,
    /// Config kind: drives merge strategy.
    kind: Kind,
}

enum Kind {
    /// JSON file with a top-level `mcpServers: { <name>: {...} }` object.
    ClaudeJson,
    /// TOML with `[mcp.<name>]` table (Codex CLI).
    CodexToml,
    /// JSON with `mcpServers: { <name>: {...} }` (Gemini CLI settings).
    GeminiJson,
}

pub struct InstallOpts {
    pub name: String,
    pub yes: bool,
    pub dry_run: bool,
}

pub fn run(opts: InstallOpts) -> Result<()> {
    let bin = std::env::current_exe().context("locating current binary")?;
    let bin = bin
        .canonicalize()
        .unwrap_or(bin)
        .to_string_lossy()
        .to_string();

    let home = dirs_home().context("no HOME directory")?;
    let agents = detect(&home);

    if agents.is_empty() {
        println!("no supported agent config found under {}", home.display());
        println!("supported: Claude Code, Codex CLI, Gemini CLI");
        return Ok(());
    }

    println!("sparql-mcp binary: {bin}");
    println!("will patch {} agent config(s):", agents.len());
    for a in &agents {
        println!("  - {:<14} {}", a.name, a.config_path.display());
    }

    if !opts.yes && !opts.dry_run && !confirm("proceed?") {
        println!("aborted");
        return Ok(());
    }

    for a in &agents {
        if opts.dry_run {
            println!("[dry-run] would patch {}", a.config_path.display());
            continue;
        }
        match patch(a, &opts.name, &bin) {
            Ok(true) => println!("  + {} updated", a.name),
            Ok(false) => println!("  = {} already up-to-date", a.name),
            Err(e) => eprintln!("  ! {} failed: {e:#}", a.name),
        }
    }
    Ok(())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn detect(home: &Path) -> Vec<Agent> {
    let mut out = Vec::new();

    // Claude Code — user-level config. Two known locations; pick whichever
    // exists, else default to ~/.claude.json.
    let claude_json = home.join(".claude.json");
    let claude_settings = home.join(".config").join("claude").join("settings.json");
    let claude = if claude_settings.exists() {
        Some(claude_settings)
    } else {
        Some(claude_json)
    };
    if let Some(p) = claude {
        out.push(Agent {
            name: "Claude Code",
            config_path: p,
            kind: Kind::ClaudeJson,
        });
    }

    // Codex CLI — ~/.codex/config.toml
    let codex = home.join(".codex").join("config.toml");
    if codex.parent().map(Path::exists).unwrap_or(false) {
        out.push(Agent {
            name: "Codex CLI",
            config_path: codex,
            kind: Kind::CodexToml,
        });
    }

    // Gemini CLI — ~/.gemini/settings.json
    let gemini = home.join(".gemini").join("settings.json");
    if gemini.parent().map(Path::exists).unwrap_or(false) {
        out.push(Agent {
            name: "Gemini CLI",
            config_path: gemini,
            kind: Kind::GeminiJson,
        });
    }

    out
}

fn patch(agent: &Agent, name: &str, bin: &str) -> Result<bool> {
    if let Some(parent) = agent.config_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let existed = agent.config_path.exists();
    let before = if existed {
        fs::read_to_string(&agent.config_path)?
    } else {
        String::new()
    };

    let after = match agent.kind {
        Kind::ClaudeJson | Kind::GeminiJson => patch_claude_like(&before, name, bin)?,
        Kind::CodexToml => patch_codex_toml(&before, name, bin)?,
    };

    if after == before {
        return Ok(false);
    }
    if existed {
        fs::copy(
            &agent.config_path,
            agent.config_path.with_extension("json.bak"),
        )
        .or_else(|_| fs::copy(&agent.config_path, append_bak(&agent.config_path)))
        .ok();
    }
    fs::write(&agent.config_path, after)?;
    Ok(true)
}

fn append_bak(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

fn patch_claude_like(before: &str, name: &str, bin: &str) -> Result<String> {
    let mut v: Value = if before.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(before).context("config is not valid JSON")?
    };
    let obj = v.as_object_mut().context("top-level must be an object")?;
    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .context("mcpServers must be an object")?;
    servers.insert(
        name.to_string(),
        json!({ "type": "stdio", "command": bin, "args": ["serve"] }),
    );
    Ok(serde_json::to_string_pretty(&v)? + "\n")
}

fn patch_codex_toml(before: &str, name: &str, bin: &str) -> Result<String> {
    let mut root: toml::Value = if before.trim().is_empty() {
        toml::Value::Table(Default::default())
    } else {
        before.parse().context("config is not valid TOML")?
    };
    let table = root
        .as_table_mut()
        .context("top-level must be a TOML table")?;
    let mcp = table
        .entry("mcp".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .context("[mcp] must be a table")?;
    let mut entry = toml::map::Map::new();
    entry.insert("command".into(), toml::Value::String(bin.to_string()));
    entry.insert(
        "args".into(),
        toml::Value::Array(vec![toml::Value::String("serve".into())]),
    );
    mcp.insert(name.to_string(), toml::Value::Table(entry));
    Ok(toml::to_string_pretty(&root)? + "\n")
}

fn confirm(prompt: &str) -> bool {
    use std::io::{self, BufRead, Write};
    print!("{prompt} [y/N] ");
    io::stdout().flush().ok();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_patch_creates_entry() {
        let out = patch_claude_like("", "sparql-mcp", "/bin/x").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["sparql-mcp"]["type"], "stdio");
        assert_eq!(v["mcpServers"]["sparql-mcp"]["command"], "/bin/x");
    }

    #[test]
    fn claude_patch_preserves_others() {
        let existing = r#"{"mcpServers":{"other":{"command":"foo"}},"theme":"dark"}"#;
        let out = patch_claude_like(existing, "sparql-mcp", "/bin/x").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "foo");
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["sparql-mcp"]["command"], "/bin/x");
    }

    #[test]
    fn codex_toml_roundtrips() {
        let out = patch_codex_toml("", "sparql-mcp", "/bin/x").unwrap();
        assert!(out.contains("[mcp.sparql-mcp]"));
        assert!(out.contains("/bin/x"));
        let reparsed: toml::Value = out.parse().unwrap();
        assert_eq!(
            reparsed["mcp"]["sparql-mcp"]["command"].as_str().unwrap(),
            "/bin/x"
        );
    }
}
