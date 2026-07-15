---
name: wakaru
description: >-
  Turn minified, bundled, or transpiled JavaScript back into readable modules.
  Use when you encounter unreadable production JS — a webpack/esbuild/Rollup
  bundle, a minified vendor script, Babel/TypeScript/SWC-transpiled output, or
  a single mangled .js file — and need to read, audit, debug it, or recover a
  best-effort SFC artifact from compiled Vue 3 component code. Not a
  deobfuscator.
license: Apache-2.0
---

# Wakaru — JavaScript decompiler and bundle unpacker

Wakaru recovers readable source from production JavaScript: it splits bundles
into modules, reverses transpiler helpers (async/await, classes, JSX, spread,
optional chaining, …), and undoes minification. It recovers **structure**
deterministically and applies only conservative renaming heuristics — most
mangled locals stay short unless a source map is provided.

## When to use this

- A file is one giant line, or full of `_interopRequireDefault`,
  `__awaiter`, `e,t,r` parameters, `void 0`, `!0`/`!1`.
- You have a webpack/esbuild/Bun/Browserify/SystemJS/AMD/Rollup/Vite bundle
  and need the individual modules.
- You have compiled Vue 3 component JavaScript and want a best-effort `.vue`
  artifact for inspection.
- A stack trace points into vendored/minified code you can't read.
- You're auditing what a site or dependency actually ships.

## Setup

Requires the CLI. Prefer running via `npx` (no global install):

```bash
npx @wakaru/cli --version
```

If invoked repeatedly, install once: `npm install -g @wakaru/cli@latest`.
In this document `wakaru` means `npx @wakaru/cli` unless installed globally.

## Core workflows

### 1. Decompile a single file

For one minified/transpiled file. Use `--json` for structured stdout. Without
`-o`, the decompiled source is included in the JSON `code` field; with `-o`,
the file is still written and `code` is omitted:

```bash
echo '<code>' | wakaru --json
# → {"code":"...readable...","warnings":[],"elapsed_ms":N}
```

Or file-to-file: `wakaru input.js -o output.js` (stdout if `-o` omitted).

### 2. Unpack a bundle (the important one)

A bundle can explode into thousands of modules — do **not** dump them all
into context. Unpack to a directory, inspect the JSON output, then open only
the files you need:

```bash
wakaru bundle.js --unpack --json -o out/
# → {"detected_formats":["webpack4"],"modules":[{"filename":"module-0.js"},...],
#    "total":42,"failed":0,"warnings":[],"elapsed_ms":N}
```

Use a fresh output directory. Wakaru refuses to write into a non-empty
directory unless `--force` is passed; use `--force` only after confirming that
overwriting its contents is acceptable.

Then read specific files from `out/` (e.g. `out/module-0.js`) on demand. Triage
by size first (`ls -lS out/`) — the largest modules are usually vendored
libraries; the app code is often smaller and more numerous.

Variants:

```bash
wakaru dist/ --unpack --json -o out/          # scan a build-output directory
wakaru entry.js chunk.js --unpack -o out/     # explicit entry + chunk files
```

### 3. Recover names / original source when a map exists

```bash
wakaru input.js --source-map input.js.map -o output.js   # recover names
wakaru extract input.js.map -o src/                       # dump sourcesContent
```

Input `--source-map` is single-file only and cannot be combined with
`--unpack`; extracted modules do not retain the bundle's generated coordinates.
Use `--emit-source-map` when unpacked output maps are needed.

### 4. Recover Vue 3 components as SFC artifacts

Use `--vue-sfc` when the input is compiled Vue 3 component JavaScript or a
bundle likely to contain it. Recovery is best-effort and additive: unpack mode
still writes JavaScript for every module, and recoverable Vue modules also get
sibling `.vue` artifacts.

```bash
wakaru input.js --vue-sfc
wakaru input.js --vue-sfc -o App.vue
wakaru bundle.js --unpack --vue-sfc --json -o out/
```

For batch analysis, prefer `--unpack --vue-sfc --json` and inspect the JSON
output first. Each `modules` entry describes an output artifact. Its `kind` is
`javascript` or `vue_sfc`; Vue-related `status` values are
`recovered_vue_sfc`, `vue_sfc_source_js`, and `vue_sfc_fallback_js`. Open
recovered `.vue` files for template inspection, but keep the paired JavaScript
artifact around when recovery falls back or looks too heuristic. Do not
present recovered SFCs as original source.

## Heavily obfuscated input

For string-array encoding, control-flow flattening, VM protectors, and similar
obfuscation, first use [webcrack](https://github.com/j4k0xb/webcrack) to strip
the obfuscation. Leave unpacking and unminifying to Wakaru:

```bash
# 1. Strip the obfuscation; leave unpacking and unminifying to Wakaru.
npx webcrack --no-unpack --no-unminify obfuscated.js > deobfuscated.js

# 2. Recover readable modules.
npx @wakaru/cli deobfuscated.js --unpack -o out/
```

## Rewrite levels — pick by intent

- `--level minimal` — near-zero semantic change. **Prefer for security review,
  auditing, or diffing** to minimize semantic risk, but do not treat its output
  as a formal equivalence guarantee.
- `--level standard` — default; balanced readability and correctness.
- `--level aggressive` — maximum readability; stronger heuristics that may
  alter edge-case behavior. Use when you just need to understand the code.

By default, Wakaru removes only dead code introduced by its own transforms and
preserves dead code already present in the input. Use `--dce` when a full
reachability sweep is desired.

## Interpreting output

- **Exit code** 0 = success, non-zero = failure (parse error, I/O). Errors go
  to stderr; `--json` output goes to stdout.
- Inspect every JSON warning's `is_error` field. Entries with `is_error: false`
  are non-fatal; an error-class warning makes the command fail even though the
  JSON output and successfully recovered files may still be written.
- `failed` in unpack JSON counts modules that errored during decompilation;
  `total` is the module count. Treat `failed > 0` as a failed run.
- With `--vue-sfc`, `recovered_vue_sfc` means a `.vue` artifact was written;
  `vue_sfc_source_js` is the paired JavaScript for that recovered module; and
  `vue_sfc_fallback_js` means the module looked Vue-like but stayed JavaScript.
- Mangled short names (`e`, `t`, `n`) in the output are expected without a
  source map — Wakaru renames only where the code gives evidence. Pair with
  an LLM renamer or a source map if names matter.

## Safety

Only decompile code you are authorized to analyze. Reading what a third party
ships is a legitimate security-research and debugging activity; using it
against targets without consent may be illegal. You are responsible for
compliance.
