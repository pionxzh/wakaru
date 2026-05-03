# Wakaru

A fast JavaScript decompiler and bundle splitter, rewritten in Rust.

Takes minified or bundled JavaScript and produces readable, modern JavaScript — 10-100x faster than the TypeScript version.

> **Early access.** This is the Rust rewrite of [wakaru](https://github.com/pionxzh/wakaru). The TypeScript version remains available as `@wakaru/cli` (latest tag) for production use.

---

## Install

### npm (recommended)

```bash
npx @wakaru/cli@next input.js -o output.js
```

Or install globally:

```bash
npm install -g @wakaru/cli@next
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/pionxzh/wakaru/releases) — no Node.js required.

| Platform | Archive |
|----------|---------|
| Linux x64 | `wakaru-rs-linux-x64.tar.gz` |
| macOS ARM64 | `wakaru-rs-darwin-arm64.tar.gz` |
| Windows x64 | `wakaru-rs-win32-x64.zip` |

### Build from source

```bash
git clone https://github.com/pionxzh/wakaru.git
cd wakaru/wakaru-rs
cargo install --path .
```

---

## Usage

### Decompile a single file

```bash
wakaru input.js -o output.js
```

Prints to stdout when `-o` is omitted.

### Unpack a bundle into individual modules

```bash
wakaru input.js --unpack -o out/
```

Splits a bundle into one file per module under `out/`. Defaults to `unpacked/` when `-o` is omitted.

### Recover original names with a source map

```bash
wakaru input.js -o output.js -m input.js.map
wakaru input.js --unpack -o out/ -m input.js.map
```

Uses source map position data to restore original identifier names. Works with or without a `names` array — names are extracted directly from `sourcesContent` when available.

### Extract original source files from a source map

```bash
wakaru input.js --extract -m input.js.map -o src/
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
- Optional chaining recovered from ternary null checks
- JSX restored from `React.createElement` calls
- Default parameters recovered from `arguments` patterns

Source map support adds:
- Duplicate import deduplication (bundlers repeat imports across modules)
- Position-based identifier rename (recovers original variable names)

---

## Contributing

```bash
cd wakaru-rs
cargo test
```

See [AGENTS.md](../AGENTS.md) for architecture and development guidelines.

---

## License

MIT
