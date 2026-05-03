# Wakaru

[![codecov][CodecovBadge]][CodecovRepo]
[![Telegram-group](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)

Wakaru is a fast JavaScript decompiler and bundle splitter. It turns minified, bundled, and transpiled JavaScript back into readable modern JavaScript.

The current rewrite is written in Rust with the SWC AST ecosystem.

> Early access: the Rust CLI is published on the `next` npm tag. The previous
> TypeScript CLI remains available on the `latest` npm tag for production use.

## Install

### npm

```bash
npx @wakaru/cli@next input.js
```

Or install globally:

```bash
npm install -g @wakaru/cli@next
```

### Pre-built binaries

Download a binary from [GitHub Releases](https://github.com/pionxzh/wakaru/releases).

| Platform | Archive |
|----------|---------|
| Linux x64 | `wakaru-linux-x64.tar.gz` |
| macOS ARM64 | `wakaru-darwin-arm64.tar.gz` |
| Windows x64 | `wakaru-win32-x64.zip` |

### Build from source

```bash
git clone https://github.com/pionxzh/wakaru.git
cd wakaru/wakaru-rs
cargo install --path .
```

## Usage

### Decompile a single file

```bash
wakaru input.js -o output.js
```

Without `-o`, Wakaru prints the decompiled output to stdout.

Wakaru also accepts stdin:

```bash
cat input.js | wakaru > output.js
wakaru - -o output.js
```

### Unpack a bundle

```bash
wakaru bundle.js --unpack -o out/
```

This splits a supported bundle into readable module files under `out/`. The
output directory is required for unpacking.

For raw splitter output before the readability pipeline:

```bash
wakaru bundle.js --unpack --raw -o out/
```

### Use a source map

```bash
wakaru input.js --source-map input.js.map -o output.js
wakaru bundle.js --unpack --source-map bundle.js.map -o out/
```

Source maps are used for identifier recovery and import deduplication.
`--sourcemap` is also accepted as an alias.

### Extract original sources from a source map

```bash
wakaru extract input.js.map -o src/
```

This writes files embedded in the source map's `sourcesContent` to disk. The
generated JavaScript file is not needed for this operation. If the source map
does not contain `sourcesContent`, extraction cannot recover the original files.

### Rewrite level

```bash
wakaru input.js --level minimal
wakaru input.js --level standard
wakaru input.js --level aggressive
```

`standard` is the default. `minimal` keeps rewrites conservative, while
`aggressive` enables stronger intent-recovery heuristics that may be less
appropriate for edge-case-sensitive code.

### Overwrite protection

Wakaru refuses to overwrite existing output files and refuses to write into
non-empty output directories unless `--force` is passed.

```bash
wakaru input.js -o output.js --force
wakaru bundle.js --unpack -o out/ --force
```

## Supported Inputs

Wakaru detects these bundle formats automatically when unpacking:

| Format | Support |
|--------|---------|
| webpack 4 | yes |
| webpack 5 | yes |
| Browserify | yes |
| esbuild | yes |

Wakaru also restores common output patterns from:

- Terser
- Babel
- SWC
- TypeScript

## What It Does

Wakaru runs a pipeline of AST transforms to undo common bundler, minifier, and
transpiler output:

- Splits bundles into individual modules
- Restores `import` / `export` from CommonJS patterns
- Removes common bundler interop wrappers
- Simplifies sequence expressions and indirect calls
- Restores booleans, `undefined`, numeric literals, and template literals
- Promotes `var` to `let` / `const` where safe
- Recovers arrow functions, shorthand properties, optional chaining, JSX,
  default parameters, object spread, rest parameters, and related syntax
- Uses source maps to recover original identifier names when available

## Development

Rust crate and CLI sources live in [`wakaru-rs/`](./wakaru-rs/).

```bash
cd wakaru-rs
cargo test
```

Useful docs:

- [`architecture.md`](./wakaru-rs/docs/architecture.md)
- [`testing.md`](./wakaru-rs/docs/testing.md)
- [`helper-detection.md`](./wakaru-rs/docs/helper-detection.md)
- [`debugging.md`](./wakaru-rs/docs/debugging.md)

The legacy TypeScript implementation is preserved on the `legacy-ts` branch
during the transition period.

## Legal Disclaimer

Usage of `wakaru` for attacking targets without prior mutual consent is illegal.
It is the end user's responsibility to obey all applicable local, state, and
federal laws. Developers assume no liability and are not responsible for misuse
or damage caused by this program.

## License

[Apache-2.0](./LICENSE)

[CodecovBadge]: https://img.shields.io/codecov/c/github/pionxzh/wakaru
[CodecovRepo]: https://codecov.io/gh/pionxzh/wakaru
