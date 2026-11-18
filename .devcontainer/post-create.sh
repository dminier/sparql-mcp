#!/usr/bin/env bash
set -euo pipefail

sudo chown -R "$(id -u):$(id -g)" \
  "$(pwd)/target" \
  /usr/local/cargo/registry 2>/dev/null || true

rustup component add rustfmt clippy

cargo fetch --locked
