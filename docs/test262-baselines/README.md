# Test262 Baseline Layout

Each baseline has a canonical deterministic JSON file and a generated Markdown
summary from `scripts/correctness/test262-roundtrip.mjs`. JSON is the enforced
per-case contract; Markdown is the human review surface.

The baseline path encodes two independent choices:

- **Slice**: which Test262 paths are run. Examples: `default`, `classes`,
  `destructuring`, `modules`.
- **Producer pipeline**: how the Test262 source is transformed before Wakaru
  decompiles it. Examples: `terser-light`, `swc-minify`, `esbuild-minify`.

Normal baseline summaries live under producer pipeline directories:

```text
terser-light/default.md
terser-light/default.json
swc-minify/default.md
swc-minify/default.json
esbuild-minify/default.md
esbuild-minify/default.json
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
identifiers
function-code
asi
keywords
reserved-words
modules
```

The `async-generators` slice also includes standalone `await` expression tests
and `for-await-of` statement tests.

`default.md` means the default Test262 slice, not raw/no-transform input. The
`terser-light` producer uses Terser as a parser/printer with no compression or
mangling. Use `--pipeline none` or `--transform none` when a no-producer
baseline is needed.

`module-graph/` runs the same recursive module-code lane with additional
producer variants that are not part of the normal 20-slice producer matrix.
Its file names are producer pipelines, and each has both canonical JSON and a
Markdown review summary:

```text
module-graph/none.md
module-graph/none.json
module-graph/babel-env-terser.md
module-graph/babel-env-terser.json
module-graph/swc-minify.md
module-graph/swc-minify.json
module-graph/esbuild-minify.md
module-graph/esbuild-minify.json
```

Regenerate the normal baseline matrix with:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs
```

Use `--producer` or `--slice` to refresh a subset. Select only the additional
module producer variants with `--slice module-graph`.
Baseline replacement also requires `--update`; ordinary runs compare without
rewriting reviewed outcomes.
Use `--missing` to skip summaries that already exist and have `complete: true`.
The matrix runner builds `wakaru-cli` once before running jobs unless `WAKARU`
is already set.
