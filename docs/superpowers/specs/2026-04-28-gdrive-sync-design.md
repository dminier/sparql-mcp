# GDrive Sync — Design Spec

**Date:** 2026-04-28  
**Status:** approved  
**Scope:** Multi-machine sync of the Obsidian vault and sparql-mcp KB backend via Google Drive MCP tools.

---

## Context

sparql-mcp stores its knowledge graph in a local RocksDB store and renders an Obsidian vault as the human-facing projection. A single user works across multiple machines; each machine runs the MCP server locally (required for I/O performance). Google Drive acts as the sync hub between machines — not a live data store.

Multiple Claude Code agents may run concurrently on the same machine, all hitting the same local MCP server. GDrive is never in the I/O path during active use.

---

## Goals

- Vault Obsidian accessible and up-to-date on every machine.
- KB backend (triples + schema) restorable on a new machine with no manual steps.
- Sync triggered automatically after every render and available on demand.
- Both TTL export (portable, human-readable) and full RocksDB snapshot (plug-and-play restore) supported.

## Non-goals

- Real-time multi-machine write concurrency (one active machine at a time).
- GDrive as a live query endpoint.
- Conflict resolution between two machines that diverged simultaneously.

---

## GDrive Structure

```
GDrive/
└── sparql-kb/
    ├── sync-manifest.json
    ├── vault/
    │   ├── _Ontology/
    │   ├── <project-slug>/
    │   │   ├── <notes>.md
    │   │   └── <slug>.canvas
    │   └── <project-slug>.ttl
    └── store-backups/
        ├── ttl/
        │   └── <project-slug>/
        │       └── <timestamp>.ttl
        └── rocksdb/
            └── <timestamp>.tar.gz
```

**`sync-manifest.json` schema:**
```json
{
  "machine": "<hostname>",
  "pushed_at": "<ISO-8601>",
  "graphs": ["urn:project:foo", "urn:project:bar"]
}
```

The `sparql-kb/` root folder is created once. Its GDrive folder ID is persisted in `sparql-mcp.toml`.

---

## Configuration

New section in `sparql-mcp.toml`:

```toml
[gdrive]
enabled        = true
folder_id      = "<GDrive folder ID of sparql-kb/>"
backup_retain  = 5        # number of RocksDB snapshots to keep
sync_on_render = true     # auto-push after every Obsidian render
```

**First-run bootstrap:** if `folder_id` is absent, the skill creates `sparql-kb/` via the GDrive MCP tool and writes the returned ID into `sparql-mcp.toml`. Authentication uses the existing `mcp__claude_ai_Google_Drive__*` tools — no additional credentials.

---

## Operations

### `kb sync push`

Triggered automatically after a successful Obsidian render (when `sync_on_render = true`) or manually via trigger phrases.

Steps (in order):
1. For each active graph: `export_graph` → upload to `store-backups/ttl/<slug>/<ts>.ttl` and overwrite `vault/<slug>.ttl`.
2. Archive the local RocksDB store directory as `<ts>.tar.gz` → upload to `store-backups/rocksdb/`.
3. Upload the full Obsidian vault to `vault/`, using GDrive file metadata to skip unchanged files (diff by modified timestamp).
4. Write `sync-manifest.json` with current hostname, timestamp, and graph list.
5. Rotate RocksDB snapshots: delete oldest entries beyond `backup_retain`.

### `kb sync pull`

Triggered on a new machine before starting work.

Steps:
1. Read `sync-manifest.json`. If the local store's last-modified timestamp is newer than `pushed_at`, warn and ask for confirmation before overwriting.
2. User chooses restore mode:
   - **TTL mode:** download the latest `.ttl` per graph from `store-backups/ttl/` → `load_ontology` into the local store.
   - **RocksDB mode:** download the latest `tar.gz` from `store-backups/rocksdb/` → extract into `[core] store` path.
3. Download the full `vault/` tree into the local `vault_root`.

### `kb sync status`

Reads and displays `sync-manifest.json`: last sync machine, timestamp, graphs included.

---

## Skill Integration (kb-workbench)

### Trigger phrases

| Intent | Phrases (FR / EN) |
|---|---|
| push | `kb sync`, `synchronise la kb`, `push to drive`, `sauvegarde`, `sync kb` |
| pull | `kb restore`, `pull from drive`, `restaure la kb`, `nouvelle machine`, `restore kb` |
| status | `kb sync status`, `état de la sync`, `dernière sync` |

### Render hook

After every render, if `sync_on_render = true`, the skill appends a `kb sync push` step. The render summary reports push outcome (files uploaded, backup size, timestamp).

### New reference file

`skills/kb-workbench/references/gdrive-sync.md` — documents the GDrive protocol, folder structure, bootstrap flow, and the pull procedure for a new machine. Loaded automatically when any sync trigger phrase is detected.

### Implementation boundary

No new Rust binary. All GDrive operations go through the existing `mcp__claude_ai_Google_Drive__*` MCP tools. The skill orchestrates the calls; `sparql-mcp` is not modified except to read the new `[gdrive]` config section (for `sync_on_render` awareness in the render completion hook).

---

## Sequence: New Machine Bootstrap

```
1. Clone / install sparql-mcp on new machine
2. Add [gdrive] section to sparql-mcp.toml with folder_id
3. Agent detects empty local store → proposes kb sync pull
4. User selects restore mode (TTL or RocksDB)
5. Store hydrated, vault downloaded to vault_root
6. Agent starts normally
```

---

## Error Handling

- GDrive upload failure during push: log error, do not fail the render. Report in summary.
- RocksDB archive exceeds a reasonable size (>500 MB): warn user before uploading, offer TTL-only push.
- `sync-manifest.json` missing on pull: proceed with latest available backup, warn that manifest is absent.
- Partial vault upload interrupted: next push is idempotent and will complete the missing files.
- `folder_id` absent at push time (not just bootstrap): abort push, surface a clear error asking the user to run the bootstrap or set `folder_id` manually in `sparql-mcp.toml`.
