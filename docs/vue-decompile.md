# Vue Decompile

Wakaru's Vue support is a no-sourcemap, best-effort recovery path for generated
Vue 3 render modules. It does not parse original `.vue` single-file components;
instead it decompiles JavaScript first, recognizes Vue runtime helper calls, and
prints a reconstructed SFC-like artifact with Wakaru's own template printer.

Run:

```bash
cargo run -p wakaru-cli -- input.js --vue-sfc
cargo run -p wakaru-cli -- --unpack bundle.js --vue-sfc -o unpacked/
```

In single-file mode without `-o`, `--vue-sfc` prints a `.vue`-like artifact
when the render module matches supported Vue helper shapes, and falls back to
normal JavaScript otherwise.

With `-o`, only `.vue` output paths are Vue-only. `-o App.vue` writes the
recovered SFC and returns an error if the input cannot be recovered as Vue.
All other output paths are JavaScript-primary: Wakaru writes decompiled
JavaScript to the requested path, and when Vue recovery succeeds it also writes
a sibling `.vue` sidecar. The sidecar name comes from the input filename with
the final suffix replaced by `.vue`; for example,
`custom/target.min.mjs --vue-sfc -o out/renamed.mjs` writes
`out/renamed.mjs` and `out/target.min.vue`. Source maps, when requested, are
emitted only for the JavaScript artifact.

In unpack mode, `--vue-sfc` is additive: every module still gets a JavaScript
artifact, and recoverable Vue render modules also get a sibling `.vue`
artifact. The JavaScript artifact for a recoverable Vue module is named with a
`.vue.js` suffix so the recovered SFC can use the original `.vue` filename.
Modules that look like Vue render modules but cannot be recovered stay as
JavaScript fallback artifacts.

`--vue-sfc` intentionally does not work with `--raw`, because raw unpack output
skips the normal decompile pipeline that normalizes imports, aliases, and
render calls before Vue recovery runs.

## Current Recovery Scope

Supported shapes:

- Vue 3 imports from `"vue"` after the normal decompile pipeline.
- `export function render(...)` modules.
- Root `createElementBlock(...)` / `createElementVNode(...)` calls, including
  transparent `Fragment` children.
- Hoisted object props such as `_hoisted_1 = { class: "card" }`.
- Static string children.
- `toDisplayString(...)` text interpolation.
- Basic static attributes, dynamic `:class` / `:style`, and `@event`
  attributes, including cached event handlers and `withModifiers(...)` /
  `withKeys(...)` event modifiers.
- `v-if` / `v-else-if` / `v-else` from Vue conditional render branches.
- `v-for` from `renderList(...)` fragment children.
- Component vnodes from `resolveComponent(...)` + `createVNode(...)` /
  `createBlock(...)`, including default and named component `v-model` pairs
  from `prop` + `onUpdate:prop`, plus component model modifier props such as
  `modelModifiers`.
- Dynamic component vnodes from `resolveDynamicComponent(...)` as
  `<component :is="...">`.
- Named slots with fallback text from `renderSlot(...)`.
- Runtime directives from `withDirectives(...)`, including `v-model` text
  inputs, `v-show`, directive arguments, modifiers, and custom
  `resolveDirective(...)` bindings.
- Simple component option objects as `<script>export default ...</script>`.

Known gaps:

- Import reconstruction for recovered component dependencies is heuristic. It
  handles common relative component imports, but unresolved or heavily rewritten
  dependencies are left as local bindings instead of fabricated imports.
- Dynamic component model arguments.
- Multiple roots and advanced slot scopes beyond the covered fallback case.
- Strong source-name recovery after aggressive minification/mangling when the
  original public names are not present in the generated render code.

Use `scripts/repro/vue-render-matrix/` to reproduce Vue compiler output and
track which generated shapes the current recovery supports. Use
`scripts/repro/vue-public-corpus/` for manual runs against pinned public Vue
builds before distilling misses into neutral regression tests.
