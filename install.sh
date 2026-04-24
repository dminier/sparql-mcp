#!/usr/bin/env bash
set -euo pipefail

# install.sh — one-line installer for sparql-mcp.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/dminier/sparql-mcp/main/install.sh | bash
#   curl -fsSL ... | bash -s -- --dir=/opt/bin
#   curl -fsSL ... | bash -s -- --skip-config
#
# Environment:
#   SPARQL_MCP_DOWNLOAD_URL  Override base URL (for mirrors / local testing).
#   SPARQL_MCP_VERSION       Pin a specific version (defaults to `latest`).

main() {

REPO="dminier/sparql-mcp"
VERSION="${SPARQL_MCP_VERSION:-latest}"
INSTALL_DIR="$HOME/.local/bin"
SKIP_CONFIG=false

if [ "$VERSION" = "latest" ]; then
    BASE_URL="https://github.com/${REPO}/releases/latest/download"
else
    BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
fi
BASE_URL="${SPARQL_MCP_DOWNLOAD_URL:-$BASE_URL}"

case "$BASE_URL" in
    https://*|http://localhost*|http://127.0.0.1*) ;;
    *) echo "error: refusing non-HTTPS download URL: $BASE_URL" >&2; exit 1 ;;
esac

for arg in "$@"; do
    case "$arg" in
        --dir=*)       INSTALL_DIR="${arg#--dir=}" ;;
        --skip-config) SKIP_CONFIG=true ;;
        --help|-h)
            echo "Usage: install.sh [--dir=<path>] [--skip-config]"
            exit 0
            ;;
    esac
done

detect_os() {
    case "$(uname -s)" in
        Darwin)               echo "darwin" ;;
        Linux)                echo "linux" ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
        *) echo "error: unsupported OS: $(uname -s)" >&2; exit 1 ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        arm64|aarch64) echo "arm64" ;;
        x86_64|amd64)  echo "amd64" ;;
        *) echo "error: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
    esac
}

OS="$(detect_os)"
ARCH="$(detect_arch)"
EXT="tar.gz"
[ "$OS" = "windows" ] && EXT="zip"

ARCHIVE="sparql-mcp-${OS}-${ARCH}.${EXT}"
URL="${BASE_URL}/${ARCHIVE}"
SUMS_URL="${BASE_URL}/SHA256SUMS"

DLDIR="$(mktemp -d)"
trap 'rm -rf "$DLDIR"' EXIT

echo "Downloading $URL"
if ! curl -fL --proto '=https' --tlsv1.2 -o "$DLDIR/$ARCHIVE" "$URL"; then
    echo "error: download failed. See https://github.com/${REPO}/releases" >&2
    exit 1
fi

# Verify checksum against SHA256SUMS if available.
if curl -fsL --proto '=https' --tlsv1.2 -o "$DLDIR/SHA256SUMS" "$SUMS_URL" 2>/dev/null; then
    EXPECTED="$(grep "$ARCHIVE" "$DLDIR/SHA256SUMS" | awk '{print $1}')"
    if [ -n "$EXPECTED" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL="$(sha256sum "$DLDIR/$ARCHIVE" | awk '{print $1}')"
        else
            ACTUAL="$(shasum -a 256 "$DLDIR/$ARCHIVE" | awk '{print $1}')"
        fi
        if [ "$EXPECTED" != "$ACTUAL" ]; then
            echo "error: CHECKSUM MISMATCH" >&2
            echo "  expected: $EXPECTED" >&2
            echo "  actual:   $ACTUAL" >&2
            exit 1
        fi
        echo "Checksum verified."
    fi
else
    echo "warning: SHA256SUMS not found at release; skipping checksum verification." >&2
fi

echo "Extracting..."
cd "$DLDIR"
if [ "$EXT" = "zip" ]; then
    unzip -q "$ARCHIVE"
else
    tar -xzf "$ARCHIVE"
fi

EXTRACTED_DIR="$DLDIR/sparql-mcp-${OS}-${ARCH}"
DLBIN="$EXTRACTED_DIR/sparql-mcp"
if [ ! -f "$DLBIN" ]; then
    echo "error: 'sparql-mcp' binary not found after extraction" >&2
    exit 1
fi

if [ "$OS" = "darwin" ]; then
    xattr -d com.apple.quarantine "$DLBIN" 2>/dev/null || true
    codesign --sign - --force "$DLBIN" 2>/dev/null || true
fi

mkdir -p "$INSTALL_DIR"
DEST="$INSTALL_DIR/sparql-mcp"
[ -f "$DEST" ] && rm -f "$DEST"
cp "$DLBIN" "$DEST"
chmod 755 "$DEST"

VERSION_STR=$("$DEST" --version 2>&1 || echo "unknown")
echo "Installed: $DEST ($VERSION_STR)"

# Seed the per-user ontology directory from the extracted archive. Agents
# launched from any cwd will find the core ontology at its XDG location.
DATA_HOME="${SPARQL_MCP_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/sparql-mcp}"
SRC_ONT=""
for candidate in "$DLDIR/sparql-mcp-${OS}-${ARCH}/ontology" "$DLDIR/ontology"; do
    if [ -d "$candidate" ]; then SRC_ONT="$candidate"; break; fi
done
if [ -n "$SRC_ONT" ]; then
    mkdir -p "$DATA_HOME/ontology"
    for f in "$SRC_ONT"/*.ttl; do
        [ -f "$f" ] || continue
        dst="$DATA_HOME/ontology/$(basename "$f")"
        # Never clobber user edits.
        if [ ! -e "$dst" ]; then cp "$f" "$dst"; fi
    done
    echo "Ontology: $DATA_HOME/ontology"
fi

if [ "$SKIP_CONFIG" = false ]; then
    echo ""
    echo "Configuring detected coding agents..."
    "$DEST" install -y || {
        echo "agent configuration failed (non-fatal)" >&2
        echo "run manually: $DEST install" >&2
    }
fi

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    echo ""
    echo "NOTE: $INSTALL_DIR is not in your PATH."
    echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.zshrc   # or ~/.bashrc"
fi

echo ""
echo "Done. Restart your coding agent."

}  # end main

main "$@"
