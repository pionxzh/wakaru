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
shapes Wakaru must recover.

By default the script uses `target/debug/wakaru(.exe)` when present, otherwise
it falls back to `cargo run -q -p wakaru-cli --`. Set `WAKARU` to test a
specific binary.

The Vue compiler package is installed under `target/repro-tools/`, so the first
run may download `@vue/compiler-sfc` and Terser packages. The `target/`
directory is ignored by git.
