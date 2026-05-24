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

    if let Err(e) = install_desktop(&bin, opts.dry_run) {
        eprintln!("  ! desktop launcher failed: {e:#}");
    }
    Ok(())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
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
        // Always append .bak (never replace the real extension — a .toml
        // renamed to .json.bak would mislead anyone trying to restore it).
        fs::copy(&agent.config_path, append_bak(&agent.config_path)).ok();
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

// ── Desktop launcher (Linux) ──────────────────────────────────────────────────

/// Simple semantic-graph logo: 5 orange nodes + edges on a cream background.
pub fn logo_svg() -> &'static str {
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
  <rect x="0" y="0" width="64" height="64" rx="12" fill="#FBF3E0"/>
  <g stroke="#E8820C" stroke-width="2.6" stroke-linecap="round">
    <line x1="32" y1="15" x2="51" y2="29"/>
    <line x1="51" y1="29" x2="44" y2="51"/>
    <line x1="44" y1="51" x2="20" y2="51"/>
    <line x1="20" y1="51" x2="13" y2="29"/>
    <line x1="13" y1="29" x2="32" y2="15"/>
    <line x1="32" y1="15" x2="44" y2="51"/>
  </g>
  <g fill="#E8820C" stroke="#B25E00" stroke-width="1.4">
    <circle cx="32" cy="15" r="5.4"/>
    <circle cx="51" cy="29" r="5.4"/>
    <circle cx="44" cy="51" r="5.4"/>
    <circle cx="20" cy="51" r="5.4"/>
    <circle cx="13" cy="29" r="5.4"/>
  </g>
</svg>
"##
}

/// XDG `.desktop` entry launching the TUI viewer in a terminal.
fn desktop_entry(bin: &str, icon_path: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=sparql-mcp\n\
         Comment=Browse sparql-mcp projects (semantic knowledge base)\n\
         Exec=bash -c \"{bin} tui || {{ echo; echo [sparql-mcp] exited with an error; \
read -rsn1 -p 'Press any key to close...'; }}\"\n\
         Icon={icon_path}\n\
         Terminal=true\n\
         Categories=Development;Utility;\n"
    )
}

#[cfg(target_os = "linux")]
fn install_desktop(bin: &str, dry_run: bool) -> Result<()> {
    let home = dirs_home().context("no HOME directory")?;
    let icon_dir = home.join(".local/share/icons");
    let app_dir = home.join(".local/share/applications");
    let icon_path = icon_dir.join("sparql-mcp.svg");
    let desktop_path = app_dir.join("sparql-mcp.desktop");
    let entry = desktop_entry(bin, &icon_path.to_string_lossy());

    if dry_run {
        println!("[dry-run] would write icon  {}", icon_path.display());
        println!("[dry-run] would write launcher {}", desktop_path.display());
        return Ok(());
    }

    fs::create_dir_all(&icon_dir).context("creating icon dir")?;
    fs::create_dir_all(&app_dir).context("creating applications dir")?;
    fs::write(&icon_path, logo_svg()).context("writing logo")?;
    fs::write(&desktop_path, &entry).context("writing .desktop")?;
    set_executable(&desktop_path);
    println!(
        "  + desktop launcher installed ({})",
        desktop_path.display()
    );

    // Best-effort copy onto the Desktop, if one exists.
    let desk = home.join("Desktop");
    if desk.is_dir() {
        let on_desktop = desk.join("sparql-mcp.desktop");
        if fs::write(&on_desktop, &entry).is_ok() {
            set_executable(&on_desktop);
            println!("  + desktop icon placed ({})", on_desktop.display());
        }
    }
    Ok(())
}

/// Windows `.cmd` launcher that opens the terminal viewer.
pub fn windows_launcher_cmd(bin: &str) -> String {
    format!("@echo off\r\n\"{bin}\" tui\r\n")
}

#[cfg(windows)]
fn install_desktop(bin: &str, dry_run: bool) -> Result<()> {
    let home = dirs_home().context("no home directory")?;
    let desktop = home.join("Desktop");
    let programs = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Roaming"))
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    let script = windows_launcher_cmd(bin);

    if dry_run {
        println!(
            "[dry-run] would write {}",
            desktop.join("sparql-mcp.cmd").display()
        );
        println!(
            "[dry-run] would write {}",
            programs.join("sparql-mcp.cmd").display()
        );
        return Ok(());
    }

    for dir in [&desktop, &programs] {
        if fs::create_dir_all(dir).is_ok() {
            let path = dir.join("sparql-mcp.cmd");
            if fs::write(&path, &script).is_ok() {
                println!("  + launcher installed ({})", path.display());
            }
        }
    }

    // Best-effort: a real .lnk shortcut on the Desktop (nicer icon, single click).
    let lnk = desktop.join("sparql-mcp.lnk");
    let ps = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');\
         $s.TargetPath='{}';$s.Arguments='tui';$s.IconLocation='{},0';$s.Save()",
        lnk.display(),
        bin,
        bin
    );
    let made_lnk = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if made_lnk {
        println!("  + desktop shortcut placed ({})", lnk.display());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn install_desktop(_bin: &str, _dry_run: bool) -> Result<()> {
    println!("  = desktop launcher: skipped (Linux and Windows only)");
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o755);
        let _ = fs::set_permissions(path, perm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_launches_tui_in_terminal() {
        let e = desktop_entry(
            "/usr/bin/sparql-mcp",
            "/home/u/.local/share/icons/sparql-mcp.svg",
        );
        assert!(e.contains("/usr/bin/sparql-mcp tui"));
        assert!(e.contains("Press any key to close"), "stays open on error");
        assert!(e.contains("Terminal=true"));
        assert!(e.contains("Type=Application"));
        assert!(e.contains("Icon=/home/u/.local/share/icons/sparql-mcp.svg"));
    }

    #[test]
    fn windows_launcher_cmd_runs_tui() {
        let c = windows_launcher_cmd("C:\\bin\\sparql-mcp.exe");
        assert!(c.contains("@echo off"));
        assert!(c.contains("sparql-mcp.exe"));
        assert!(c.trim_end().ends_with("tui"));
    }

    #[test]
    fn logo_is_orange_on_cream_svg() {
        let svg = logo_svg();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("#FBF3E0"), "cream background");
        assert!(svg.contains("#E8820C"), "orange nodes");
        assert_eq!(svg.matches("<circle").count(), 5, "5 nodes");
    }

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
