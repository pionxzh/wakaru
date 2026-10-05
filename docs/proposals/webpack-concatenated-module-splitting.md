# Webpack Concatenated Modules: Split at External-Import Boundaries

Status: **PROPOSED.** Not started. A planned import-hoisting barrier in
UnEsm (see "Relationship to the hoisting barrier" below) is the short-term
guard for the same miscompile; this proposal is the structural fix.

Ground rules: follow [AGENTS.md](../../AGENTS.md), including a focused unit
test for every change. Use synthetic module ids and filenames in tests and
commits. Update [unpacking.md](../unpacking.md) in the same commit that
changes splitting behavior.

## Problem

Webpack's `ModuleConcatenationPlugin` (on by default in production builds)
merges several ESM source modules into one module factory. Inside the merged
factory, each inner module's code runs in source evaluation order, and an
inner module's import of a module that could not be concatenated (a CommonJS
dependency, a module shared with another chunk) is emitted inline, at the
point where that inner module starts:

```js
// src/patch.js
globalThis.fetch = spy;
export const patched = true;

// src/player.js
import dep from "./cjs-dep.js";        // cjs-dep.js captures globalThis.fetch at load
export function report() { return dep.usesSpy(); }

// webpack 5 production, concatenated (unminified for reading):
__webpack_require__.d(__webpack_exports__, { main: () => main });
globalThis.fetch = spy;                                   // patch.js body
// EXTERNAL MODULE: ./src/cjs-dep.js
var cjs_dep = __webpack_require__(625);                   // player.js's import
var cjs_dep_default = __webpack_require__.n(cjs_dep);
function report() { return cjs_dep_default().usesSpy(); }
```

The bundle loads `cjs-dep.js` after the patch, which is the ESM semantics of
the source. wakaru turns the whole factory into one ES module. The `require`
becomes an `import`, and ES imports evaluate before the module body, so the
recovered module loads `cjs-dep.js` before the patch:

```js
import cjs_dep from "./module-625.js";   // now runs first
globalThis.fetch = spy;
```

One ES module cannot express "evaluate this import after that code". Only
the original split into several modules can. Production builds strip the
`// ./src/x.js` inner-module comments, so the split has to be inferred.

## Current behavior

- `UnEsm` converts every activation-time `require` of a resolvable module to
  an import, and the imports evaluate before any statement that stays in the
  module body. A concatenated factory therefore becomes one ESM module whose
  external imports run before the inner-module code that preceded them.
- Heuristic scope-hoisted splitting (`unpacker/scope_hoist.rs`) clusters
  top-level declarations by reference graph. On detected modules it runs only
  at `--level aggressive` (`nested_scope_split_enabled` in
  `driver/unpack/mod.rs`). It does not use inner-module boundaries, so a split
  module can still import the external dependency above the code that had to
  run first.

## Boundary signal

A webpack factory that does not concatenate puts every harmony import at the
top of the factory, before any other activation code. So, inside a harmony
factory (one that defines exports through `__webpack_require__.d` or marks
itself with `__webpack_require__.r`), an activation-time `__webpack_require__`
that follows non-require activation code is an inner-module boundary. The
`__webpack_require__.n(x)` compat getter right after it, which the unpacker
inlines as `() => x && x.__esModule ? x.default : x`, is corroboration: it only
appears for a harmony default import of a non-harmony module.

Limits of the signal:

- An inner module with no external import has no visible start. That is
  harmless for ordering: it has no external import to move.
- Terser can fold the boundary `var` into an earlier `var` list. The cut point
  is then inside a declaration; splitting the declarator list is required.
- Transpiled ESM→CJS factories (Babel, SWC, TypeScript) can also have code
  before a `require` (helper IIFEs, `_interopRequireDefault` calls). They do
  not carry webpack harmony markers and must not be cut.

## Design

1. **Detect.** In the webpack 4/5 unpackers, mark a factory as concatenated
   when it is a harmony factory and has at least one activation-time
   `__webpack_require__` after non-require activation code. Record the cut
   positions as detector metadata; do not change the factory body.
2. **Plan.** Build segments: each cut starts a new segment that owns the
   boundary require(s) and the code up to the next cut. Feed the segments to
   the existing scope-hoist planner as forced partition boundaries instead of
   reference-graph clusters.
3. **Order.** Emit each segment as a synthetic module. Segment *k* imports
   segment *k−1* for evaluation order (a side-effect import placed before its
   own external imports), plus named imports for bindings it reads from
   earlier segments. The original factory's exports are re-exported from the
   segment that defines each binding.
4. **Merge on cycles.** A function in an earlier segment that references a
   binding in a later segment creates a cycle once both are modules. Merge
   segments whose split would create an activation-time cycle, using the
   scope-hoist emitter's existing SCC merge. A merge can reintroduce the
   reorder inside the merged segment; the hoisting barrier is the guard
   there.
5. **Level.** Run at every level that runs UnEsm on unpacked modules. The
   split preserves evaluation order; it is not a heuristic rewrite. Gate on
   detection, not on `--level aggressive`.

## Validation

- Synthetic webpack 4 and 5 builds (concatenation on, minify on and off):
  two ESM modules with a global write and a CommonJS dependency that reads
  the global at load. Run the bundle and the recovered modules; outputs must
  match.
- The same build with Terser folding the boundary `var` into the previous
  declaration list.
- A transpiled ESM→CJS factory with helper code before its requires must stay
  unsplit.
- Fixture suite and repro stats unchanged except for intended splits.
- Measure how many split factories end up fully merged by cycles; if most do,
  the design does not pay for itself and the barrier stays the only fix.

## Relationship to the hoisting barrier

The planned barrier keeps a later `require` in place when an earlier
statement is a global or `process.env` write, a leftover string `require`
call, or a discarded-result call through a required binding (`p.install();`).
It keeps the `require` in place, which is correct but leaves CommonJS
calls inside ESM output. This split removes the barrier's work for
concatenated factories by giving each inner module its own import list.
The barrier stays for hand-written CommonJS (`require("dotenv").config()`),
which has no ESM equivalent, and for segments merged by cycles.

## Open questions

- Whether webpack 4 concatenation emits the same boundary shape (inner
  modules with external imports) as webpack 5. Check before generalizing the
  detector.
- Whether Rspack's concatenation matches webpack 5 closely enough to reuse
  the detector.
- File naming for segments (`module-<id>/segment-<n>.js` vs. reuse of the
  scope-hoist `chunk_*` naming) and how source maps attribute them.
