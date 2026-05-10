<div align="center">

# Wakaru

**Unpack. Unminify. Understand.**

Fast JavaScript decompiler and bundle splitter for modern frontend.

[![CI](https://img.shields.io/github/actions/workflow/status/pionxzh/wakaru/rust-ci.yml?branch=main&label=CI)](https://github.com/pionxzh/wakaru/actions/workflows/rust-ci.yml)
[![npm](https://img.shields.io/npm/v/@wakaru/cli?label=npm)](https://www.npmjs.com/package/@wakaru/cli)
[![Telegram](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)

</div>

## Before / After

Minified code goes in:

```js
var a = void 0;
var b = !0 ? 1 : 2;
exports.foo = function(x) { return x === null || x === void 0 ? void 0 : x.bar; };
```

Clean code comes out:

```js
const a = undefined;
const b = 1;
export function foo(x) {
  return x?.bar;
}
```

## Quick Start

```bash
npx @wakaru/cli input.js -o output.js               # decompile a file
npx @wakaru/cli bundle.js --unpack -o out/          # unpack and decompile a bundle
```


## Features

- ✅ Bundle splitting — webpack 4/5, esbuild, Bun, Browserify
- ✅ Transpiler & minifier recovery — Terser, Babel, SWC, TypeScript
- ✅ Source map support for better names & import deduplication
- ✅ Rewrite levels: `minimal` | `standard` | `aggressive`


## Why Wakaru?

Production JavaScript is hard to read because multiple tools have transformed it:

- **Bundlers** collapse many modules into one file and inject runtime wrappers
- **Transpilers** downgrade modern syntax and insert helper functions
- **Minifiers** erase names, fold constants, and compress control flow

Wakaru handles all three in a single command — feed it a bundle, get back readable modules.


## Install

```bash
npm install -g @wakaru/cli
```

Or pre-built binaries from [GitHub Releases](https://github.com/pionxzh/wakaru/releases).


## CLI Reference

### Decompile a single file

```bash
wakaru input.js -o output.js
```

Without `-o`, output goes to stdout. Stdin is also supported:

```bash
cat input.js | wakaru > output.js
```

### Unpack a bundle

```bash
wakaru bundle.js --unpack -o out/
wakaru bundle.js --unpack --raw -o out/    # raw split, no readability transforms
```

### Source maps

```bash
wakaru input.js --source-map input.js.map -o output.js
```

Source maps enable identifier recovery and import deduplication.

### Extract original sources

```bash
wakaru extract input.js.map -o src/
```

Writes files embedded in the source map's `sourcesContent` to disk.

### Rewrite level

Wakaru offers three rewrite levels so you can choose the right tradeoff for your use case:

| Level | When to use |
|-------|-------------|
| `minimal` | You need near-zero semantic changes — only safe, obvious transforms. Good for auditing or diffing where behavioral fidelity matters most. |
| `standard` | Default. Balanced readability and correctness for most use cases. |
| `aggressive` | You just want to read the code. Enables stronger intent-recovery heuristics that produce cleaner output but may alter edge-case behavior. |

```bash
wakaru input.js --level minimal
wakaru input.js --level standard      # default
wakaru input.js --level aggressive
```

### Overwrite protection

Wakaru refuses to overwrite existing files unless `--force` is passed.

## Contributing

Every kind of contribution is welcome.

Some areas where help is especially useful:

- Share real-world bundles that Wakaru doesn't handle well
- Report missing helper detection or false positives
- Report semantic or correctness issues

When reporting a bug, please include: the input code, the command you ran, the current output, and what you expected instead.

<details>
<summary>Development setup</summary>

1. Fork the repo and create your branch from `main`
2. Install a stable Rust toolchain
3. Run `cargo test` to verify everything passes
4. Make your changes and add tests

Before submitting a PR:

```bash
cargo test
cargo clippy -- -D warnings
```

This project uses [Conventional Commits](https://www.conventionalcommits.org/). Please mention the issue number in the commit message or PR description.

Docs: [`architecture.md`](./docs/architecture.md) | [`testing.md`](./docs/testing.md) | [`helper-detection.md`](./docs/helper-detection.md)

</details>

## License

[Apache-2.0](./LICENSE)

<sub>Usage of wakaru for attacking targets without prior mutual consent is illegal. End users are responsible for complying with all applicable laws.</sub>
