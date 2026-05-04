#!/usr/bin/env bash
# Generates esbuild test fixtures from the source files in src/.
# Bun fixtures live here too because they exercise the same esbuild-style
# scope-hoisted ESM unpacker, not a separate Bun-specific detector.
# Requires:
#   - Node.js + npm (uses npx to fetch esbuild on-the-fly)
#   - Bun (for Bun comparison fixtures)
#
# Usage:
#   cd wakaru-rs/tests/fixtures/esbuild-gen
#   bash generate.sh
#
# Each configuration produces a bundle in dist/<name>/bundle.js.
# The generated outputs are checked into the repo so tests don't require Node.js.

set -euo pipefail
cd "$(dirname "$0")"

ESBUILD_VERSION="0.25.4"

rm -rf dist

echo "=== esbuild ${ESBUILD_VERSION} ==="

echo "  es-scope-only:         ESM scope-hoisted only (no factories)"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-scope-only.js \
  --bundle --format=esm --outfile=dist/es-scope-only/bundle.js

echo "  es-scope-side-effects: ESM scope-hoisted with module-level side effects"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-scope-side-effects.js \
  --bundle --format=esm --outfile=dist/es-scope-side-effects/bundle.js

echo "  es-mixed:              ESM with both factories (CJS) and scope-hoisted modules"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-mixed.js \
  --bundle --format=esm --outfile=dist/es-mixed/bundle.js

echo "  es-global-side-effect: ESM scope-hoisted with global-call side effect using module binding"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-global-side-effect.js \
  --bundle --format=esm --outfile=dist/es-global-side-effect/bundle.js

echo "  es-entry-expr:         ESM scope-hoisted + entry-level expression statement"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-entry-expr.js \
  --bundle --format=esm --outfile=dist/es-entry-expr/bundle.js

echo "  es-single-boundary:    ESM scope-hoisted with one namespace boundary"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-single-boundary.js \
  --bundle --format=esm --outfile=dist/es-single-boundary/bundle.js

echo "  es-private-helper:     ESM scope-hoisted with private helper + var entry decl"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-private-helper.js \
  --bundle --format=esm --outfile=dist/es-private-helper/bundle.js

echo "  es-helper-after-export: ESM scope-hoisted with private helper after export"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-helper-after-export.js \
  --bundle --format=esm --outfile=dist/es-helper-after-export/bundle.js

echo "  iife-factories:        IIFE with factory pattern"
npx --yes esbuild@${ESBUILD_VERSION} src/entry-factories.js \
  --bundle --format=iife --outfile=dist/iife-factories/bundle.js

echo ""
echo "=== bun $(bun --version) ==="

echo "  bun-scope-only-min:         minified ESM scope-hoisted only"
bun build src/entry-scope-only.js \
  --target=browser --format=esm --minify --outfile=dist/bun-scope-only-min/bundle.js

echo "  bun-scope-side-effects-min: minified ESM scope-hoisted with module side effects"
bun build src/entry-scope-side-effects.js \
  --target=browser --format=esm --minify --outfile=dist/bun-scope-side-effects-min/bundle.js

echo "  bun-mixed-min:              minified ESM with inlined CJS + scope-hoisted namespaces"
bun build src/entry-mixed.js \
  --target=browser --format=esm --minify --outfile=dist/bun-mixed-min/bundle.js

echo "  bun-single-boundary-min:    minified ESM scope-hoisted with one namespace boundary"
bun build src/entry-single-boundary.js \
  --target=browser --format=esm --minify --outfile=dist/bun-single-boundary-min/bundle.js

echo "  bun-helper-after-export-min: minified ESM with private helper after export"
bun build src/entry-helper-after-export.js \
  --target=browser --format=esm --minify --outfile=dist/bun-helper-after-export-min/bundle.js

echo "  bun-factories-min:          minified Bun bundle without namespace boundaries"
bun build src/entry-factories.js \
  --target=browser --format=esm --minify --outfile=dist/bun-factories-min/bundle.js

echo ""
echo "Done. Outputs in dist/*/"
