# Test262 Baseline Layout

These files are deterministic Markdown summaries from
`scripts/correctness/test262-roundtrip.mjs`.

The baseline path encodes two independent choices:

- **Slice**: which Test262 paths are run. Examples: `default`, `classes`,
  `destructuring`, `modules`.
- **Producer pipeline**: how the Test262 source is transformed before Wakaru
  decompiles it. Examples: `terser-light`, `swc-minify`, `esbuild-minify`.

Root-level files such as `default.md` and `classes.md` use the runner defaults:

```text
producer pipeline: terser-light
rewrite level: minimal
```

`default.md` means the default Test262 slice, not raw/no-transform input.
Use `--pipeline none` or `--transform none` when a no-producer baseline is
needed.

Producer-specific folders group the same slices under a non-default producer:

```text
swc-minify/default.md
esbuild-minify/classes.md
```

`module-graph/` is different: it runs the module-code slice with recursive
module dependency loading. Its file names are producer pipelines:

```text
module-graph/none.md
module-graph/swc-minify.md
module-graph/esbuild-minify.md
```
