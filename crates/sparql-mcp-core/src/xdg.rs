//! XDG-style resolution of the per-user data root for sparql-mcp.
//!
//! Resolution order, first hit wins:
//! 1. `$SPARQL_MCP_HOME`              (explicit override)
//! 2. `$XDG_DATA_HOME/sparql-mcp`     (freedesktop.org spec)
//! 3. `$HOME/.local/share/sparql-mcp` (Unix fallback)
//! 4. `$USERPROFILE\AppData\Local\sparql-mcp` (Windows)

use std::path::PathBuf;

pub fn data_home() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SPARQL_MCP_HOME") {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(p).join("sparql-mcp"));
    }
    if let Some(h) = std::env::var_os("HOME") {
        return Some(
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("sparql-mcp"),
        );
    }
    if let Some(h) = std::env::var_os("USERPROFILE") {
        return Some(
            PathBuf::from(h)
                .join("AppData")
                .join("Local")
                .join("sparql-mcp"),
        );
    }
    None
}

pub fn store_dir() -> PathBuf {
    data_home()
        .map(|p| p.join("store"))
        .unwrap_or_else(|| PathBuf::from("./store"))
}

pub fn ontology_dir() -> PathBuf {
    data_home()
        .map(|p| p.join("ontology"))
        .unwrap_or_else(|| PathBuf::from("./ontology"))
}

pub fn docs_dir() -> PathBuf {
    data_home()
        .map(|p| p.join("docs"))
        .unwrap_or_else(|| PathBuf::from("./docs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparql_mcp_home_wins() {
        // Isolate from the ambient environment.
        let prev = std::env::var_os("SPARQL_MCP_HOME");
        std::env::set_var("SPARQL_MCP_HOME", "/tmp/smc-test");
        assert_eq!(data_home().unwrap(), PathBuf::from("/tmp/smc-test"));
        assert_eq!(store_dir(), PathBuf::from("/tmp/smc-test/store"));
        match prev {
            Some(v) => std::env::set_var("SPARQL_MCP_HOME", v),
            None => std::env::remove_var("SPARQL_MCP_HOME"),
        }
    }
}
