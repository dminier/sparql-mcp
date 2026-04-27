#!/usr/bin/env bash
# Print git metadata of a repo as JSON. Exits 2 if not a git repo (caller decides
# whether to fall back to a manual provenance tag).
set -euo pipefail
repo="${1:?usage: git_meta.sh <repo_path>}"
cd "$repo"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 2

branch=$(git rev-parse --abbrev-ref HEAD)
sha=$(git rev-parse HEAD)
short=$(git rev-parse --short HEAD)
cdate=$(git log -1 --format=%cI)
cmsg=$(git log -1 --format=%s)
remote=$(git remote get-url origin 2>/dev/null || echo "")
[ -z "$(git status --porcelain)" ] && dirty=false || dirty=true
slug=$(basename "$(git rev-parse --show-toplevel)")

python3 -c '
import json, sys
print(json.dumps({
  "repo": sys.argv[1], "repoPath": sys.argv[2],
  "gitBranch": sys.argv[3], "gitCommit": sys.argv[4],
  "gitCommitShort": sys.argv[5], "gitCommitDate": sys.argv[6],
  "gitCommitMsg": sys.argv[7], "gitRemote": sys.argv[8],
  "gitDirty": sys.argv[9] == "true",
}, ensure_ascii=False))
' "$slug" "$(pwd)" "$branch" "$sha" "$short" "$cdate" "$cmsg" "$remote" "$dirty"
