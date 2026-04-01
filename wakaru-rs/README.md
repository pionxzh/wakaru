# wakaru-rs

A fast JavaScript decompiler and bundle splitter.

Takes minified or bundled JavaScript and produces readable, modern JavaScript.

---

## Install

```bash
cargo build --release
# binary at: target/release/wakaru-rs
```

---

## Usage

### Decompile a single file

```bash
wakaru-rs input.js -o output.js
```

Prints to stdout when `-o` is omitted.

### Unpack a bundle into individual modules

```bash
wakaru-rs input.js --unpack -o out/
```

Splits a bundle into one file per module under `out/`. Defaults to `unpacked/` when `-o` is omitted.

### Recover original names with a source map

```bash
wakaru-rs input.js -o output.js -m input.js.map
wakaru-rs input.js --unpack -o out/ -m input.js.map
```

Uses source map position data to restore original identifier names. Works with or without a `names` array — names are extracted directly from `sourcesContent` when available.

### Extract original source files from a source map

```bash
wakaru-rs input.js --extract -m input.js.map -o src/
```

Writes the embedded `sourcesContent` files to disk as-is. Does not decompile. Requires `-m`.

---

## Supported bundle formats

| Format | Detected automatically |
|---|---|
| webpack 4 | yes |
| webpack 5 | yes |
| Browserify | yes |
| esbuild | yes |

---

## What it does

Runs a pipeline of AST transforms to undo common minification patterns:

- Sequence expressions split into statements (`a(), b()` → `a(); b();`)
- Minified boolean/undefined literals restored (`!0` → `true`, `void 0` → `undefined`)
- Template literals recovered (`.concat()` chains → `` `${...}` ``)
- Bracket notation simplified (`obj["foo"]` → `obj.foo`)
- Indirect calls cleaned up (`(0, fn)(x)` → `fn(x)`)
- IIFEs unwrapped
- `var` promoted to `let`/`const` where safe
- Arrow functions and method shorthand restored
- CommonJS `require` / `exports` patterns reconstructed as ESM `import`/`export`
- Dead code from bundler feature flags removed

Source map support adds:
- Duplicate import deduplication (bundlers repeat imports across modules)
- Position-based identifier rename (recovers original variable names)
