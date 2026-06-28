# @wakaru/cli

Fast JavaScript decompiler and bundle splitter for modern frontend code.

## Quick Start

```bash
npx @wakaru/cli input.js -o output.js               # decompile a file
npx @wakaru/cli bundle.js --unpack -o out/          # unpack and decompile a bundle
npx @wakaru/cli dist/ --unpack -o out/              # scan a bundle output directory
```

## Install

```bash
npm install -g @wakaru/cli@latest
wakaru input.js -o output.js
```

## What It Does

- Splits bundles from webpack 4/5, esbuild, Bun, Browserify, SystemJS, and AMD.
- Recovers readable JavaScript from transpiler and minifier output.
- Supports source maps for name recovery and output mappings.
- Offers `minimal`, `standard`, and `aggressive` rewrite levels.

## CLI Reference

### Decompile a single file

```bash
wakaru input.js -o output.js
cat input.js | wakaru > output.js
```

Without `-o`, output goes to stdout. Stdin is supported for single-file
decompilation.

### Unpack bundles and chunks

```bash
wakaru bundle.js --unpack -o out/
wakaru bundle.js --unpack --raw -o out/
wakaru bundle.js --unpack=strict -o out/
wakaru entry.js chunk.js --unpack -o out/
wakaru dist/ --unpack -o out/
```

- `--unpack` splits detected bundles and then decompiles each module.
- `--unpack --raw` writes extracted modules before the readability pipeline.
- `--unpack=strict` uses structural bundle detection without heuristic fallback.
- Directory inputs are recursive and detect-only; skipped files are not copied.

### Formatter

```bash
wakaru input.js --formatter -o output.js
wakaru bundle.js --unpack --formatter -o out/
```

`--formatter` runs a final formatting pass after decompilation. It is off by
default.

### Source maps

```bash
wakaru input.js --source-map input.js.map -o output.js
wakaru input.js --emit-source-map -o output.js
```

`--source-map` improves name recovery from the original source map.
`--emit-source-map` writes a `.map` file that maps the decompiled output back to
the input.

### Extract original sources

```bash
wakaru extract input.js.map -o src/
```

Writes files embedded in a source map's `sourcesContent` to disk.

### Rewrite levels and cleanup

```bash
wakaru input.js --level minimal
wakaru input.js --level standard
wakaru input.js --level aggressive
wakaru input.js --dce
```

- `minimal` keeps to high-confidence, low-risk rewrites.
- `standard` is the default readability-oriented mode.
- `aggressive` enables more speculative generated-code recovery.
- `--dce` opts into a full dead-code reachability sweep.

### JSON, diagnostics, and profiling

```bash
wakaru bundle.js --unpack --json -o out/
echo 'var a=1;' | wakaru --json
wakaru input.js --diagnostics
wakaru input.js --profile trace.json
wakaru input.js --profile trace.json --profile-rules
```

- `--json` writes structured JSON to stdout.
- `--diagnostics` reports post-transform warnings to stderr.
- `--profile` writes a Chrome trace file.
- `--profile-rules` includes per-rule timings in the trace.

### Overwrite protection

Wakaru refuses to overwrite existing files unless `--force` is passed.

## Links

- Repository: https://github.com/pionxzh/wakaru
- Documentation: https://github.com/pionxzh/wakaru#readme
- Releases: https://github.com/pionxzh/wakaru/releases
