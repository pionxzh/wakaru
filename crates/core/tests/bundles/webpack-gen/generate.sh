#!/usr/bin/env bash
# Generates webpack test fixtures from the source files in src/.
# Requires: Node.js + npm (uses npx to fetch webpack on-the-fly).
#
# Usage:
#   cd tests/bundles/webpack-gen
#   bash generate.sh
#
# Each config produces a bundle in dist/<name>/bundle.js.
# The generated outputs are checked into the repo so tests don't require Node.js.

set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist

echo "=== Webpack 4 (4.47.0) ==="

echo "  wp4-cjs:           CJS-only modules (dev, string IDs, object map)"
npx --yes -p webpack@4 -p webpack-cli@3 webpack --config webpack4-cjs.config.cjs 2>/dev/null

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

echo "  wp5-prod:          Production (fully flattened, nothing to unpack)"
npx --yes webpack-cli@5 --config webpack5-prod.config.cjs 2>/dev/null

echo "  wp5-cjs-min:       CJS-only modules (production, minified)"
npx --yes webpack-cli@5 --config webpack5-cjs-min.config.cjs 2>/dev/null

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
echo "Done. Outputs in dist/*/"
