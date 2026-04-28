# GDrive Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add multi-machine KB sync via Google Drive — Obsidian vault + TTL exports + RocksDB snapshots stored in GDrive, pushed after each render and pulled to hydrate a fresh machine.

**Architecture:** A new `[gdrive]` TOML section is parsed into a `GDriveConfig` struct and threaded through to a new `get_gdrive_config` MCP tool, which exposes folder_id and store_path to the agent. All GDrive I/O is orchestrated at skill level using the existing `mcp__claude_ai_Google_Drive__*` MCP tools — no new Rust binary. A new `gdrive-sync.md` reference file drives the agent through push/pull/status/bootstrap flows.

**Tech Stack:** Rust (serde/toml for config), rmcp (MCP tool registration), Markdown skill files (kb-workbench plugin)

---

## File Map

| Action | File | Responsibility |
|---|---|---|
| Modify | `crates/sparql-mcp-core/src/config.rs` | Add `GDriveConfig` struct + `gdrive` field on `Config` |
| Create | `crates/sparql-mcp-core/src/application/tools/gdrive.rs` | `get_gdrive_config` tool definition + implementation |
| Modify | `crates/sparql-mcp-core/src/application/tools/mod.rs` | Expose `pub mod gdrive` |
| Modify | `crates/sparql-mcp-core/src/mcp/server.rs` | Add `gdrive_config` + `store_path` fields; register + dispatch tool |
| Modify | `crates/sparql-mcp-core/src/main.rs` | Pass `gdrive_config` and `store_path` to `SparqlMcpServer::new` |
| Create | `plugins/kb-workbench/skills/kb-workbench/references/gdrive-sync.md` | Full agent sync protocol (push / pull / status / bootstrap) |
| Modify | `plugins/kb-workbench/skills/kb-workbench/SKILL.md` | Add sync trigger phrases |
| Modify | `plugins/kb-workbench/skills/kb-workbench/references/obsidian-rendering.md` | Add post-render sync step |

---

## Task 1: GDriveConfig struct in config.rs

**Files:**
- Modify: `crates/sparql-mcp-core/src/config.rs`

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/sparql-mcp-core/src/config.rs` (inside an existing `#[cfg(test)]` block, or create one):

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p sparql-mcp-core gdrive_config 2>&1 | head -30
```

Expected: compile error — `gdrive` field not yet on `Config`.

- [ ] **Step 3: Add GDriveConfig struct and field to Config**

In `crates/sparql-mcp-core/src/config.rs`, after the `McpServer` struct (around line 83), add:

```rust
fn default_backup_retain() -> usize { 5 }
fn default_true() -> bool { true }

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
```

Add the field to `Config` (after the existing `mcp` field):

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServer>,
    #[serde(default)]
    pub gdrive: Option<GDriveConfig>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p sparql-mcp-core gdrive_config 2>&1
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/sparql-mcp-core/src/config.rs
git commit -m "feat(config): add GDriveConfig struct with folder_id, backup_retain, sync_on_render"
```

---

## Task 2: `get_gdrive_config` MCP tool

**Files:**
- Create: `crates/sparql-mcp-core/src/application/tools/gdrive.rs`
- Modify: `crates/sparql-mcp-core/src/application/tools/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/sparql-mcp-core/src/application/tools/gdrive.rs` with the test first:

```rust
//! get_gdrive_config tool — returns GDrive sync configuration and store path.

use std::path::PathBuf;

use rmcp::model::{CallToolResult, Content, JsonObject, Tool};
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

    #[test]
    fn returns_config_when_present() {
        let gd = Some(GDriveConfig {
            enabled: true,
            folder_id: Some("abc123".to_string()),
            backup_retain: 3,
            sync_on_render: false,
        });
        let result = get_gdrive_config(&gd, &PathBuf::from("/tmp/store")).unwrap();
        let text = match &result.content[0] {
            rmcp::model::Content::Text(t) => t.text.clone(),
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
        let text = match &result.content[0] {
            rmcp::model::Content::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["enabled"], false);
        assert_eq!(v["backup_retain"], 5);
    }
}
```

- [ ] **Step 2: Expose module in mod.rs**

In `crates/sparql-mcp-core/src/application/tools/mod.rs`, add:

```rust
pub mod gdrive;
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p sparql-mcp-core get_gdrive_config 2>&1 | head -20
```

Expected: compile errors — `make_tool` not yet imported, but module structure is there.

- [ ] **Step 4: Fix imports (make_tool is pub in sparql.rs)**

Verify `make_tool` is re-exported. In `crates/sparql-mcp-core/src/application/tools/sparql.rs`, confirm the function is `pub fn make_tool`. If it is, no change needed. If not, add `pub` to it.

```bash
grep -n "pub fn make_tool\|fn make_tool" crates/sparql-mcp-core/src/application/tools/sparql.rs
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p sparql-mcp-core get_gdrive_config 2>&1
```

Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/sparql-mcp-core/src/application/tools/gdrive.rs \
        crates/sparql-mcp-core/src/application/tools/mod.rs
git commit -m "feat(tools): add get_gdrive_config MCP tool"
```

---

## Task 3: Wire tool into SparqlMcpServer

**Files:**
- Modify: `crates/sparql-mcp-core/src/mcp/server.rs`
- Modify: `crates/sparql-mcp-core/src/main.rs`

- [ ] **Step 1: Write the registration test**

In `crates/sparql-mcp-core/src/mcp/server.rs`, there is already a test `all_core_tools_have_dispatch_arm` (around line 310). Run it first to confirm it passes before changes:

```bash
cargo test -p sparql-mcp-core all_core_tools_have_dispatch_arm 2>&1
```

Expected: PASS.

- [ ] **Step 2: Add fields to SparqlMcpServer struct**

In `crates/sparql-mcp-core/src/mcp/server.rs`, update the `SparqlMcpServer` struct (lines 30–36):

```rust
#[derive(Clone)]
pub struct SparqlMcpServer {
    store: Arc<dyn SparqlStore>,
    doc_store: Arc<dyn DocStore>,
    ontology_dir: PathBuf,
    active_graph: String,
    plugins: Arc<Vec<Box<dyn ToolPlugin>>>,
    gdrive_config: Option<crate::config::GDriveConfig>,
    store_path: PathBuf,
}
```

- [ ] **Step 3: Update `new` constructor**

Replace the `new` method (lines 39–53):

```rust
pub fn new(
    store: Arc<dyn SparqlStore>,
    doc_store: Arc<dyn DocStore>,
    ontology_dir: PathBuf,
    active_graph: String,
    gdrive_config: Option<crate::config::GDriveConfig>,
    store_path: PathBuf,
) -> Self {
    Self {
        store,
        doc_store,
        ontology_dir,
        active_graph,
        plugins: Arc::new(Vec::new()),
        gdrive_config,
        store_path,
    }
}
```

- [ ] **Step 4: Register the tool in core_tool_defs**

In `core_tool_defs()` (around line 263), add after the last existing tool:

```rust
crate::application::tools::gdrive::tool_get_gdrive_config_def(),
```

- [ ] **Step 5: Add dispatch arm**

In `dispatch_core` (around line 89), add after the `"write_doc"` arm:

```rust
"get_gdrive_config" => gdrive::get_gdrive_config(&self.gdrive_config, &self.store_path),
```

And add the import at the top of the `use` block in `server.rs`:

```rust
use crate::application::tools::{cbm, doc, export, gdrive, ontology, project, sparql};
```

- [ ] **Step 6: Update main.rs to pass new fields**

In `crates/sparql-mcp-core/src/main.rs`, locate the `Cmd::Serve` arm (around line 352). Replace:

```rust
let srv = SparqlMcpServer::new(store, doc_store, ontology_path, cli.active_graph);
```

With:

```rust
let srv = SparqlMcpServer::new(
    store,
    doc_store,
    ontology_path,
    cli.active_graph,
    cfg.gdrive.clone(),
    store_path.clone(),
);
```

- [ ] **Step 7: Run the registration test**

```bash
cargo test -p sparql-mcp-core all_core_tools_have_dispatch_arm 2>&1
```

Expected: PASS (confirms `get_gdrive_config` is both in list and dispatch).

- [ ] **Step 8: Run full test suite**

```bash
cargo test -p sparql-mcp-core 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/sparql-mcp-core/src/mcp/server.rs \
        crates/sparql-mcp-core/src/main.rs
git commit -m "feat(server): thread GDriveConfig and store_path into SparqlMcpServer; register get_gdrive_config tool"
```

---

## Task 4: gdrive-sync.md reference file

**Files:**
- Create: `plugins/kb-workbench/skills/kb-workbench/references/gdrive-sync.md`

- [ ] **Step 1: Create the reference file**

Create `plugins/kb-workbench/skills/kb-workbench/references/gdrive-sync.md` with the full agent sync protocol:

````markdown
# GDrive Sync — agent protocol

Backed by `get_gdrive_config` (sparql-mcp MCP tool) and the `mcp__claude_ai_Google_Drive__*` tools.

## 0. Pre-flight: read config

Always start any sync operation by calling:

```
mcp__sparql-mcp__get_gdrive_config()
```

This returns:
```json
{
  "enabled": true,
  "folder_id": "<ID of sparql-kb/ in GDrive>",
  "backup_retain": 5,
  "sync_on_render": true,
  "store_path": "/home/user/.local/share/sparql-mcp/store"
}
```

If `enabled` is false, abort and inform the user to add `[gdrive]\nenabled = true\nfolder_id = "..."` to `sparql-mcp.toml`.

If `folder_id` is null, run **Bootstrap** (§4) first.

---

## 1. kb sync push

### 1a. Export TTL per graph

```
graphs = mcp__sparql-mcp__list_graphs()
for each graph_iri in graphs:
    ttl = mcp__sparql-mcp__export_graph(graph_iri=graph_iri)
    slug = graph_iri.split(":")[-1]   # e.g. "urn:project:foo" → "foo"
    ts   = current UTC timestamp in YYYYMMDDTHHMMSSz format

    # Versioned backup
    upload_or_overwrite(
        parent_id = resolve_subfolder(folder_id, "store-backups/ttl/<slug>/"),
        name      = "<ts>.ttl",
        content   = ttl
    )

    # Live snapshot in vault
    upload_or_overwrite(
        parent_id = resolve_subfolder(folder_id, "vault/"),
        name      = "<slug>.ttl",
        content   = ttl
    )
```

### 1b. Archive and upload RocksDB snapshot

```bash
ts=$(date -u +%Y%m%dT%H%M%SZ)
tar -czf /tmp/sparql-mcp-rocksdb-$ts.tar.gz -C "$(dirname $STORE_PATH)" "$(basename $STORE_PATH)"
```

Upload `/tmp/sparql-mcp-rocksdb-$ts.tar.gz` to subfolder `store-backups/rocksdb/` in GDrive.

### 1c. Upload Obsidian vault

For each file in `vault_root` (from `render_spec.yaml`):
- Search GDrive for the file by name in the appropriate subfolder.
- If modifiedTime in GDrive < local mtime → upload/overwrite.
- If not found → create.

Use `mcp__claude_ai_Google_Drive__create_file` for new files,
`mcp__claude_ai_Google_Drive__create_file` with the existing file id to overwrite.

### 1d. Write sync-manifest.json

Upload to the root `folder_id`:

```json
{
  "machine": "<hostname>",
  "pushed_at": "<ISO-8601 UTC timestamp>",
  "graphs": ["urn:project:foo", "urn:project:bar"]
}
```

Use `mcp__claude_ai_Google_Drive__create_file` (overwrites if already exists — find existing ID first via `search_files`).

### 1e. Rotate RocksDB snapshots

```
files = mcp__claude_ai_Google_Drive__search_files(
    query="'<rocksdb_folder_id>' in parents and name contains '.tar.gz'"
)
sort files by createdTime ascending
if len(files) > backup_retain:
    delete oldest (len(files) - backup_retain) files
```

Note: GDrive MCP tools do not expose a delete endpoint. Log a warning listing files to delete manually if over the limit; do not fail the push.

---

## 2. kb sync pull

### 2a. Check manifest

```
manifest = mcp__claude_ai_Google_Drive__search_files(
    query="name = 'sync-manifest.json' and '<folder_id>' in parents"
)
content = mcp__claude_ai_Google_Drive__read_file_content(file_id=manifest[0].id)
```

If local store mtime > manifest.pushed_at: warn user and ask confirmation before overwriting.

### 2b. Choose restore mode

Ask: "TTL mode (re-import triples) or RocksDB mode (replace store directory)?"

**TTL mode:**
```
for each graph in manifest.graphs:
    slug = graph.split(":")[-1]
    files = search_files(query="'<ttl_slug_folder_id>' in parents")
    latest = sort by createdTime desc, take first
    ttl_content = read_file_content(latest.id)
    write to /tmp/<slug>.ttl
    mcp__sparql-mcp__load_ontology_file(path="/tmp/<slug>.ttl", graph_iri=graph)
```

**RocksDB mode:**
```
files = search_files(query="'<rocksdb_folder_id>' in parents and name contains '.tar.gz'")
latest = sort by createdTime desc, take first
content = download_file_content(latest.id)    # binary
write to /tmp/sparql-mcp-restore.tar.gz
```

Then (agent asks user to run):
```bash
systemctl stop sparql-mcp 2>/dev/null || true
rm -rf "$STORE_PATH"
tar -xzf /tmp/sparql-mcp-restore.tar.gz -C "$(dirname $STORE_PATH)"
```

### 2c. Download vault

```
vault_files = list all files under store-backups/vault/ recursively
for each file:
    content = read_file_content(file.id)
    write to local vault_root preserving path structure
```

---

## 3. kb sync status

```
manifest = search + read sync-manifest.json from GDrive
print: machine, pushed_at, graphs
compare pushed_at with local store mtime
report: "in sync" or "local store is N minutes ahead of last push"
```

---

## 4. Bootstrap (first run on new machine / folder_id absent)

1. Create the root folder:
```
result = mcp__claude_ai_Google_Drive__create_file(
    name="sparql-kb",
    mimeType="application/vnd.google-apps.folder"
)
folder_id = result.id
```

2. Inform user: "Add this to `sparql-mcp.toml`:"
```toml
[gdrive]
enabled   = true
folder_id = "<folder_id>"
```

3. Subfolders are created lazily on first push (search → create if missing).

---

## 5. Helper: resolve_subfolder

To find or create a nested subfolder path like `store-backups/ttl/foo/`:

```
current_id = folder_id
for each segment in ["store-backups", "ttl", "foo"]:
    results = search_files(query="name='<segment>' and '<current_id>' in parents and mimeType='application/vnd.google-apps.folder'")
    if results empty:
        result = create_file(name=segment, mimeType="application/vnd.google-apps.folder", parent=current_id)
        current_id = result.id
    else:
        current_id = results[0].id
return current_id
```
````

- [ ] **Step 2: Verify file created**

```bash
ls plugins/kb-workbench/skills/kb-workbench/references/gdrive-sync.md
```

Expected: file exists.

- [ ] **Step 3: Commit**

```bash
git add plugins/kb-workbench/skills/kb-workbench/references/gdrive-sync.md
git commit -m "docs(kb-workbench): add gdrive-sync reference — full agent push/pull/bootstrap protocol"
```

---

## Task 5: Update SKILL.md with sync triggers

**Files:**
- Modify: `plugins/kb-workbench/skills/kb-workbench/SKILL.md`

- [ ] **Step 1: Read current triggers section**

Open `plugins/kb-workbench/skills/kb-workbench/SKILL.md` and locate the section that maps trigger phrases to references (search for "trigger" or "auto-trigger" or the phrase list).

- [ ] **Step 2: Add sync trigger block**

In the triggers / routing section of `SKILL.md`, add a new entry alongside the existing ones:

```markdown
## GDrive sync

Triggered by: `kb sync`, `kb sync push`, `kb sync pull`, `kb sync status`,
`synchronise la kb`, `push to drive`, `sauvegarde`, `pull from drive`,
`restaure la kb`, `nouvelle machine`, `restore kb`, `état de la sync`, `dernière sync`.

→ Load `references/gdrive-sync.md` and follow the matching operation (push / pull / status / bootstrap).
```

- [ ] **Step 3: Commit**

```bash
git add plugins/kb-workbench/skills/kb-workbench/SKILL.md
git commit -m "feat(kb-workbench): add GDrive sync trigger phrases to SKILL.md"
```

---

## Task 6: Post-render sync step in obsidian-rendering.md

**Files:**
- Modify: `plugins/kb-workbench/skills/kb-workbench/references/obsidian-rendering.md`

- [ ] **Step 1: Add post-render sync section**

At the end of `plugins/kb-workbench/skills/kb-workbench/references/obsidian-rendering.md`, append:

```markdown
## Post-render GDrive sync

After every successful render, if `sync_on_render = true` in the GDrive config
(check via `get_gdrive_config`), immediately trigger **kb sync push** as defined
in `gdrive-sync.md`.

Report the push outcome in the render summary:
- Files uploaded to vault/
- TTL snapshots written
- RocksDB snapshot size + timestamp
- Any errors (do not fail the render — log and continue)
```

- [ ] **Step 2: Commit**

```bash
git add plugins/kb-workbench/skills/kb-workbench/references/obsidian-rendering.md
git commit -m "feat(kb-workbench): trigger GDrive push after Obsidian render when sync_on_render=true"
```

---

## Verification checklist

- [ ] `cargo test -p sparql-mcp-core` — all tests pass
- [ ] `cargo build -p sparql-mcp-core` — clean build
- [ ] `sparql-mcp serve` starts without error with a `[gdrive]` section in `sparql-mcp.toml`
- [ ] `sparql-mcp serve` starts without error with no `[gdrive]` section (defaults work)
- [ ] Agent call to `get_gdrive_config` returns correct JSON with store_path
- [ ] `kb sync status` phrase in Claude Code loads gdrive-sync.md and reads the manifest
- [ ] After a render, agent auto-triggers push when `sync_on_render = true`
