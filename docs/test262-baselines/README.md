# Test262 Baseline Layout

These files are deterministic Markdown summaries from
`scripts/correctness/test262-roundtrip.mjs`.

The baseline path encodes two independent choices:

- **Slice**: which Test262 paths are run. Examples: `default`, `classes`,
  `destructuring`, `modules`.
- **Producer pipeline**: how the Test262 source is transformed before Wakaru
  decompiles it. Examples: `terser-light`, `swc-minify`, `esbuild-minify`.

Normal baseline summaries live under producer pipeline directories:

```text
terser-light/default.md
swc-minify/default.md
esbuild-minify/default.md
```

Each normal producer runs the same slice set:

```text
default
classes
destructuring
async-generators
scope
control-flow
calls
operators
templates
literals
block-scope-syntax
variables
assignment-target-type
arguments-object
modules
```

`default.md` means the default Test262 slice, not raw/no-transform input. The
`terser-light` producer uses Terser as a parser/printer with no compression or
mangling. Use `--pipeline none` or `--transform none` when a no-producer
baseline is needed.

`module-graph/` is different: it runs the module-code slice with recursive
module dependency loading. Its file names are producer pipelines:

```text
module-graph/none.md
module-graph/swc-minify.md
module-graph/esbuild-minify.md
```

Regenerate the normal baseline matrix with:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs
```

Use `--producer` or `--slice` to refresh a subset.
Use `--missing` to skip summaries that already exist and have `complete: true`.
The matrix runner builds `wakaru-cli` once before running jobs unless `WAKARU`
is already set.
