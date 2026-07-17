#!/usr/bin/env bash
# Generates a Cocos Creator 2.x-style project-script bundle from the readable
# CommonJS sources in src/. Cocos Creator 2.4 used Browserify 13, whose
# browser-pack dependency accepts the same module-table rows and custom prelude.
# The generated outputs are checked in so tests do not require Node.js. The
# pinned Uglify pass reproduces Cocos production compression of factory bodies.

set -euo pipefail
cd "$(dirname "$0")"

npm ci --ignore-scripts --no-audit --no-fund
node generate.cjs
./node_modules/.bin/uglifyjs dist/project.js \
  --compress \
  --mangle \
  --output dist/project.min.js
