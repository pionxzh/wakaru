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

In single-file mode, `--vue-sfc` prints a `.vue`-like artifact when the render
module matches supported Vue helper shapes, and falls back to normal JavaScript
otherwise. In unpack mode, recoverable modules are written with a `.vue`
extension; modules that do not match stay as JavaScript.

`--vue-sfc` intentionally does not work with `--raw`, because raw unpack output
skips the normal decompile pipeline that normalizes imports, aliases, and
render calls before Vue recovery runs.

## Current Recovery Scope

Supported initial shapes:

- Vue 3 imports from `"vue"` after the normal decompile pipeline.
- `export function render(...)` modules.
- Root `createElementBlock(...)` / `createElementVNode(...)` calls.
- Hoisted object props such as `_hoisted_1 = { class: "card" }`.
- Static string children.
- `toDisplayString(...)` text interpolation.
- Basic static attributes, dynamic `:class` / `:style`, and `@event`
  attributes.
- Simple component option objects as `<script>export default ...</script>`.

Known gaps:

- Control flow recovery (`v-if`, `v-else-if`, `v-else`).
- List recovery (`v-for` from `renderList` / `Fragment`).
- Component vnode and slot recovery (`createVNode`, `resolveComponent`,
  `renderSlot`).
- Directive recovery (`withDirectives`, `vModelText`, `vShow`, and related
  helpers).
- Strong source-name recovery after aggressive minification/mangling when the
  render context parameter itself has been renamed.

Use `scripts/repro/vue-render-matrix/` to reproduce Vue compiler output and
track which generated shapes the current recovery supports.
