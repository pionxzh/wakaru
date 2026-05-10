#!/usr/bin/env bash
# Generates Vite/Rollup test fixtures from the source files in src/.
# Requires: Node.js + npm
#
# Usage:
#   cd wakaru-rs/tests/fixtures/vite-gen
#   npm install
#   bash generate.sh
#
# Each configuration produces a bundle in dist/<name>/bundle.mjs.
# The generated outputs are checked into the repo so tests don't require Node.js.

set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist

VITE="./node_modules/.bin/vite"

echo "=== vite $(${VITE} --version) (rollup) ==="

echo "  es:     ESM scope-hoisted (unminified)"
${VITE} build --config vite.config.mjs

echo "  es-min: ESM scope-hoisted (minified)"
${VITE} build --config vite-min.config.mjs

echo ""
echo "Done. Outputs in dist/*/"
