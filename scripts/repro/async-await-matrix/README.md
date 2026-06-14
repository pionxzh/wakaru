# Async/Await Matrix

This matrix generates async function, async arrow, async IIFE, double-await,
try/catch/finally, loop control flow (with and without internal continue),
destructuring/default, object-rest, nested async callback, and generator
delegation snippets through Babel, TypeScript, SWC, and esbuild, then runs
wakaru over each generated shape.

## Comparison model

Non-mangled shapes are checked with substring needles (`expected` /
`expectedAny`). Mangled shapes can't be — every local binding is renamed — so
they are compared **structurally**: `wakaru debug normalize --rename` alpha-
renames all local bindings to canonical names and reprints, so an alpha-
equivalent recovery normalizes to byte-identical source. A mangled shape passes
when its normalized output equals the normalized `source` or any entry in the
snippet's `acceptForms` (genuinely distinct idiomatic recoveries, e.g. a C-style
loop recovered as `for…of`). See `../lib/compare.mjs`.

This replaced an earlier regex name-stripping normalizer that was lossy and
**false-passed** unrecovered output: Terser-compressed regenerator state
machines and Babel lazy-init helper artifacts were reported as recovered. Those
now correctly show as `no`. Remaining `no` rows fall into three honest buckets:

- **state-machine** — wakaru leaves a Terser-compressed regenerator runtime intact.
- **degraded** — a helper artifact leaks (`__rest` inlined, `const x = undefined`,
  `push.apply(...)` not recovered).

The hoisted `let x; … x = await …` split is folded back to `let x = await …` by
the `MergeDeclarationInit` rule, so those shapes recover; the `acceptForms`
above capture the resulting `let`-form (it stays `let`, not `const`, because the
merge runs after const-promotion).

Each Babel/TypeScript/SWC/esbuild output is checked in three Terser variants:

- raw compiler output
- Terser compression without name mangling
- Terser compression with name mangling

The matrix also includes standalone source-through-Terser rows for both Terser
variants, because some recoverable shapes only appear after compiler or source
output is minified.

The `class-async-method` snippet also includes a dedicated Babel preset-env IE11
profile. Its Terser compression+mangle variant reproduces Babel's lazy async
class-method trampoline after minification, where the method descriptor value
becomes a comma-sequence assignment plus `.apply(this, arguments)` wrapper.

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
# Full matrix
node scripts/repro/async-await-matrix/matrix.mjs --level standard

# Full matrix with failure details (lowered + recovered code blocks)
node scripts/repro/async-await-matrix/matrix.mjs --level standard --details

# Single snippet (exact or substring match)
node scripts/repro/async-await-matrix/matrix.mjs --level standard --snippet async-iife

# Dump full lowered input + wakaru output for one snippet × tool
node scripts/repro/async-await-matrix/matrix.mjs --level standard --dump async-simple-loop tsc-es5-terser-compress
```

The script installs transformer and minifier packages under
`target/repro-tools/`, so those downloads are cached outside the source tree.
