#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

wasm-pack build "$REPO_ROOT/crates/wasm" \
  --target web \
  --out-dir "$REPO_ROOT/crates/wasm/pkg" \
  --release
