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
