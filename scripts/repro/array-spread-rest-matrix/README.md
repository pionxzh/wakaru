# Array Spread/Rest Matrix

This matrix generates array spread, argument spread, rest parameter, and array
destructuring rest snippets through Babel, TypeScript, SWC, and esbuild, then
runs wakaru over each generated shape. esbuild is checked at ES2015 because it
does not lower these constructs to ES5.

The matrix also includes standalone Terser rows and Babel/TypeScript/SWC/esbuild
output minified through Terser, because some recoverable shapes only appear
after compiler output is minified.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

Run from the repo root:

```bash
node scripts/repro/array-spread-rest-matrix/matrix.mjs --level standard
node scripts/repro/array-spread-rest-matrix/matrix.mjs --level aggressive --details
```

The script installs transformer and minifier packages under
`target/repro-tools/`, so those downloads are cached outside the source tree.
