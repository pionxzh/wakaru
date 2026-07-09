# Closure ModuleManager generated fixture

This fixture runs `google-closure-compiler@20260629.0.0` over the files in
`src/` using `ADVANCED` compilation and code-split `--chunk` output. The
generator reads the compiler's own chunk dependency JSON and packages the
emitted chunks into the public Closure Library
[`goog.module.ModuleManager`](https://google.github.io/closure-library/api/goog.module.ModuleManager.html)
response contract.

The provenance boundary is intentional:

- Closure Compiler produces every application payload, the empty chunk files,
  and the dependency graph used by the fixture.
- `generate.mjs` supplies the response annotations, loader calls, and guarded
  segments. A Closure Library maintainer describes this
  [serving/bundling pass](https://groups.google.com/g/closure-library-discuss/c/ZEt7LQyImMc)
  as outside Closure Compiler, and its production implementation is not open
  source.

This is therefore a producer-assisted reproduction, not a claim that the
script implements a private serving system. The minimized, fully anonymized
shape in `../closure-module-manager/annotated-served-order-shape.js`
separately covers the structural envelope reported in wakaru#195.

Generate or verify the checked-in output with Node.js and npm:

```sh
npm ci
npm run generate
npm run check
```

The exact compiler dependency and its transitive dependency tree are committed
in `package.json` and `package-lock.json`; the generator invokes only that local
installation and does not let `npx` resolve packages from the network.

Tests consume `dist/compiler-chunks/bundle.js` directly and do not run Node.js
or download Closure Compiler.
