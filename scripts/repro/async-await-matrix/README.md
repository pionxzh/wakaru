# Async/Await Matrix

This matrix generates async function, async arrow, try/catch, and generator
snippets through Babel, TypeScript, SWC, and esbuild, then runs wakaru over each
generated shape.

The matrix also includes standalone Terser rows and Babel/TypeScript/SWC/esbuild
output minified through Terser, because some recoverable shapes only appear
after compiler output is minified.

Babel is run in two modes:

- `async-generator`: `@babel/plugin-transform-async-to-generator` only, leaving
  native generator syntax inside `_asyncToGenerator(...)`.
- `regenerator`: async-to-generator plus `@babel/plugin-transform-regenerator`,
  producing `regeneratorRuntime.wrap(...)` state-machine output.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

Run from the repo root:

```bash
node scripts/repro/async-await-matrix/matrix.mjs --level standard
node scripts/repro/async-await-matrix/matrix.mjs --level standard --details
```

The script installs transformer and minifier packages under
`target/repro-tools/`, so those downloads are cached outside the source tree.
