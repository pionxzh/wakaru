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

The source deliberately exercises element structure, static attributes, text
interpolation, a listener, a property binding, nested `@if` / `@else` embedded
views, content projection, a local template reference, a pipe binding,
component styles, and a cross-chunk component. The generated files contain
production Ivy definitions; they do not contain `ɵsetClassMetadata` or a copy
of the original HTML template literal.

Regenerate with:

```bash
cd crates/core/tests/bundles/angular-ivy-gen
npm ci
npm run generate
```

For the ordinary Angular and Closure `SIMPLE` artifacts, the generator
canonicalizes only output filenames and their relative import specifiers. It
does not rewrite component or runtime code.

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
