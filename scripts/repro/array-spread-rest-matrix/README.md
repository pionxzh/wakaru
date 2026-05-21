# Array Spread/Rest Matrix

This matrix generates array spread, argument spread, rest parameter, and array
destructuring rest snippets through Babel, TypeScript, and SWC, then runs
wakaru over each generated shape. esbuild is omitted here because it does not
lower these ES2015 constructs to ES5.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

Run from the repo root:

```bash
node scripts/repro/array-spread-rest-matrix/matrix.mjs --level standard
node scripts/repro/array-spread-rest-matrix/matrix.mjs --level aggressive --details
```

The script installs transformer packages under `target/repro-tools/`, so those
downloads are cached outside the source tree.
