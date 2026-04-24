#!/usr/bin/env bash
# start_sparql_http.sh — run a single sparql-mcp server behind an SSE bridge.
#
# Rationale: sparql-mcp is a STDIO MCP server, and its oxigraph store takes
# an exclusive rocksdb process-lock. Two Claude sessions each spawning their
# own STDIO server would fight for the lock and one would fail. By running
# sparql-mcp once under mcp-proxy and exposing it as SSE on 127.0.0.1, every
# Claude session / subagent connects to the same server process and shares
# the store cleanly.
#
# Usage:
#   scripts/start_sparql_http.sh            # foreground, Ctrl-C to stop
#   scripts/start_sparql_http.sh --bg       # background, PID in .sparql_http.pid
#
# Logs: .sparql_http.log (in the repo root)
# Port: SPARQL_HTTP_PORT env var, default 7733

set -euo pipefail

# Resolve repo root by walking up looking for sparql-mcp.toml. Works whether
# invoked from the repo checkout, a symlinked .claude/skills, or the plugin
# cache at ~/.claude/plugins/cache/.../kb-workbench/.
find_repo_root() {
  if [[ -n "${SPARQL_MCP_REPO:-}" ]]; then echo "$SPARQL_MCP_REPO"; return; fi
  local d="${PWD}"
  while [[ "$d" != "/" ]]; do
    [[ -f "$d/sparql-mcp.toml" ]] && { echo "$d"; return; }
    d="$(dirname "$d")"
  done
  echo "error: could not locate sparql-mcp.toml from $PWD." \
       "cd into the repo, or export SPARQL_MCP_REPO=/path/to/repo" >&2
  exit 1
}

REPO_ROOT="$(find_repo_root)"
cd "$REPO_ROOT"

PORT="${SPARQL_HTTP_PORT:-7733}"
HOST="${SPARQL_HTTP_HOST:-127.0.0.1}"
BIN="${SPARQL_MCP_BIN:-$REPO_ROOT/target/debug/sparql-mcp}"
PIDFILE="$REPO_ROOT/.sparql_http.pid"
LOGFILE="$REPO_ROOT/.sparql_http.log"

if [[ ! -x "$BIN" ]]; then
  echo "error: sparql-mcp binary not found at $BIN" >&2
  echo "build it first: cargo build -p sparql-mcp" >&2
  exit 1
fi

if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
  echo "sparql-mcp bridge already running (PID $(cat "$PIDFILE")) on $HOST:$PORT"
  exit 0
fi

# sparql-mcp uses tracing; in STDIO mode logs on stdout would corrupt the
# JSONRPC stream that mcp-proxy pipes. Force log level to off unless caller
# overrides RUST_LOG.
export RUST_LOG="${RUST_LOG:-off}"

CMD=(mcp-proxy --host "$HOST" --port "$PORT" --pass-environment -- "$BIN" serve --config sparql-mcp.toml)

if [[ "${1:-}" == "--bg" ]]; then
  nohup "${CMD[@]}" >"$LOGFILE" 2>&1 &
  echo $! >"$PIDFILE"
  sleep 1
  if kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
    echo "sparql-mcp bridge started on http://$HOST:$PORT/sse (PID $(cat "$PIDFILE"))"
    echo "log: $LOGFILE"
  else
    echo "error: bridge failed to start — see $LOGFILE" >&2
    rm -f "$PIDFILE"
    exit 1
  fi
else
  echo "sparql-mcp bridge on http://$HOST:$PORT/sse  (Ctrl-C to stop)"
  exec "${CMD[@]}"
fi
