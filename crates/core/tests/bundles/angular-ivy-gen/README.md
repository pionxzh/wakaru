# Angular Ivy generated fixture

This fixture is generated from a real Angular production application. It is
the primary producer regression for Angular artifact recovery; public website
bundles are supplementary local stress tests, not the specification.

The pinned Angular CLI application produces:

- `dist/runtime.js` — the shared, minified Angular runtime chunk;
- `dist/main.js` — two eagerly loaded application components;
- `dist/lazy.js` — one component in a lazy ESM chunk;
- `dist/closure-simple.js` — the same three chunks passed through Closure
  Compiler `SIMPLE`.

The source deliberately exercises element structure, static attributes, text
interpolation, a listener, a property binding, component styles, and a
cross-chunk component. The generated files contain production Ivy definitions;
they do not contain `ɵsetClassMetadata` or a copy of the original HTML template
literal.

Regenerate with:

```bash
cd crates/core/tests/bundles/angular-ivy-gen
npm ci
npm run generate
```

The generator canonicalizes only output filenames and their relative import
specifiers. It does not rewrite component or runtime code.

Closure `ADVANCED` is not committed as a positive fixture. Running generic
`ADVANCED` over an Angular application without Angular-aware externs and
retained roots lets whole-program dead-code elimination remove component
metadata. A future advanced fixture must encode that producer contract
explicitly rather than making the decompiler recover code that no longer
exists.
