#!/usr/bin/env python3
"""
kb_gdrive_sync.py — KB GDrive sync helper script

Architecture:
  The agent handles MCP calls (sparql-mcp graph export, GDrive create_file).
  This script handles everything else: compression, rclone upload, manifest.

Usage:
  python3 kb_gdrive_sync.py compress  --src /tmp/kb-sync --out /tmp/kb-sync/kb-all-graphs-<ts>.tar.gz
  python3 kb_gdrive_sync.py upload    --file /tmp/kb-sync/kb-all-graphs-<ts>.tar.gz --remote gdrive:sparql-kb/store-backups/ttl/
  python3 kb_gdrive_sync.py manifest  --folder-id <id> --graphs <graph1> <graph2> ... --archive <name> --out /tmp/kb-sync/manifest.json
  python3 kb_gdrive_sync.py b64check  --file <path>    # prints size + b64 size, exits 0 if <4MB
  python3 kb_gdrive_sync.py b64       --file <path>    # prints raw base64 (only for files that passed b64check)
  python3 kb_gdrive_sync.py status    --manifest /tmp/kb-sync/manifest.json

All subcommands print JSON to stdout so the agent can parse results.
"""

import argparse
import base64
import json
import os
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
from datetime import datetime, timezone
from pathlib import Path

RCLONE = os.path.expanduser("~/.local/bin/rclone")
MCP_B64_LIMIT_BYTES = 4 * 1024 * 1024   # 4 MB raw → ~5.3 MB base64, safe margin
COMPRESS_EXTS = {".ttl", ".n3", ".nq", ".jsonld", ".owl"}


# ── helpers ──────────────────────────────────────────────────────────────────

def ok(data: dict):
    print(json.dumps({"status": "ok", **data}))
    sys.exit(0)


def err(msg: str, **extra):
    print(json.dumps({"status": "error", "message": msg, **extra}), file=sys.stderr)
    sys.exit(1)


def ts_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def rclone_available() -> bool:
    return Path(RCLONE).exists() or shutil.which("rclone") is not None


def rclone_configured(remote: str) -> bool:
    """Check that the remote name exists in rclone config."""
    name = remote.split(":")[0]
    rc = shutil.which("rclone") or RCLONE
    result = subprocess.run([rc, "listremotes"], capture_output=True, text=True)
    return f"{name}:" in result.stdout


# ── subcommands ───────────────────────────────────────────────────────────────

def cmd_compress(args):
    """Pack all TTL files in --src into a single gzip archive."""
    src = Path(args.src)
    if not src.is_dir():
        err(f"--src {src} is not a directory")

    files = [f for f in src.iterdir() if f.suffix in COMPRESS_EXTS]
    if not files:
        err(f"No TTL/RDF files found in {src}")

    out = Path(args.out) if args.out else src / f"kb-all-graphs-{ts_now()}.tar.gz"
    out.parent.mkdir(parents=True, exist_ok=True)

    with tarfile.open(out, "w:gz") as tar:
        for f in sorted(files):
            tar.add(f, arcname=f.name)

    size = out.stat().st_size
    b64_size = (size * 4 + 2) // 3   # ceiling

    ok({
        "archive": str(out),
        "files": len(files),
        "size_bytes": size,
        "size_mb": round(size / 1024 / 1024, 2),
        "b64_size_bytes": b64_size,
        "upload_via": "mcp" if size < MCP_B64_LIMIT_BYTES else "rclone",
    })


def cmd_upload(args):
    """Upload a file via rclone. Requires rclone configured with a GDrive remote."""
    path = Path(args.file)
    if not path.exists():
        err(f"File not found: {path}")

    if not rclone_available():
        err(
            "rclone not found. Install with: "
            "curl -s https://rclone.org/install.sh | bash  "
            "then run: rclone config  (choose Google Drive, named 'gdrive')"
        )

    remote = args.remote  # e.g. "gdrive:sparql-kb/store-backups/ttl/"
    if not rclone_configured(remote):
        err(
            f"rclone remote '{remote.split(':')[0]}' not configured. "
            "Run: rclone config  and add a Google Drive remote named 'gdrive'."
        )

    rc = shutil.which("rclone") or RCLONE
    result = subprocess.run(
        [rc, "copy", "--progress", str(path), remote],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        err("rclone upload failed", stderr=result.stderr, stdout=result.stdout)

    ok({
        "uploaded": str(path),
        "remote": remote,
        "size_bytes": path.stat().st_size,
    })


def cmd_b64check(args):
    """Check if a file is small enough for MCP base64 upload (<4MB raw)."""
    path = Path(args.file)
    if not path.exists():
        err(f"File not found: {path}")

    size = path.stat().st_size
    b64_size = (size * 4 + 2) // 3
    fits = size < MCP_B64_LIMIT_BYTES

    ok({
        "file": str(path),
        "size_bytes": size,
        "size_mb": round(size / 1024 / 1024, 2),
        "b64_size_bytes": b64_size,
        "fits_mcp": fits,
        "upload_via": "mcp" if fits else "rclone",
    })


def cmd_b64(args):
    """Print raw base64 of a file (only for files < 4 MB — check with b64check first)."""
    path = Path(args.file)
    if not path.exists():
        err(f"File not found: {path}")

    size = path.stat().st_size
    if size >= MCP_B64_LIMIT_BYTES:
        err(
            f"File is {size / 1024 / 1024:.1f} MB — exceeds MCP limit. "
            "Use 'upload' subcommand with rclone instead."
        )

    data = path.read_bytes()
    # Print raw base64 (not JSON) so agent can use it directly in create_file
    sys.stdout.write(base64.b64encode(data).decode())
    sys.stdout.flush()
    sys.exit(0)


def cmd_manifest(args):
    """Generate sync-manifest.json and encode it as base64 for MCP upload."""
    graphs = args.graphs or []
    manifest = {
        "machine": socket.gethostname(),
        "pushed_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "folder_id": args.folder_id,
        "graphs": graphs,
        "archive": args.archive or "",
        "note": "Generated by kb_gdrive_sync.py",
    }

    out = Path(args.out) if args.out else Path(tempfile.mktemp(suffix=".json"))
    out.write_text(json.dumps(manifest, indent=2))

    b64 = base64.b64encode(out.read_bytes()).decode()

    ok({
        "manifest_path": str(out),
        "b64": b64,
        "manifest": manifest,
    })


def cmd_status(args):
    """Read a local manifest and report sync status."""
    path = Path(args.manifest)
    if not path.exists():
        err(f"Manifest not found: {path}. Run 'kb sync push' first.")

    manifest = json.loads(path.read_text())
    pushed_at = manifest.get("pushed_at", "unknown")
    graphs = manifest.get("graphs", [])

    ok({
        "machine": manifest.get("machine"),
        "pushed_at": pushed_at,
        "graphs_count": len(graphs),
        "graphs": graphs,
        "archive": manifest.get("archive"),
    })


# ── main ──────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="KB GDrive sync helper")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("compress", help="Pack TTL files into a single tar.gz")
    p.add_argument("--src", required=True, help="Directory containing exported TTL files")
    p.add_argument("--out", help="Output archive path (default: <src>/kb-all-graphs-<ts>.tar.gz)")

    p = sub.add_parser("upload", help="Upload a file to GDrive via rclone")
    p.add_argument("--file", required=True, help="Local file to upload")
    p.add_argument("--remote", required=True, help="rclone remote path, e.g. gdrive:sparql-kb/store-backups/ttl/")

    p = sub.add_parser("b64check", help="Check if a file fits MCP base64 upload limit (<4MB)")
    p.add_argument("--file", required=True)

    p = sub.add_parser("b64", help="Print raw base64 of a file (for MCP create_file content field)")
    p.add_argument("--file", required=True)

    p = sub.add_parser("manifest", help="Generate and base64-encode sync-manifest.json")
    p.add_argument("--folder-id", required=True, help="GDrive folder ID")
    p.add_argument("--graphs", nargs="*", help="List of graph IRIs")
    p.add_argument("--archive", help="Archive filename uploaded to GDrive")
    p.add_argument("--out", help="Local path to write manifest JSON")

    p = sub.add_parser("status", help="Show sync status from local manifest")
    p.add_argument("--manifest", default="/tmp/kb-sync/manifest.json")

    args = parser.parse_args()
    {
        "compress": cmd_compress,
        "upload": cmd_upload,
        "b64check": cmd_b64check,
        "b64": cmd_b64,
        "manifest": cmd_manifest,
        "status": cmd_status,
    }[args.cmd](args)


if __name__ == "__main__":
    main()
