---
name: wakaru
description: >-
  Turn minified, bundled, or transpiled JavaScript back into readable modules.
  Use when you encounter unreadable production JS — a webpack/esbuild/Metro/Rollup
  bundle, a minified vendor script, Babel/TypeScript/SWC-transpiled output, or
  a single mangled .js file — and need to read, audit, debug it, or recover a
  best-effort component artifact from compiled Vue 3 or Angular Ivy code. Not
  a deobfuscator.
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
- You have a webpack bundle, supported Vercel ncc CommonJS output with an IIFE
  webpack bootstrap, or an esbuild/Bun/Metro/Browserify/Cocos Creator 2.x/
  Closure ModuleManager/SystemJS/AMD/Rollup/Vite bundle and need the individual
  modules.
- You have a Bun single-file PE, Mach-O, or ELF executable and need its embedded
  JavaScript without running it.
- You need every file stored in a Bun single-file executable, including binary assets,
  WebAssembly, native add-ons, or embedded databases.
- You have compiled Vue 3 component JavaScript and want a best-effort `.vue`
  artifact for inspection.
- You have production Angular Ivy JavaScript and want readable TypeScript
  components with inline templates and styles.
- A stack trace points into vendored/minified code you can't read.
- You're auditing what a site or dependency actually ships.

## Setup

Requires the CLI. Prefer running via `npx` (no global install):

```bash
npx wakaru --version
```

If invoked repeatedly, install once: `npm install -g wakaru@latest`.
In this document `wakaru` means `npx wakaru` unless installed globally.

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
wakaru ./compiled-app --unpack --raw -o out/  # extract a Bun single-file executable safely
wakaru bun extract ./compiled-app -o raw/     # dump every embedded Bun file byte-for-byte
```

Bun single-file executable extraction accepts an explicit executable path. It validates
the embedded module graph; directory scans and stdin remain JavaScript-only
inputs. `--unpack` selects the JavaScript-like records and sends them through
bundle splitting. Prefer `--raw` when comparing their shipped representation.

Use `wakaru bun extract` instead when the task needs the container itself. It
writes every validated file record below `raw/files/` without decoding or
transforming it and records loader metadata and byte ranges in
`raw/manifest.json`. Add `--json` for the same manifest on stdout.
`--include-internals` also emits opaque Bun source-map, JavaScriptCore bytecode,
and module-info regions; do not treat `source-map.bunmap` as a v3 JSON map.
The extractor supports Bun 1.3.3+.

Ordinary Browserify bundles use unambiguous dependency-map request paths for
readable module filenames. Conflicting or missing hints retain
`module-<id>.js`, and entry names remain stable.

Webpack string module IDs keep their safe relative resource path. JavaScript-
like extensions (`.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx`, `.mts`, and
`.cts`) stay unchanged; every other or extensionless resource appends `.js`
(for example, `style.less` becomes `style.less.js`). Loader queries and URL
fragments do not enter filesystem names, and collisions are made unique before
consumer references are synthesized.

For normal multi-input unpack, heuristic scope-hoisted ESM inputs and
structural esbuild ESM chunks keep their original safe relative input paths as
public entry filenames. Generated children live beneath each public filename's
stem, so sibling ESM imports and dynamic imports continue to resolve. Plain
passthrough inputs keep their relative directory structure too (a
parent-relative `../` prefix is dropped), so same-named files from different
directories coexist and sibling imports between inputs stay resolvable.
Generated modules yield to these reserved physical-input paths. Duplicate
normalized paths claimed by physical ESM identities fail as ambiguous instead
of suffixing either identity; script-loaded bundle inputs do not reserve their
physical filenames. Raw output only skips readability transforms, keeps
provisional extraction names, and has no public-path reservation or usable
module-graph contract.

For development or benchmark triage, validate a normal output tree as one
emitted-module graph:

```bash
wakaru debug validate out/
wakaru debug validate out/ --json
```

This reports dangling relative references, missing or star-ambiguous imported
or re-exported names, duplicate exports, and writes to imported or `const`
bindings, then exits nonzero on findings. Text output uses
`filename:line:column`; JSON
findings include one-based `line` and `column`. The recursive scan accepts
`.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx`, `.mts`, `.cts`, and extensionless
emitted modules, including modules emitted beneath `node_modules`; hidden paths
and unrelated extensions stay excluded. Do not use it to grade raw output,
which has no usable module-graph contract.

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

### 5. Recover Angular Ivy components

Use `--angular` for production Angular AOT output. Recovery is additive in
unpack mode: JavaScript remains available for every module, while proven
components get readable `*.component.ts` artifacts with inline templates and
styles.

```bash
wakaru compiled.js --angular
wakaru compiled.js --angular -o DemoCard.component.ts
wakaru compiled.js --angular -o readable.js
wakaru bundle.js --unpack --angular --json -o out/
wakaru dist/ --unpack --angular --json -o out/
```

Without `-o`, use this only when one component is expected. A `.ts` output is
component-only and requires exactly one recovered component. A JavaScript
output path keeps the decompiled JavaScript and writes all recovered
components as sibling sidecars. `--unpack --angular` is preferred for bundles,
chunks, or directories; directory mode processes ordinary JavaScript modules
as well as detected bundles. In default directory mode it preserves ordinary
production chunks intact so relative ESM symbol edges remain available to the
Ivy analyzer; structural bundle detection still runs.

In JSON, recovered artifacts use `kind: "angular_component"` and status
`recovered_angular_component` or `partial_angular_component`. Paired
JavaScript uses `angular_component_source_js`. Treat partial artifacts as
inspection aids: unsupported Ivy regions remain explicit, and recovered code
is not claimed to be original source. `--angular` is incompatible with
`--raw` and `--vue-sfc`. Add `--diagnostics` to audit component candidates and
the rendered, unsupported, and malformed Ivy runtime-call totals, including
privacy-safe phase/arity summaries for unknown runtime calls.

## Heavily obfuscated input

For string-array encoding, control-flow flattening, VM protectors, and similar
obfuscation, first use [webcrack](https://github.com/j4k0xb/webcrack) to strip
the obfuscation. Leave unpacking and unminifying to Wakaru:

```bash
# 1. Strip the obfuscation; leave unpacking and unminifying to Wakaru.
npx webcrack --no-unpack --no-unminify obfuscated.js > deobfuscated.js

# 2. Recover readable modules.
npx wakaru deobfuscated.js --unpack -o out/
```

## Rewrite levels — pick by intent

- `--level minimal` — near-zero semantic change. **Prefer for security review,
  auditing, or diffing** to minimize semantic risk, but do not treat its output
  as a formal equivalence guarantee.
- `--level standard` — default; balanced readability and correctness.
- `--level aggressive` — maximum readability; stronger heuristics that may
  alter edge-case behavior. Use when you just need to understand the code.
- `--unpack=inspect` — recursively retain finer scope-hoist boundaries for
  static inspection. The resulting module graph may not preserve runtime
  initialization order; the CLI prints a warning whenever this mode is used.
  Add `--raw` independently to skip readability transforms. With
  `--provenance`, optional `context_ranges` group fine siblings that share a
  coarse evidence context; they do not claim that the siblings share one
  package identity.

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
- With `--angular`, `recovered_angular_component` and
  `partial_angular_component` mean a TypeScript component artifact was
  written; `angular_component_source_js` is its paired JavaScript module.
- Mangled short names (`e`, `t`, `n`) in the output are expected without a
  source map — Wakaru renames only where the code gives evidence. Pair with
  an LLM renamer or a source map if names matter.

## Safety

Only decompile code you are authorized to analyze. Reading what a third party
ships is a legitimate security-research and debugging activity; using it
against targets without consent may be illegal. You are responsible for
compliance.
