# Async/Await Matrix

This matrix generates async function, async arrow, async IIFE, double-await,
try/catch/finally, loop control flow (with and without internal continue),
destructuring/default, object-rest, nested async callback, and generator
delegation snippets through Babel, TypeScript, SWC, and esbuild, then runs
wakaru over each generated shape.

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
