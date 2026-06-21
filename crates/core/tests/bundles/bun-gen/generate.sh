#!/usr/bin/env bash
# Generates Bun bundler test fixtures from the source files in src/.
# Requires: bun
#
# Usage:
#   cd crates/core/tests/bundles/bun-gen
#   bash generate.sh

set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist

echo "=== bun $(bun --version) ==="

echo "  es:         ESM scope-hoisted (unminified)"
bun build src/entry.js --outdir dist/es --format esm

echo "  es-min:     ESM scope-hoisted (minified)"
bun build src/entry.js --outdir dist/es-min --format esm --minify

echo "  cjs-interop:     ESM importing CJS (unminified)"
bun build src-cjs/entry-cjs.js --outdir dist/cjs-interop --format esm

echo "  cjs-interop-min: ESM importing CJS (minified)"
bun build src-cjs/entry-cjs.js --outdir dist/cjs-interop-min --format esm --minify

echo ""
echo "Done. Outputs in dist/*/"
