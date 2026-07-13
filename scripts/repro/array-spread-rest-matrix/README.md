# Array Spread/Rest Matrix

This matrix generates array spread, argument spread, rest parameter, and array
destructuring rest snippets through Babel, TypeScript, SWC, and esbuild, then
runs wakaru over each generated shape. esbuild is checked at ES2015 because it
does not lower these constructs to ES5.

The matrix also includes standalone Terser rows and Babel/TypeScript/SWC/esbuild
output minified through Terser, because some recoverable shapes only appear
after compiler output is minified.

TypeScript ES5 direct index/slice array-rest rows are marked `gated` below
`aggressive`, because recovering `items[0]` plus `items.slice(n)` as array
destructuring changes semantics for non-array or custom-slice values.

The `array-rest-basic`, `array-rest-default-hole`, and
`array-rest-nested-pattern` snippets deliberately extend that weak boundary.
At `standard`, direct index/`slice()` forms remain unrecovered unless a helper
proves the required array/iterator semantics. Nested rows may recover the inner
rest while leaving the outer index/`slice()` accesses split, and helper-heavy
`toArray` variants may remain lowered. These intentional challenge rows expand
the denominator and explain the matrix's lower aggregate rate; they are not
regressions in previously passing shapes.

`array-rest-basic` also accepts Babel's retained single-read `_items = items`
capture before the recovered destructuring. `items` is environment-injected in
the harness, so SmartInline intentionally cannot prove it frozen; preserving
that capture is faithful recovery rather than a failed array-rest rewrite.

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
