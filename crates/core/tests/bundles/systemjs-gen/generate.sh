#!/usr/bin/env bash
# Generates SystemJS test fixtures from the source files in src/.
# Requires Node.js + npm (uses npx to fetch Rollup, SWC, and TypeScript on-the-fly).
#
# Usage:
#   cd crates/core/tests/bundles/systemjs-gen
#   bash generate.sh
#
# Generated outputs are checked into the repo so tests do not require Node.js.

set -euo pipefail
cd "$(dirname "$0")"

ROLLUP_VERSION="4.29.1"
SWC_CLI_VERSION="0.7.9"
SWC_CORE_VERSION="1.15.3"
TYPESCRIPT_VERSION="5.9.3"

rm -rf dist

echo "=== Rollup ${ROLLUP_VERSION} ==="

echo "  preserve: System.register preserveModules output"
npx --yes rollup@${ROLLUP_VERSION} src/entry.js \
  --format system \
  --preserveModules \
  --dir dist/preserve

echo ""
echo "=== SWC ${SWC_CORE_VERSION} ==="

echo "  swc: module.type=systemjs compiler output"
npx --yes -p @swc/cli@${SWC_CLI_VERSION} -p @swc/core@${SWC_CORE_VERSION} swc src \
  -d dist/swc \
  --config-file swc.swcrc

echo ""
echo "=== TypeScript ${TYPESCRIPT_VERSION} ==="

echo "  tsc: --module system compiler output"
npx --yes -p typescript@${TYPESCRIPT_VERSION} tsc src-ts/entry.ts src-ts/dep.ts \
  --module system \
  --target es2018 \
  --outDir dist/tsc

echo ""
echo "Done. Outputs in dist/*/"
