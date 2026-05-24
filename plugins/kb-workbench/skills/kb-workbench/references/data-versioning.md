# Data versioning — private git repo (NOT the public code repo)

> **Hard separation.** This repo (`dminier/sparql-mcp`) versions **code + schema
> migrations** — public, no personal data, ever. Your **store data** (the
> per-project triples, which are personal) is versioned **separately** in a
> **private** git repo. These two histories never mix.
>
> | Concern | Where | Contains |
> |---|---|---|
> | Code + **schema migration process** | `dminier/sparql-mcp` (public) | `migrations/*.ru` (structure only), Rust, skills |
> | **Personal data** | `mcazerty/mcazerty-data` (private) | `*.ttl` exports of `urn:project:*` graphs |
>
> Schema migrations describe *how the structure changes*; they carry no personal
> triples. Data exports carry personal triples; they live only in the private repo.

---

## 0. Pre-flight — do you already have it set up?

Run:

```bash
sparql-mcp-data-status   # alias defined below, or:
git -C "${SPARQL_MCP_DATA_REPO:-$HOME/sparql-mcp-data}" rev-parse --is-inside-work-tree 2>/dev/null \
  && echo "data repo OK" || echo "data repo NOT configured — run §1"
```

If it prints `NOT configured`, walk the user through §1. Otherwise skip to §2.

## 1. One-time setup of the private data repo

The end state: a working **private** git repo that versions the store's TTL exports.

```bash
# 1. Choose a location and remember it
export SPARQL_MCP_DATA_REPO="$HOME/sparql-mcp-data"
echo 'export SPARQL_MCP_DATA_REPO="$HOME/sparql-mcp-data"' >> ~/.bashrc

# 2. Create the PRIVATE remote (must be private — it holds personal data)
gh repo create mcazerty/mcazerty-data --private --description "sparql-mcp store data (personal)"

# 3. Clone and seed it
git clone git@github.com:mcazerty/mcazerty-data.git "$SPARQL_MCP_DATA_REPO"
cd "$SPARQL_MCP_DATA_REPO"
mkdir -p projects
cat > README.md <<'MD'
# sparql-mcp-data (PRIVATE)
Per-project TTL exports of the sparql-mcp store. Personal data — keep private.
One file per project graph: projects/<slug>.ttl
MD
git add README.md && git commit -m "init: private sparql-mcp data repo"
git push -u origin main
```

> If `gh` is not authenticated, the user runs `! gh auth login` in this session.
> If they prefer a non-GitHub host, any private remote works — only the
> `git remote add origin <url>` line changes.

## 2. Sync data → private repo (run after meaningful changes / before machine switch)

```bash
cd "$SPARQL_MCP_DATA_REPO"
```

For each project graph, export TTL via the MCP tool and write it under `projects/`:

```
graphs = mcp__sparql-mcp__list_graphs()          # urn:project:<slug>, skip urn:meta/urn:staging:*
for g in graphs where g startswith "urn:project:":
    slug = g.rsplit(":",1)[-1]
    ttl  = mcp__sparql-mcp__export_graph(graph_iri=g)
    write ttl to  projects/<slug>.ttl
```

Then commit + push:

```bash
git add projects/
git commit -m "data: snapshot $(date -u +%Y-%m-%dT%H:%MZ)"
git push
```

The git history of `mcazerty-data` **is** the version history of your data.
(The existing GDrive sync — `gdrive-sync.md` — remains available for
multi-machine hydration; it is complementary, not a replacement.)

## 3. Restore data on a new machine

```bash
git clone git@github.com:mcazerty/mcazerty-data.git "$SPARQL_MCP_DATA_REPO"
for f in "$SPARQL_MCP_DATA_REPO"/projects/*.ttl; do
  slug=$(basename "$f" .ttl)
  sparql-mcp load-file --path "$f" --graph "urn:project:$slug"
done
```

## 4. Guard — keep personal data OUT of the public repo

The public repo ships a pre-commit guard that **rejects** committing any `*.ttl`
containing `urn:project:` data, or RocksDB snapshots. Enable it once per clone:

```bash
git config core.hooksPath .githooks
```

If the guard ever blocks a legitimate commit, the file belongs in the **private**
data repo (§2), not here.
