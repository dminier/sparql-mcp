# GDrive Sync — agent protocol

Backed by `get_gdrive_config` (sparql-mcp MCP tool), the `mcp__claude_ai_Google_Drive__*` tools,
and **`scripts/kb_gdrive_sync.py`** for all file-system operations.

---

## ★ RÈGLE D'OR — Une seule archive, jamais fichier par fichier

Ne jamais uploader les graphs TTL individuellement. Le protocole correct est toujours :

```
exporter tous les graphs → compresser en une archive tar.gz → uploader l'archive
```

Pourquoi : chaque appel MCP `create_file` encode le contenu en base64 dans le corps JSON.
Limite pratique : **4 MB raw** (base64 ≈ ×1.33). Au-delà, `rclone` est obligatoire.

Le script `scripts/kb_gdrive_sync.py` détecte automatiquement le bon chemin :

```bash
python3 scripts/kb_gdrive_sync.py b64check --file <archive>
# → {"fits_mcp": false, "upload_via": "rclone"}
```

---

## Architecture agent ↔ script

```
Agent (MCP)                          Script (subprocess)
────────────────────────────────     ────────────────────────────────
mcp__sparql-mcp__get_gdrive_config   
mcp__sparql-mcp__list_graphs         
mcp__sparql-mcp__export_graph × N    
                                  →  compress  --src /tmp/kb-sync --out <archive>
                                     b64check  --file <archive>
                                     b64       --file <archive>   (si fits_mcp)
                                     upload    --file <archive> --remote gdrive:...  (sinon)
                                     manifest  --folder-id <id> --graphs ... --archive <name>
mcp__claude_ai_Google_Drive__create_file   (manifest.json, et archive si fits_mcp)
```

L'agent gère les appels MCP. Le script gère la compression, la détection de taille,
rclone et la génération du manifest.

---

## 0. Pre-flight : lire la config

```python
config = mcp__sparql-mcp__get_gdrive_config()
# → {enabled, folder_id, backup_retain, store_path}
```

- `enabled = false` → arrêter, demander à l'utilisateur d'ajouter `[gdrive]` dans `sparql-mcp.toml`.
- `folder_id = null` → exécuter **Bootstrap** (§4) d'abord.

---

## 1. kb sync push

### 1a. Exporter tous les graphs

```python
graphs = mcp__sparql-mcp__list_graphs()   # → {graphs: [...], count: N}
ts = datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
mkdir -p /tmp/kb-sync

for graph_iri in graphs.graphs:
    slug = graph_iri.replace(":", "_").replace("/", "_")
    mcp__sparql-mcp__export_graph(
        graph_iri = graph_iri,
        path      = f"/tmp/kb-sync/{slug}.ttl"
    )
```

### 1b. Compresser en une archive unique

```bash
python3 scripts/kb_gdrive_sync.py compress \
  --src /tmp/kb-sync \
  --out /tmp/kb-sync/kb-all-graphs-{ts}.tar.gz
# → {"archive": "...", "files": 18, "size_mb": 6.1, "upload_via": "rclone"|"mcp"}
```

### 1c. Uploader l'archive

```bash
# Vérifier la taille
result = python3 scripts/kb_gdrive_sync.py b64check --file <archive>
```

**Si `fits_mcp = true`** (archive < 4 MB) :

```bash
b64 = python3 scripts/kb_gdrive_sync.py b64 --file <archive>
# Puis :
mcp__claude_ai_Google_Drive__create_file(
    title                   = "<archive_name>",
    mimeType                = "application/gzip",
    parentId                = resolve_subfolder(folder_id, "store-backups/ttl/"),
    disableConversionToGoogleType = True,
    content                 = b64,
)
```

**Si `fits_mcp = false`** (archive ≥ 4 MB) → utiliser rclone :

```bash
python3 scripts/kb_gdrive_sync.py upload \
  --file  <archive> \
  --remote gdrive:sparql-kb/store-backups/ttl/
```

> Si rclone n'est pas configuré, le script affiche les instructions (`rclone config`).
> Demander à l'utilisateur de taper `! rclone config` pour le configurer de façon interactive.

### 1d. Écrire le manifest

```bash
result = python3 scripts/kb_gdrive_sync.py manifest \
  --folder-id <folder_id> \
  --graphs <iri1> <iri2> ... \
  --archive <archive_name> \
  --out /tmp/kb-sync/manifest.json
# → {"b64": "...", "manifest": {...}}

mcp__claude_ai_Google_Drive__create_file(
    title   = "sync-manifest.json",
    mimeType = "application/json",
    parentId = folder_id,
    disableConversionToGoogleType = True,
    content  = result["b64"],
)
```

### 1e. Rotation des archives (optionnel)

Les outils MCP GDrive n'exposent pas de endpoint de suppression.
Si `backup_retain` est dépassé, lister les fichiers dans `store-backups/ttl/` et signaler à l'utilisateur les archives à supprimer manuellement.

---

## 2. kb sync pull

### 2a. Lire le manifest

```python
files = mcp__claude_ai_Google_Drive__search_files(
    query = "title = 'sync-manifest.json'"
)
content = mcp__claude_ai_Google_Drive__read_file_content(file_id=files[0].id)
manifest = json.loads(content)
```

Si le store local est plus récent que `manifest.pushed_at` → avertir et demander confirmation.

### 2b. Télécharger et extraire l'archive TTL

```python
# Chercher l'archive dans store-backups/ttl/
files = mcp__claude_ai_Google_Drive__search_files(
    query = f"title = '{manifest.archive}'"
)
content = mcp__claude_ai_Google_Drive__download_file_content(file_id=files[0].id)
# Écrire en binaire → /tmp/kb-sync-restore/<archive_name>
# Puis :
tar -xzf /tmp/kb-sync-restore/<archive_name> -C /tmp/kb-sync-restore/
```

### 2c. Réimporter les graphs

```python
for ttl_file in /tmp/kb-sync-restore/*.ttl:
    graph_iri = derive_iri_from_filename(ttl_file)  # inverse du slug
    mcp__sparql-mcp__load_ontology_file(
        path      = ttl_file,
        graph_iri = graph_iri
    )
```

---

## 3. kb sync status

```bash
# Local
python3 scripts/kb_gdrive_sync.py status --manifest /tmp/kb-sync/manifest.json

# GDrive
files = mcp__claude_ai_Google_Drive__search_files(query="title = 'sync-manifest.json'")
content = mcp__claude_ai_Google_Drive__read_file_content(file_id=files[0].id)
manifest = json.loads(content)
# Comparer pushed_at avec mtime du store local
```

Afficher : machine, pushed_at, nombre de graphs, nom de l'archive.

---

## 4. Bootstrap (premier lancement / folder_id absent)

```python
# Vérifier si sparql-kb existe déjà
existing = mcp__claude_ai_Google_Drive__search_files(
    query = "title = 'sparql-kb' and mimeType = 'application/vnd.google-apps.folder'"
)

if existing.files:
    folder_id = existing.files[0].id
else:
    result = mcp__claude_ai_Google_Drive__create_file(
        title    = "sparql-kb",
        mimeType = "application/vnd.google-apps.folder"
    )
    folder_id = result.id
```

Informer l'utilisateur d'ajouter dans `sparql-mcp.toml` :

```toml
[gdrive]
enabled   = true
folder_id = "<folder_id>"
```

Les sous-dossiers (`store-backups/ttl/`) sont créés à la demande lors du premier push.

---

## 5. Helper : resolve_subfolder

Créer paresseusement un chemin de sous-dossiers dans GDrive :

```python
def resolve_subfolder(root_id, path):
    """path = "store-backups/ttl" — crée les segments manquants."""
    current = root_id
    for segment in path.split("/"):
        results = mcp__claude_ai_Google_Drive__search_files(
            query = f"title = '{segment}' and mimeType = 'application/vnd.google-apps.folder'"
        )
        if results.files:
            current = results.files[0].id
        else:
            f = mcp__claude_ai_Google_Drive__create_file(
                title    = segment,
                mimeType = "application/vnd.google-apps.folder",
                parentId = current
            )
            current = f.id
    return current
```

---

## 6. Configuration rclone (première fois)

Si `upload_via = "rclone"` et rclone non configuré :

```
Demandez à l'utilisateur de taper dans le prompt :
  ! ~/.local/bin/rclone config

Choisir : n (new remote) → nom "gdrive" → type "drive" → laisser client_id vide
→ authentifier via browser → terminer.

Ensuite tester :
  ! ~/.local/bin/rclone lsd gdrive:
```

rclone est installé dans `~/.local/bin/rclone` (installé par kb_gdrive_sync.py ou manuellement).
