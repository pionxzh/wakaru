# Angular Ivy generated fixture

This fixture is generated from a real Angular production application. It is
the primary producer regression for Angular artifact recovery; public website
bundles are supplementary local stress tests, not the specification.

The pinned Angular CLI application produces:

- `dist/runtime.js` — the shared, minified Angular runtime chunk;
- `dist/main.js` — two eagerly loaded application components;
- `dist/lazy.js` — one component in a lazy ESM chunk;
- `dist/closure-simple.js` — the same three chunks passed through Closure
  Compiler `SIMPLE`;
- `dist/closure-advanced.js` — a separate producer entry passed through
  Closure Compiler `ADVANCED` with explicit retained roots and externs;
- `dist/closure-advanced-structural.js` — a minimally rooted `ADVANCED`
  profile that preserves the component definition role by name but requires
  template instruction roles to be inferred from renamed runtime bodies and
  their use in compiled templates;
- `dist/template-constructs.js` — direct full-AOT Angular compiler output for
  isolated flat bindings and nested template constructs;
- `dist/template-constructs-assignment.js` — the same direct compiler output
  with its top-level embedded-view functions lowered to stable predeclared
  assignments, matching the binding form used by Closure ModuleManager-style
  packaging.

The source deliberately exercises element structure, static attributes, text
and expression interpolation, listeners, property/attribute/class/style
bindings, nested `@if` / `@else` embedded views, `@let`, pure literal
expressions, HTML/SVG/MathML namespace switches, content projection, local
template references, pipe bindings, selector matrices, component styles, and a
cross-chunk component. The generated files contain production Ivy definitions;
they do not contain `ɵsetClassMetadata` or a copy of the original HTML template
literal.

`src/isolated/template-constructs.component.ts` is compiled directly with the
pinned Angular compiler in full AOT mode, then passed through esbuild syntax
minification with `ngDevMode=false`. It isolates:

- flat event, attribute, class, style, property, and multi-expression text
  bindings;
- expression interpolation used by property and attribute bindings;
- pure object/array literal bindings emitted through `ɵɵpureFunction*`;
- `@let` declarations read by interpolation and a nested conditional;
- `<ng-container>`, bounded static/interpolated i18n messages, and element and
  attribute selector matrices;
- SVG and MathML trees followed by an HTML element, including the corresponding
  namespace switches and a statically split constant-attribute table;
- `@if`, `@for` / `@empty`, projection, a loop-local reference, and a pipe,
  including the view restoration and `$implicit` aliases emitted for a loop
  listener;
- a multi-level `@let` / `@for` / `@if` listener that combines a local
  reference, loop context, parent let value, and multiple ordered effects;
- structural i18n element markers and interpolation, plus selected and default
  `<ng-content>` fallback views;
- signal-backed two-way binding and Angular's static/dynamic
  `animate.enter` / `animate.leave` bindings and animation listener;
- `@defer` with primary, loading, placeholder, and error views;
- negative `prefetch on idle` and `hydrate on idle` defer variants, which
  must remain partial rather than being mislabeled as ordinary `on idle`;
- legacy `*ngIf` and `*ngFor`, whose authored shorthand is absent from the
  output and must remain an honest neutral `<ng-template>` representation.

The direct compiler fixture complements the chunk fixture: it pins exact
instruction vocabulary without making the test depend on application bundler
chunking or tree shaking.

The assignment-backed derivative is produced mechanically from that compiler
output with the pinned TypeScript AST factory. It contains no copied production
code and changes only the declaration form of top-level embedded-view
functions. This keeps the recovery regression tied to real Angular output while
pinning the `var view; view = function (...) { ... }` module shape.

Regenerate with:

```bash
cd crates/core/tests/bundles/angular-ivy-gen
npm ci
npm run generate
```

For the ordinary Angular and Closure `SIMPLE` artifacts, the generator
canonicalizes only output filenames and their relative import specifiers. For
the isolated compiler fixture, esbuild removes development-only metadata and
performs syntax-only minification; it does not bundle dependencies or mangle
names. The assignment-backed derivative is then printed from the same generated
AST as described above.

The `ADVANCED` fixture is built from `src/advanced-main.ts`. Its producer
contract exposes three things through properties declared in
`closure-advanced.externs.js`: the component classes, their compiled `ɵcmp`
definition values, and a narrow map from public Ivy instruction names to the
runtime functions used by the templates. The compiler input is not rewritten.

The structural `ADVANCED` fixture is built from
`src/advanced-structural-main.ts`. It keeps the same component roots but
exports only `ɵɵdefineComponent` by canonical name. An otherwise anonymous
control-flow runtime root keeps Angular's generic first-call/continuation
template helper family observable. Wakaru must identify that family from
template argument shape, the returned self-continuation, and shared
parameter-forwarding behavior.

The structural components require Closure-renamed roles to be recovered from
runtime contracts and template use. They cover conditional and property
instructions; attribute/class/style families; `<ng-container>` and bounded
i18n; pure literal expressions; `@let`; HTML/SVG/MathML namespace switches;
optimized repeater and defer families; signal query APIs and an optional-chain
listener; view-local listener aliases; structural i18n; projection fallbacks;
and two-way/animation binding families. One conditional view has a captured
listener and parent-context property binding.
Closure preserves structurally recognizable `nextContext`, `restoreView`, and
`resetView` bodies but inlines `getCurrentView` into a member read, so recovery
must prove the view-state family and validate that use. Separate components
retain `prefetch on idle` and `hydrate on idle` helpers as negative evidence:
those helpers must not be conflated with the ordinary idle trigger.
Selector-only components also prove that selector matrices are reconstructed
independently of readable descriptor property names.

All three roots are necessary. Exporting a class alone does not make an unused
static `ɵcmp` assignment observable to Closure, and retaining component
definitions alone does not preserve canonical instruction-role evidence.
Running generic `ADVANCED` without this contract still removes the metadata;
that output is intentionally not a positive decompiler fixture.
