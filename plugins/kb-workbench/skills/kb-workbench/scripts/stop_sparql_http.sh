#!/usr/bin/env bash
# stop_sparql_http.sh — stop the SSE bridge started by start_sparql_http.sh
set -euo pipefail

find_repo_root() {
  if [[ -n "${SPARQL_MCP_REPO:-}" ]]; then echo "$SPARQL_MCP_REPO"; return; fi
  local d="${PWD}"
  while [[ "$d" != "/" ]]; do
    [[ -f "$d/sparql-mcp.toml" ]] && { echo "$d"; return; }
    d="$(dirname "$d")"
  done
  echo "error: could not locate sparql-mcp.toml from $PWD" >&2
  exit 1
}
REPO_ROOT="$(find_repo_root)"
PIDFILE="$REPO_ROOT/.sparql_http.pid"

if [[ ! -f "$PIDFILE" ]]; then
  echo "no pidfile at $PIDFILE — bridge not running?"
  exit 0
fi

PID="$(cat "$PIDFILE")"
if kill -0 "$PID" 2>/dev/null; then
  kill "$PID"
  for _ in 1 2 3 4 5; do
    kill -0 "$PID" 2>/dev/null || break
    sleep 0.5
  done
  kill -0 "$PID" 2>/dev/null && kill -9 "$PID" || true
  echo "stopped sparql-mcp bridge (PID $PID)"
else
  echo "stale pidfile (PID $PID not running)"
fi
rm -f "$PIDFILE"
