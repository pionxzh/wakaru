<div align="center">

# Wakaru

**Unpack. Unminify. Understand.**

Fast JavaScript decompiler and bundle splitter for modern frontend.

[![CI](https://img.shields.io/github/actions/workflow/status/pionxzh/wakaru/rust-ci.yml?branch=main&label=CI)](https://github.com/pionxzh/wakaru/actions/workflows/rust-ci.yml)
[![npm](https://img.shields.io/npm/v/@wakaru/cli?label=npm)](https://www.npmjs.com/package/@wakaru/cli)
[![Telegram](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)

[Try the online playground](https://wakaru.vercel.app/playground)

</div>

## Quick Start

```bash
npx @wakaru/cli input.js -o output.js               # decompile a file
npx @wakaru/cli bundle.js --unpack -o out/          # unpack and decompile a bundle
npx @wakaru/cli dist/ --unpack -o out/              # scan a bundle output directory
```


## Features

- ✅ Bundle splitting — webpack 4/5, esbuild, Bun, Browserify, SystemJS, AMD
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
npm install -g @wakaru/cli@latest
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

### Unpack bundles and chunks

```bash
wakaru bundle.js --unpack -o out/
wakaru bundle.js --unpack --raw -o out/       # raw split, no readability transforms
wakaru bundle.js --unpack=strict -o out/      # structural detection only, no heuristic fallback
wakaru entry.js chunk.js --unpack -o out/     # unpack multiple explicit files
wakaru dist/ --unpack -o out/                 # recursively scan a directory
```

Directory inputs are supported only with `--unpack`. Wakaru recursively scans
`.js`, `.mjs`, and `.cjs` files, skips hidden files/directories and
`node_modules`, and includes only files detected as bundles or chunks. Skipped
files are not copied or decompiled. Explicit file inputs keep the normal
fallback behavior when no bundle format is detected.

### Formatter

```bash
wakaru input.js --formatter -o output.js
wakaru bundle.js --unpack --formatter -o out/
```

`--formatter` runs a final formatting pass after decompilation. Off by default.

### Source maps

```bash
wakaru input.js --source-map input.js.map -o output.js
wakaru input.js --emit-source-map -o output.js    # emit output .map alongside decompiled file
```

Source maps enable identifier recovery and import deduplication. They are
currently supported only with a single input file.

`--emit-source-map` writes a `.map` file alongside each output file, mapping
the decompiled output back to the input.

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
wakaru input.js --dce                 # remove all dead code (full reachability sweep)
```

By default, only transform-induced dead code is removed; pre-existing dead code
in the input is preserved. `--dce` opts into a full reachability sweep.

### JSON output

```bash
wakaru bundle.js --unpack --json -o out/    # machine-readable JSON to stdout
echo 'var a=1;' | wakaru --json             # single-file JSON (includes code)
```

`--json` writes structured JSON to stdout instead of human-readable summaries.
Warnings and errors are included in the JSON object. Useful for CI pipelines
and tooling integration. In unpack mode, each module includes an artifact
`kind` such as `javascript` or `vue_sfc` and a `status` such as `decompiled`,
`recovered_vue_sfc`, or `vue_sfc_fallback_js` for likely-Vue modules that
could not be recovered as SFC output.

### Diagnostics and profiling

```bash
wakaru input.js --diagnostics                  # post-transform diagnostic checks to stderr
wakaru input.js --profile trace.json           # Chrome trace (open with chrome://tracing)
wakaru input.js --profile trace.json --profile-rules  # include per-rule spans
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

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for full setup notes.

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
