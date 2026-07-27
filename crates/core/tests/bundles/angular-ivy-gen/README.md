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
  Closure Compiler `ADVANCED` with explicit retained roots and externs.
- `dist/template-constructs.js` — direct full-AOT Angular compiler output for
  isolated flat bindings and nested template constructs.

The source deliberately exercises element structure, static attributes, text
interpolation, a listener, a property binding, nested `@if` / `@else` embedded
views, content projection, a local template reference, a pipe binding,
component styles, and a cross-chunk component. The generated files contain
production Ivy definitions; they do not contain `ɵsetClassMetadata` or a copy
of the original HTML template literal.

`src/isolated/template-constructs.component.ts` is compiled directly with the
pinned Angular compiler in full AOT mode, then passed through esbuild syntax
minification with `ngDevMode=false`. It isolates:

- flat event, attribute, class, style, property, and multi-expression text
  bindings;
- `@if`, `@for` / `@empty`, projection, a loop-local reference, and a pipe,
  including the view restoration and `$implicit` aliases emitted for a loop
  listener.

The direct compiler fixture complements the chunk fixture: it pins exact
instruction vocabulary without making the test depend on application bundler
chunking or tree shaking.

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
names.

The `ADVANCED` fixture is built from `src/advanced-main.ts`. Its producer
contract exposes three things through properties declared in
`closure-advanced.externs.js`: the component classes, their compiled `ɵcmp`
definition values, and a narrow map from public Ivy instruction names to the
runtime functions used by the templates. The compiler input is not rewritten.

All three roots are necessary. Exporting a class alone does not make an unused
static `ɵcmp` assignment observable to Closure, and retaining component
definitions alone does not preserve canonical instruction-role evidence.
Running generic `ADVANCED` without this contract still removes the metadata;
that output is intentionally not a positive decompiler fixture.
