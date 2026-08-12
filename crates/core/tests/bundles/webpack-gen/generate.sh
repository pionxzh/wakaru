#!/usr/bin/env bash
# Generates webpack test fixtures from the source files in src/.
# Requires: Node.js + npm (installs webpack 4 locally for deterministic
# module ids; fetches webpack 5 and ncc via npx on-the-fly).
#
# Usage:
#   cd tests/bundles/webpack-gen
#   bash generate.sh
#
# Each config produces a checked-in bundle under dist/<name>/.
# The generated outputs are checked into the repo so tests don't require Node.js.

set -euo pipefail
cd "$(dirname "$0")"

# dist/ also carries hand-authored fixtures no generator here produces
# (wp-path-traversal's traversal ids, wp5-require-s's minified require.s
# shape). Remove only the outputs this script regenerates: every
# webpackN-<x>.config.cjs writes to dist/wpN-<x>, plus the two ncc builds.
for config in webpack4-*.config.cjs webpack5-*.config.cjs; do
  name="${config%.config.cjs}"
  rm -rf "dist/wp${name#webpack}"
done
rm -rf dist/wp5-ncc dist/wp5-ncc-min node_modules

echo "=== Webpack 4 (4.47.0) ==="

# Webpack 4's NodeSourcePlugin embeds `buildin/global.js` as a module whose id
# is the physical webpack package location relative to the compilation
# context. Running webpack 4 straight from the npx cache leaks the generator
# machine's cache path into the checked-in bundle (and varies per machine).
# Install it locally first so the id is always
# "./node_modules/webpack/buildin/global.js".
npm install --no-save --no-package-lock --no-audit --no-fund \
  webpack@4.47.0 webpack-cli@3.3.12 >/dev/null

webpack4() {
  ./node_modules/.bin/webpack "$@"
}

echo "  wp4-cjs:           CJS-only modules (dev, string IDs, object map)"
webpack4 --config webpack4-cjs.config.cjs 2>/dev/null

echo "  wp4-umd:           CJS-only modules wrapped as a UMD library"
webpack4 --config webpack4-umd.config.cjs 2>/dev/null

echo "  wp4-amd:           CJS-only modules wrapped as an AMD library"
webpack4 --config webpack4-amd.config.cjs 2>/dev/null

echo "  wp4-esm:           ESM modules (require.r + require.d 3-arg form)"
webpack4 --config webpack4-esm.config.cjs 2>/dev/null

echo "  wp4-mixed:         ESM entry importing CJS module via require()"
webpack4 --config webpack4-mixed.config.cjs 2>/dev/null

echo "  wp4-require-n:     ESM entry importing CJS via import (triggers require.n + .a)"
webpack4 --config webpack4-require-n.config.cjs 2>/dev/null

echo "  wp4-prod:          Production (numeric IDs, array, module concatenation, no minify)"
webpack4 --config webpack4-prod.config.cjs 2>/dev/null

echo "  wp4-cjs-min:       CJS-only modules (production, minified)"
webpack4 --config webpack4-cjs-min.config.cjs 2>/dev/null

echo "  wp4-esm-min:       ESM modules (production, minified, concatenated)"
webpack4 --config webpack4-esm-min.config.cjs 2>/dev/null

echo "  wp4-dynamic:       Dynamic import (JSONP chunk: window.webpackJsonp)"
webpack4 --config webpack4-dynamic.config.cjs 2>/dev/null

echo "  wp4-dynamic-min:   Dynamic import (production, minified JSONP chunk)"
webpack4 --config webpack4-dynamic-min.config.cjs 2>/dev/null

echo "  wp4-var-inject:    Var injection (.call(this, require(global.js)))"
webpack4 --config webpack4-var-inject.config.cjs 2>/dev/null

echo "  wp4-inner-umd-min: Inner CommonJS modules with UMD export branches"
webpack4 --config webpack4-inner-umd-min.config.cjs 2>/dev/null

# Drop the local webpack 4 install so the npx-invoked webpack 5 builds below
# cannot resolve it (webpack-cli prefers a local install via import-local).
rm -rf node_modules

echo ""
echo "=== Webpack 5 ==="

# Keep each fixture family on the producer version that reproduces its
# checked-in artifacts. The array/runtime-mutation fixtures were added later.
webpack5_5_106() {
  npx --yes -p webpack@5.106.2 -p webpack-cli@5.1.4 webpack "$@"
}

webpack5_5_109() {
  npx --yes -p webpack@5.109.0 -p webpack-cli@5.1.4 webpack "$@"
}

echo "  wp5-cjs:           CJS-only modules (dev, string IDs; 5.106.2)"
webpack5_5_106 --config webpack5-cjs.config.cjs 2>/dev/null

echo "  wp5-esm:           ESM modules (require.r + require.d object form)"
webpack5_5_106 --config webpack5-esm.config.cjs 2>/dev/null

echo "  wp5-mixed:         ESM entry importing CJS module via require()"
webpack5_5_106 --config webpack5-mixed.config.cjs 2>/dev/null

echo "  wp5-umd:           CJS-only modules wrapped as a UMD library"
webpack5_5_106 --config webpack5-umd.config.cjs 2>/dev/null

echo "  wp5-umd-esm:       ESM modules wrapped as a UMD library"
webpack5_5_106 --config webpack5-umd-esm.config.cjs 2>/dev/null

echo "  wp5-amd:           CJS-only modules wrapped as an AMD library"
webpack5_5_106 --config webpack5-amd.config.cjs 2>/dev/null

echo "  wp5-prod:          Production (fully flattened, nothing to unpack)"
webpack5_5_106 --config webpack5-prod.config.cjs 2>/dev/null

echo "  wp5-cjs-min:       CJS-only modules (production, minified)"
webpack5_5_106 --config webpack5-cjs-min.config.cjs 2>/dev/null

echo "  wp5-umd-min:       CJS-only modules wrapped as a minified UMD library"
webpack5_5_106 --config webpack5-umd-min.config.cjs 2>/dev/null

echo "  wp5-esm-min:       ESM modules (production, fully flattened + minified)"
webpack5_5_106 --config webpack5-esm-min.config.cjs 2>/dev/null

echo "  wp5-dynamic:       Dynamic import (async chunk via require())"
webpack5_5_106 --config webpack5-dynamic.config.cjs 2>/dev/null

echo "  wp5-dynamic-min:   Dynamic import (production, minified async chunk)"
webpack5_5_106 --config webpack5-dynamic-min.config.cjs 2>/dev/null

echo "  wp5-var-inject:    Global access (uses __webpack_require__.g, no var injection)"
webpack5_5_106 --config webpack5-var-inject.config.cjs 2>/dev/null

echo "  wp5-require-o:     Split initial chunk startup via __webpack_require__.O"
webpack5_5_106 --config webpack5-require-o.config.cjs 2>/dev/null

echo "  wp5-array:         Dense natural ids (holey array table + Array(n).concat chunk; 5.109.0)"
webpack5_5_109 --config webpack5-array.config.cjs 2>/dev/null

echo "  wp5-require-mutation-min: Entry mutates the raw require binding around startup (5.109.0)"
webpack5_5_109 --config webpack5-require-mutation-min.config.cjs 2>/dev/null

echo "  wp5-inner-umd-min: Inner CommonJS modules with UMD export branches (5.109.0)"
webpack5_5_109 --config webpack5-inner-umd-min.config.cjs 2>/dev/null

echo ""
echo "=== Vercel ncc (0.44.1) ==="

echo "  wp5-ncc:           Node CJS bundle with inline webpack startup"
npx --yes @vercel/ncc@0.44.1 build src/ncc-entry.cjs -o dist/wp5-ncc 2>/dev/null

echo "  wp5-ncc-min:       Minified Node CJS bundle with inline webpack startup"
npx --yes @vercel/ncc@0.44.1 build src/ncc-entry.cjs -m -o dist/wp5-ncc-min 2>/dev/null

# Webpack and ncc development templates emit tab-only padding after runtime
# comment markers. Strip it without changing executable content so checked-in
# generated fixtures pass Git's whitespace checks.
node - \
  dist/wp5-array/bundle.js \
  dist/wp5-array/chunk-1.js \
  dist/wp5-ncc/index.cjs \
  dist/wp5-ncc-min/index.cjs <<'NODE'
const fs = require("node:fs");

for (const filename of process.argv.slice(2)) {
  const source = fs.readFileSync(filename, "utf8");
  fs.writeFileSync(filename, source.replace(/[ \t]+(?=\r?\n|$)/g, ""));
}
NODE

echo ""
echo "Done. Outputs in dist/*/"
