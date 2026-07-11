# Vue docs examples corpus

This manual harness assembles the official `vuejs/docs` examples into the same
Composition API source SFCs used by the Vue docs playground and compiles them
with the repository's Vue compiler version. Every component runs through three
profiles: the production inline-template default used by Vite and vue-loader,
the production external-render fallback, and the development external-render
shape. Wakaru's `--vue-sfc` recovery is checked independently for each profile.

The checkout lives under `target/vue-docs/` and outputs/reports stay under
`target/vue-docs-examples/`; third-party source and generated artifacts are not
committed.

```bash
node scripts/repro/vue-docs-examples/run.mjs
node scripts/repro/vue-docs-examples/run.mjs --filter grid
node scripts/repro/vue-docs-examples/run.mjs --profile prod-inline
node --test scripts/repro/vue-docs-examples/run.test.mjs
```

Use `--docs <path>` for an existing checkout and `--no-build-wakaru` with
`WAKARU=/path/to/wakaru` to reuse a built binary. If the default checkout is
missing, the runner clones `git@github.com:vuejs/docs.git` over SSH.

The available profiles are:

- `prod-inline`: normal production `<script setup>` output for inlineable
  templates in current Vite and vue-loader builds.
- `prod-external`: production external-render topology used after a toolchain
  separately resolves or preprocesses a template. This harness forces that
  topology on its already-resolved plain HTML fixtures; it does not run template
  preprocessors.
- `dev-external`: development/HMR output with a separately compiled template.

The report checks SFC parsing/template compilation, restoration of
`<script setup>`, preserved import specifiers, leaked compiler-only setup markers,
and normalized template compiler equivalence. Styles are deliberately excluded
from recovery scoring because normal generated JavaScript does not contain the
original CSS.
