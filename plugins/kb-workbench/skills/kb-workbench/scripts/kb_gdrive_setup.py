#!/usr/bin/env python3
"""
kb_gdrive_setup.py — One-shot rclone + GDrive setup for KB sync

Idempotent: safe to re-run. Handles:
  1. Install rclone to ~/.local/bin if missing
  2. Run `rclone authorize drive` to fetch OAuth token (user visits URL once)
  3. Write rclone config with remote name "gdrive"
  4. Test with `rclone lsd gdrive:`
  5. Resolve/create the sparql-kb folder and patch sparql-mcp.toml

Usage:
  python3 kb_gdrive_setup.py
  python3 kb_gdrive_setup.py --token-only   # just print rclone authorize output
  python3 kb_gdrive_setup.py --check        # only verify current setup
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

RCLONE_BIN = Path.home() / ".local" / "bin" / "rclone"
RCLONE_CONF = Path.home() / ".config" / "rclone" / "rclone.conf"
REMOTE_NAME = "gdrive"
ROOT_FOLDER = "sparql-kb"
SPARQL_MCP_TOML = Path.home() / "projects" / "sparql-mcp" / "sparql-mcp.toml"


def log(msg: str):
    print(f"[kb-gdrive-setup] {msg}", file=sys.stderr)


def fail(msg: str, code: int = 1):
    print(f"[kb-gdrive-setup] ERROR: {msg}", file=sys.stderr)
    sys.exit(code)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


# ── 1. Install rclone ────────────────────────────────────────────────────────

def ensure_rclone() -> Path:
    if RCLONE_BIN.exists():
        version = run([str(RCLONE_BIN), "--version"]).stdout.splitlines()[0]
        log(f"rclone present: {version}")
        return RCLONE_BIN

    sys_rclone = shutil.which("rclone")
    if sys_rclone:
        log(f"rclone in PATH: {sys_rclone}")
        return Path(sys_rclone)

    log("rclone not found, installing to ~/.local/bin/")
    RCLONE_BIN.parent.mkdir(parents=True, exist_ok=True)

    import platform
    arch = "amd64" if platform.machine() in ("x86_64", "amd64") else "arm64"
    osname = "linux" if sys.platform == "linux" else "osx"

    url = f"https://downloads.rclone.org/rclone-current-{osname}-{arch}.zip"
    log(f"Downloading {url}")
    with tempfile.TemporaryDirectory() as td:
        zip_path = Path(td) / "rclone.zip"
        urllib.request.urlretrieve(url, zip_path)
        import zipfile
        with zipfile.ZipFile(zip_path) as z:
            for member in z.namelist():
                if member.endswith("/rclone"):
                    z.extract(member, td)
                    extracted = Path(td) / member
                    shutil.copy2(extracted, RCLONE_BIN)
                    RCLONE_BIN.chmod(0o755)
                    log(f"Installed {RCLONE_BIN}")
                    return RCLONE_BIN
    fail("Could not extract rclone binary from zip")


# ── 2. Authorize ─────────────────────────────────────────────────────────────

def remote_configured(rclone: Path) -> bool:
    result = run([str(rclone), "listremotes"])
    return f"{REMOTE_NAME}:" in result.stdout


def authorize(rclone: Path) -> str:
    """
    Run `rclone authorize drive` — this prints an auth URL and waits.
    The user visits it in their browser and authorizes; rclone captures
    the callback on localhost and prints a JSON token to stdout.
    Returns the JSON token string.
    """
    log("")
    log("=" * 60)
    log("rclone authorize will print an URL. Visit it in your browser,")
    log("authorize, and rclone will auto-capture the token.")
    log("=" * 60)
    log("")

    proc = subprocess.Popen(
        [str(rclone), "authorize", "drive"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )

    token = None
    captured = []
    for line in proc.stdout:
        sys.stderr.write(line)
        sys.stderr.flush()
        captured.append(line)
        m = re.search(r'(\{"access_token".+\})', line)
        if m:
            token = m.group(1)
            break

    proc.wait(timeout=300)
    if not token:
        for line in captured:
            m = re.search(r'(\{"access_token".+\})', line)
            if m:
                token = m.group(1)
                break

    if not token:
        fail("Did not receive token from rclone authorize")
    return token


def write_rclone_config(token: str):
    RCLONE_CONF.parent.mkdir(parents=True, exist_ok=True)

    existing = ""
    if RCLONE_CONF.exists():
        existing = RCLONE_CONF.read_text()
        existing = re.sub(
            rf"\[{REMOTE_NAME}\].*?(?=\n\[|\Z)",
            "",
            existing,
            flags=re.DOTALL,
        ).strip()
        if existing:
            existing += "\n\n"

    block = (
        f"[{REMOTE_NAME}]\n"
        f"type = drive\n"
        f"scope = drive\n"
        f"token = {token}\n"
    )
    RCLONE_CONF.write_text(existing + block)
    RCLONE_CONF.chmod(0o600)
    log(f"Wrote {RCLONE_CONF}")


# ── 3. Test connection & resolve folder ──────────────────────────────────────

def test_connection(rclone: Path):
    result = run([str(rclone), "lsd", f"{REMOTE_NAME}:"])
    if result.returncode != 0:
        fail(f"rclone lsd {REMOTE_NAME}: failed:\n{result.stderr}")
    log("rclone connection OK")


def resolve_root_folder(rclone: Path) -> str:
    """Find or create sparql-kb at the root, return its file ID."""
    result = run([
        str(rclone), "lsjson", f"{REMOTE_NAME}:", "--dirs-only",
    ])
    if result.returncode != 0:
        fail(f"rclone lsjson failed: {result.stderr}")

    entries = json.loads(result.stdout)
    for e in entries:
        if e.get("Name") == ROOT_FOLDER and e.get("IsDir"):
            log(f"Found existing folder {ROOT_FOLDER} (id={e['ID']})")
            return e["ID"]

    log(f"Creating folder {ROOT_FOLDER} at Drive root")
    mkresult = run([str(rclone), "mkdir", f"{REMOTE_NAME}:{ROOT_FOLDER}"])
    if mkresult.returncode != 0:
        fail(f"mkdir failed: {mkresult.stderr}")

    result = run([str(rclone), "lsjson", f"{REMOTE_NAME}:", "--dirs-only"])
    entries = json.loads(result.stdout)
    for e in entries:
        if e.get("Name") == ROOT_FOLDER:
            return e["ID"]
    fail("Created folder but cannot retrieve its ID")


# ── 4. Patch sparql-mcp.toml ─────────────────────────────────────────────────

def patch_toml(folder_id: str):
    if not SPARQL_MCP_TOML.exists():
        log(f"sparql-mcp.toml not at {SPARQL_MCP_TOML} — skipping patch")
        return

    text = SPARQL_MCP_TOML.read_text()
    block = f'[gdrive]\nenabled   = true\nfolder_id = "{folder_id}"\n'

    if "[gdrive]" in text:
        text = re.sub(
            r"\[gdrive\][^\[]*",
            block + "\n",
            text,
            count=1,
            flags=re.DOTALL,
        )
    else:
        if not text.endswith("\n"):
            text += "\n"
        text += "\n" + block

    SPARQL_MCP_TOML.write_text(text)
    log(f"Patched {SPARQL_MCP_TOML} with folder_id={folder_id}")


# ── 5. Check-only mode ───────────────────────────────────────────────────────

def cmd_check():
    rclone = RCLONE_BIN if RCLONE_BIN.exists() else (shutil.which("rclone") and Path(shutil.which("rclone")))
    out = {
        "rclone_installed": bool(rclone),
        "rclone_path": str(rclone) if rclone else None,
        "config_exists": RCLONE_CONF.exists(),
        "remote_configured": False,
        "remote_works": False,
        "sparql_kb_folder_id": None,
    }
    if rclone:
        out["remote_configured"] = remote_configured(rclone)
        if out["remote_configured"]:
            r = run([str(rclone), "lsd", f"{REMOTE_NAME}:"])
            out["remote_works"] = r.returncode == 0
            if out["remote_works"]:
                try:
                    out["sparql_kb_folder_id"] = resolve_root_folder(rclone)
                except SystemExit:
                    pass
    print(json.dumps(out, indent=2))


# ── main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Set up rclone + GDrive for KB sync")
    parser.add_argument("--check", action="store_true", help="Verify current setup, don't change anything")
    parser.add_argument("--force", action="store_true", help="Re-authorize even if remote exists")
    args = parser.parse_args()

    if args.check:
        cmd_check()
        return

    rclone = ensure_rclone()

    if remote_configured(rclone) and not args.force:
        log(f"Remote '{REMOTE_NAME}' already configured (use --force to re-auth)")
    else:
        token = authorize(rclone)
        write_rclone_config(token)

    test_connection(rclone)
    folder_id = resolve_root_folder(rclone)
    patch_toml(folder_id)

    print(json.dumps({
        "status": "ok",
        "rclone": str(rclone),
        "remote": REMOTE_NAME,
        "folder_id": folder_id,
        "sparql_mcp_toml": str(SPARQL_MCP_TOML),
    }, indent=2))


if __name__ == "__main__":
    main()
