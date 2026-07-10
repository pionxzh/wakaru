# Vue docs examples corpus

This manual harness assembles the official `vuejs/docs` examples into the same
Composition API SFC shape used by the Vue docs playground, compiles them with
the repository's Vue compiler version, and runs Wakaru's `--vue-sfc` recovery
against every generated component.

The checkout lives under `target/vue-docs/` and outputs/reports stay under
`target/vue-docs-examples/`; third-party source and generated artifacts are not
committed.

```bash
node scripts/repro/vue-docs-examples/run.mjs
node scripts/repro/vue-docs-examples/run.mjs --filter grid
node --test scripts/repro/vue-docs-examples/run.test.mjs
```

Use `--docs <path>` for an existing checkout and `--no-build-wakaru` with
`WAKARU=/path/to/wakaru` to reuse a built binary. If the default checkout is
missing, the runner clones `git@github.com:vuejs/docs.git` over SSH.

The report checks SFC parsing/template compilation, restoration of
`<script setup>`, preserved import specifiers, leaked compiler-only setup markers,
and normalized template compiler equivalence. Styles are deliberately excluded
from recovery scoring because normal generated JavaScript does not contain the
original CSS.
