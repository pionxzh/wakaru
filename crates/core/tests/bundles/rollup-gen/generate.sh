#!/usr/bin/env bash
# Generates Rollup test fixtures from the source files in src/.
# Requires: Node.js + npm
#
# Usage:
#   cd crates/core/tests/bundles/rollup-gen
#   npm install
#   bash generate.sh

set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist

ROLLUP="./node_modules/.bin/rollup"

echo "=== rollup $(${ROLLUP} --version) ==="

echo "  es:     ESM scope-hoisted (unminified)"
echo "  es-min: ESM scope-hoisted (minified)"
${ROLLUP} --config rollup.config.mjs

echo ""
echo "Done. Outputs in dist/*/"
