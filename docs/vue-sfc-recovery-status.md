# Vue SFC Recovery Status

`--vue-sfc` is experimental. It is intended to be useful as an additional
recovered artifact for Vue 3 render modules, not a guarantee that Wakaru can
recreate the original `.vue` source.

The recovery path does not rely on source maps. It decompiles JavaScript first,
recognizes Vue runtime render shapes, builds Wakaru's own Vue IR, and prints a
best-effort SFC with Wakaru's template/script emitters.

## Experimental Posture

Keep the feature behind `--vue-sfc` as experimental while these constraints
hold:

- The default JavaScript decompile path remains the primary output.
- In unpack mode, Vue SFC output is additive: recoverable modules get sibling
  `.vue` files while JavaScript artifacts are still emitted.
- Non-recoverable Vue-like modules fall back to JavaScript instead of producing
  invalid SFCs.
- Recovered SFCs are validated with `@vue/compiler-sfc` in the public corpus
  harness.
- Real misses should be reduced into neutral synthetic regression tests before
  being committed.

Do not document it as source reconstruction or round-trip Vue decompilation.
The correct expectation is "best-effort Vue 3 SFC-shaped recovery from generated
render JavaScript."

## What Works Reasonably Well

- Vue 3 render helper imports and common Vite-style helper aliasing.
- Standalone render functions and setup-returned render functions.
- Script setup reconstruction for props, emits, refs, computed values, setup
  locals, imported components, common composables, and compiler-generated
  `setup(...)` objects paired with a separate render function.
- Authored top-level script-setup effects such as `watch(...)`, lifecycle hooks,
  and their imports, while compiler-only expose/return markers are omitted.
- Template recovery for common element/component vnodes, text interpolation,
  static and dynamic attrs, events, class/style bindings, `v-if`, `v-for`,
  slots, dynamic components, and common runtime directives.
- Additive CLI behavior for batch/unpack use cases.
- Public-corpus smoke runs for small Vite projects and opt-in larger projects.
- The official Vue docs Composition API examples: all 27 component fixtures at
  docs commit `e4641141026871271e5083c99ad4cd3f4a8e9a68` recover to parseable,
  template-compilable SFCs with required import specifiers and no leaked
  script-setup markers. The stricter token-aware generated-template comparison
  is 12/27; it ignores generated-code formatting and hoist numbering while
  preserving string-literal contents. It still treats harmless loop-variable
  renaming and equivalent template syntax as different, so it is a conservative
  fidelity signal rather than a semantic pass rate.

## Known Gaps

- Script setup can still be bundle-shaped when dependency evidence is weak.
  Wakaru may preserve helper declarations, local aliases, or computed blocks
  instead of inventing cleaner source.
- Import and dependency reconstruction is heuristic. Relative component imports,
  composables, providers, and injected setup dependencies are improved, but
  unresolved aliases or heavily rewritten modules can still leave missing or
  overly raw declarations.
- Component selection and splitting are heuristic for scope-hoisted modules that
  contain multiple candidate Vue components.
- Webpack/vue-loader coverage is less mature than Vite coverage. The fallback
  behavior is intentional, but more public webpack cases are needed before
  calling this broad bundler support.
- Template fidelity is incomplete for advanced dynamic components, complex
  directive payloads, deep slot-scope patterns, and unusual generated children.
- Name recovery depends on names that survive into generated JavaScript. Heavy
  minification can force generic or bundle-shaped output.
- Styles, custom SFC blocks, comments, formatting, and author-level source
  organization are not recovered.
- There is no source-map-backed mapping from recovered SFC sections to original
  source locations.

## Public Corpus Workflow

Use `scripts/repro/vue-docs-examples/` for a fast, focused check of the official
Vue examples:

```powershell
node scripts/repro/vue-docs-examples/run.mjs
node scripts/repro/vue-docs-examples/run.mjs --filter grid
node scripts/repro/vue-docs-examples/run.mjs --profile prod-inline
node --test scripts/repro/vue-docs-examples/run.test.mjs
```

The runner clones `vuejs/docs` over SSH into `target/vue-docs/` when needed,
assembles the same Composition API source SFCs as the docs playground, compiles
them with the docs repository's Vue version, and writes its report under
`target/vue-docs-examples/`. It checks the production inline-template default
used by Vite and vue-loader, the production external-render fallback, and the
development external-render shape independently. The smaller Vue render matrix
also applies Terser compression and identifier mangling to all three profiles.

Use `scripts/repro/vue-public-corpus/` for confidence checks and gap discovery:

```powershell
node scripts/repro/vue-public-corpus/run.mjs --list
node scripts/repro/vue-public-corpus/run.mjs
node scripts/repro/vue-public-corpus/run.mjs --case vitepress-docs
node scripts/repro/vue-public-corpus/run.mjs --all
```

The runner clones pinned public repositories into `target/vue-public-corpus/`,
builds them, runs `wakaru --unpack --vue-sfc --json`, writes recovered outputs
under `target/vue-public-corpus/outputs/`, and reports recovery/fallback counts,
unsupported markers, SFC parse results, and template compile results.
The default smoke set includes pinned Vite JavaScript/TypeScript starters and a
webpack 5 + vue-loader production build; larger application corpora remain
opt-in.

The corpus is intentionally not committed. When it reveals a bug, inspect the
generated output locally, identify the smallest structural gap, and add a
neutral synthetic unit test that does not contain third-party project code.

## Follow-Up Targets

- Add more public webpack/vue-loader cases and reduce the first concrete misses.
- Improve import/dependency recovery where there is strong local evidence, while
  continuing to avoid fabricating setup declarations from weak `.value` reads.
- Keep simplifying script setup output when the dependency graph is provable.
- Expand template IR coverage for currently unsupported directives, slots, and
  dynamic component/model shapes.
- Add targeted tests for each recovered public-corpus gap before changing the
  emitter or selection heuristics.
- Periodically run the public corpus before release notes or before promoting
  the feature out of experimental status.

## Keeping This Document Current

Update this file whenever the CLI output contract, recovery scope, public corpus
harness, or experimental status changes. Keep `README.md`, `docs/cli.md`,
`docs/vue-decompile.md`, and `SKILL.md` aligned with the same user-facing
expectations.
