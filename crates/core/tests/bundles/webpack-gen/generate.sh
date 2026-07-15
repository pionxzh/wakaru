#!/usr/bin/env bash
# Generates webpack test fixtures from the source files in src/.
# Requires: Node.js + npm (uses npx to fetch webpack on-the-fly).
#
# Usage:
#   cd tests/bundles/webpack-gen
#   bash generate.sh
#
# Each config produces a checked-in bundle under dist/<name>/.
# The generated outputs are checked into the repo so tests don't require Node.js.

set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist

echo "=== Webpack 4 (4.47.0) ==="

echo "  wp4-cjs:           CJS-only modules (dev, string IDs, object map)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-cjs.config.cjs 2>/dev/null

echo "  wp4-umd:           CJS-only modules wrapped as a UMD library"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-umd.config.cjs 2>/dev/null

echo "  wp4-amd:           CJS-only modules wrapped as an AMD library"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-amd.config.cjs 2>/dev/null

echo "  wp4-esm:           ESM modules (require.r + require.d 3-arg form)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-esm.config.cjs 2>/dev/null

echo "  wp4-mixed:         ESM entry importing CJS module via require()"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-mixed.config.cjs 2>/dev/null

echo "  wp4-require-n:     ESM entry importing CJS via import (triggers require.n + .a)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-require-n.config.cjs 2>/dev/null

echo "  wp4-prod:          Production (numeric IDs, array, module concatenation, no minify)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-prod.config.cjs 2>/dev/null

echo "  wp4-cjs-min:       CJS-only modules (production, minified)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-cjs-min.config.cjs 2>/dev/null

echo "  wp4-esm-min:       ESM modules (production, minified, concatenated)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-esm-min.config.cjs 2>/dev/null

echo "  wp4-dynamic:       Dynamic import (JSONP chunk: window.webpackJsonp)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-dynamic.config.cjs 2>/dev/null

echo "  wp4-dynamic-min:   Dynamic import (production, minified JSONP chunk)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-dynamic-min.config.cjs 2>/dev/null

echo "  wp4-var-inject:    Var injection (.call(this, require(global.js)))"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-var-inject.config.cjs 2>/dev/null

echo ""
echo "=== Webpack 5 (latest) ==="

echo "  wp5-cjs:           CJS-only modules (dev, string IDs)"
npx --yes webpack-cli@5 --config webpack5-cjs.config.cjs 2>/dev/null

echo "  wp5-esm:           ESM modules (require.r + require.d object form)"
npx --yes webpack-cli@5 --config webpack5-esm.config.cjs 2>/dev/null

echo "  wp5-mixed:         ESM entry importing CJS module via require()"
npx --yes webpack-cli@5 --config webpack5-mixed.config.cjs 2>/dev/null

echo "  wp5-umd:           CJS-only modules wrapped as a UMD library"
npx --yes webpack-cli@5 --config webpack5-umd.config.cjs 2>/dev/null

echo "  wp5-umd-esm:       ESM modules wrapped as a UMD library"
npx --yes webpack-cli@5 --config webpack5-umd-esm.config.cjs 2>/dev/null

echo "  wp5-amd:           CJS-only modules wrapped as an AMD library"
npx --yes webpack-cli@5 --config webpack5-amd.config.cjs 2>/dev/null

echo "  wp5-prod:          Production (fully flattened, nothing to unpack)"
npx --yes webpack-cli@5 --config webpack5-prod.config.cjs 2>/dev/null

echo "  wp5-cjs-min:       CJS-only modules (production, minified)"
npx --yes webpack-cli@5 --config webpack5-cjs-min.config.cjs 2>/dev/null

echo "  wp5-umd-min:       CJS-only modules wrapped as a minified UMD library"
npx --yes webpack-cli@5 --config webpack5-umd-min.config.cjs 2>/dev/null

echo "  wp5-esm-min:       ESM modules (production, fully flattened + minified)"
npx --yes webpack-cli@5 --config webpack5-esm-min.config.cjs 2>/dev/null

echo "  wp5-dynamic:       Dynamic import (async chunk via require())"
npx --yes webpack-cli@5 --config webpack5-dynamic.config.cjs 2>/dev/null

echo "  wp5-dynamic-min:   Dynamic import (production, minified async chunk)"
npx --yes webpack-cli@5 --config webpack5-dynamic-min.config.cjs 2>/dev/null

echo "  wp5-var-inject:    Global access (uses __webpack_require__.g, no var injection)"
npx --yes webpack-cli@5 --config webpack5-var-inject.config.cjs 2>/dev/null

echo "  wp5-require-o:     Split initial chunk startup via __webpack_require__.O"
npx --yes webpack-cli@5 --config webpack5-require-o.config.cjs 2>/dev/null

echo ""
echo "=== Vercel ncc (0.44.1) ==="

echo "  wp5-ncc:           Node CJS bundle with inline webpack startup"
npx --yes @vercel/ncc@0.44.1 build src/ncc-entry.cjs -o dist/wp5-ncc 2>/dev/null

echo "  wp5-ncc-min:       Minified Node CJS bundle with inline webpack startup"
npx --yes @vercel/ncc@0.44.1 build src/ncc-entry.cjs -m -o dist/wp5-ncc-min 2>/dev/null

# ncc's development template emits tab-only padding after a few runtime
# comment markers. Normalize line endings without changing executable content
# so generated fixtures remain reproducible and pass Git's whitespace checks.
node - dist/wp5-ncc/index.cjs dist/wp5-ncc-min/index.cjs <<'NODE'
const fs = require("node:fs");

for (const filename of process.argv.slice(2)) {
  const source = fs.readFileSync(filename, "utf8");
  fs.writeFileSync(filename, source.replace(/[ \t]+(?=\r?\n|$)/g, ""));
}
NODE

echo ""
echo "Done. Outputs in dist/*/"
