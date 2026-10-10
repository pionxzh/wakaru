# Vue Render Reproduction Matrix

This harness checks how Vue 3 single-file components compile into render
functions, then runs `wakaru --vue-sfc` on the generated JavaScript without
using source maps. It is for investigation and regression hunting, not as a
committed snapshot source.

The target recovery path is intentionally no-sourcemap:

1. Parse/decompile the generated JavaScript module.
2. Recognize Vue compiler/runtime helper calls such as `openBlock`,
   `createElementBlock`, `createElementVNode`, `toDisplayString`,
   `resolveComponent`, and `withDirectives`.
3. Eventually emit a best-effort `.vue`-like artifact through a custom
   template/SFC printer.

Run:

```powershell
node scripts/repro/vue-render-matrix/matrix.mjs
```

Add `--details` to print full generated and recovered code for missed cases.
Add `--level minimal`, `--level standard`, or `--level aggressive` to run
wakaru with a specific rewrite level.

Rows are grouped by distinct generated output per snippet. Vue compiler output
is tested as production inline-template (the Vite/vue-loader default),
production external-render fallback, and development external-render output.
Each profile also runs through Terser compression and compression+mangling
because patch flags, comments, hoists, and renamed bindings all affect the
shapes Wakaru must recover. Two Babel preset-env passes (IE 11 targets) add
the ES5-lowered form of every profile, where render closures, slots, and
handlers are `function` expressions instead of arrows: one keeps ES module
syntax, the other uses Babel's own SystemJS module transform.

Without `-o`, `--vue-sfc` prints decompiled JavaScript, so the harness writes
each run to a temporary `.vue` output path and compares that file. A run where
recovery fails exits non-zero and shows up as `wakaru-failed`.

By default the script asks Cargo to refresh `target/debug/wakaru(.exe)` once,
then uses that binary for the matrix. Set `WAKARU` to test a specific binary.

The Vue compiler package is installed in the shared repro tool cache
(`docs/testing.md`), so the first run may download `@vue/compiler-sfc` and
Terser packages. The `target/` directory is ignored by git.

## Known gaps

- **Babel's SystemJS transform** (producer @babel/preset-env@7.29.7 targets
  ie 11, modules systemjs). The transform declares the component object,
  hoisted static props, and Vue helper imports as `var`s at the top of the
  `System.register` callback and assigns them later: the component in
  `execute()` (`__sfc__ = {...}; _export("default", __sfc__)`), helpers in
  the setter (`_openBlock = _vue.openBlock`). Vue recovery handles the Rollup
  `system` shape, which passes the component straight to the export call,
  and does not follow these later assignments.
  - Render closure returned from `setup` (inline-template profile with a
    `<script setup>`): no SFC is recovered.
  - Top-level `function render` (external-render profiles, and components
    without a script): some snippets recover; others keep the hoisted
    variable (`v-bind="_hoisted_1"`) or lose `v-if` branches, the
    event-handler binding, or the `v-model` link.
  - Terser variants of every profile: no SFC is recovered.
