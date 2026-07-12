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

The harness-managed default checkout is pinned to docs commit
`e4641141026871271e5083c99ad4cd3f4a8e9a68`. If it is missing, the runner
clones `git@github.com:vuejs/docs.git` over SSH and checks out that commit; if
it has moved, the runner updates only a clean checkout. Use `--docs <path>` for
an intentional custom checkout, which the runner never changes, and
`--no-build-wakaru` with `WAKARU=/path/to/wakaru` to reuse a built binary.

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
and token-normalized template compiler equivalence. The token comparison ignores
generated-code formatting, comments, and hoist numbering while preserving
string-literal contents. Styles are deliberately excluded from recovery scoring
because normal generated JavaScript does not contain the original CSS.
