---
name: wakaru
description: >-
  Turn minified, bundled, or transpiled JavaScript back into readable modules.
  Use when you encounter unreadable production JS — a webpack/esbuild/Rollup
  bundle, a minified vendor script, Babel/TypeScript/SWC-transpiled output, or
  a single mangled .js file — and need to read, audit, debug it, or recover a
  best-effort Vue 3 SFC artifact from generated render code. Not a deobfuscator
  (see webcrack for obfuscator.io-style protection).
homepage: https://github.com/pionxzh/wakaru
license: Apache-2.0
---

# Wakaru — JavaScript decompiler and bundle unpacker

Wakaru recovers readable source from production JavaScript: it splits bundles
into modules, reverses transpiler helpers (async/await, classes, JSX, spread,
optional chaining, …), and undoes minification. It recovers **structure**
deterministically; it does not invent identifier names (mangled locals stay
mangled unless a source map is provided).

## When to use this

- A file is one giant line, or full of `_interopRequireDefault`,
  `__awaiter`, `e,t,r` parameters, `void 0`, `!0`/`!1`.
- You have a webpack/esbuild/Bun/Browserify/SystemJS/AMD/Rollup/Vite bundle
  and need the individual modules.
- You have generated Vue 3 render JavaScript and want a best-effort `.vue`
  artifact for inspection.
- A stack trace points into vendored/minified code you can't read.
- You're auditing what a site or dependency actually ships.

**Do not use it for** heavily obfuscated code (string-array encoding,
control-flow flattening, VM protectors). Run
[webcrack](https://github.com/j4k0xb/webcrack) first, then wakaru on its output.

## Setup

Requires the CLI. Prefer running via `npx` (no global install):

```bash
npx @wakaru/cli --version
```

If invoked repeatedly, install once: `npm install -g @wakaru/cli@latest`.
In this document `wakaru` means `npx @wakaru/cli` unless installed globally.

## Core workflows

### 1. Decompile a single file

For one minified/transpiled file. Use `--json` to get output as structured
data instead of writing a file:

```bash
echo '<code>' | wakaru --json
# → {"code":"...readable...","warnings":[],"elapsed_ms":N}
```

Or file-to-file: `wakaru input.js -o output.js` (stdout if `-o` omitted).

### 2. Unpack a bundle (the important one)

A bundle can explode into thousands of modules — do **not** dump them all
into context. Unpack to a directory, read the JSON manifest, then open only
the modules you need:

```bash
wakaru bundle.js --unpack --json -o out/
# → {"detected_formats":["webpack4"],"modules":[{"filename":"module-0.js"},...],
#    "total":42,"failed":0,"warnings":[],"elapsed_ms":N}
```

Then read specific files from `out/` (e.g. `out/module-0.js`) on demand. Triage
by size first (`ls -lS out/`) — the largest modules are usually vendored
libraries; the app code is often smaller and more numerous.

Variants:

```bash
wakaru dist/ --unpack --json -o out/          # scan a build-output directory
wakaru entry.js chunk.js --unpack -o out/     # explicit entry + chunk files
wakaru bundle.js --unpack --provenance -o out/ # also write provenance.json:
                                              # which input byte ranges each
                                              # module came from
```

### 3. Recover names / original source when a map exists

```bash
wakaru input.js --source-map input.js.map -o output.js   # recover names
wakaru extract input.js.map -o src/                       # dump sourcesContent
```

### 4. Recover Vue 3 render modules as SFC artifacts

Use `--vue-sfc` only when the input is generated Vue 3 render JavaScript or a
bundle likely to contain Vue render modules. It is best-effort and additive:
unpack mode still writes JavaScript for every module, and recoverable Vue
modules also get sibling `.vue` artifacts.

```bash
wakaru input.js --vue-sfc
wakaru input.js --vue-sfc -o App.vue
wakaru bundle.js --unpack --vue-sfc --json -o out/
```

For batch analysis, prefer `--unpack --vue-sfc --json` and inspect the manifest
first. Modules use `kind` values such as `javascript` or `vue_sfc`, and
`status` values such as `recovered_vue_sfc`, `vue_sfc_source_js`, or
`vue_sfc_fallback_js`. Open recovered `.vue` files for template inspection, but
keep the paired JavaScript artifact around when recovery falls back or looks
too heuristic. Do not present recovered SFCs as original source.

## Rewrite levels — pick by intent

- `--level minimal` — near-zero semantic change. **Use for security review,
  auditing, or diffing**, where the semantics you read must be the semantics
  that ran.
- `--level standard` — default; balanced readability and correctness.
- `--level aggressive` — maximum readability; stronger heuristics that may
  alter edge-case behavior. Use when you just need to understand the code.

## Interpreting output

- **Exit code** 0 = success, non-zero = failure (parse error, I/O). Errors go
  to stderr; `--json` output goes to stdout.
- `warnings` in the JSON are non-fatal (e.g. a pattern wakaru couldn't fully
  recover). The output is still usable.
- `failed` in unpack JSON counts modules that errored during decompilation;
  `total` is the module count.
- With `--vue-sfc`, `recovered_vue_sfc` means a `.vue` artifact was written;
  `vue_sfc_source_js` is the paired JavaScript for that recovered module; and
  `vue_sfc_fallback_js` means the module looked Vue-like but stayed JavaScript.
- Mangled short names (`e`, `t`, `n`) in the output are expected without a
  source map — wakaru recovered the structure, not the names. Pair with an
  LLM renamer or a source map if names matter.

## Safety

Only decompile code you are authorized to analyze. Reading what a third party
ships is a legitimate security-research and debugging activity; using it
against targets without consent may be illegal. You are responsible for
compliance.
