# CLI Reference

Full reference for `@wakaru/cli`. For a quick start, see the
[README](../README.md).

## Decompile a single file

```bash
wakaru input.js -o output.js
```

Without `-o`, output goes to stdout. Stdin is also supported:

```bash
cat input.js | wakaru > output.js
```

A `.cjs` or `.cts` input is decompiled as CommonJS: its `require()` calls and
`module.exports` are left alone instead of being recovered as ESM syntax,
because Node loads such a file as CommonJS however it is written. Every other
input name still recovers ESM. `--unpack` does not apply this yet: a `.cjs`
input file, or a recovered module that keeps a `.cjs` name, can still be
written with ESM syntax that Node cannot load.

## Unpack bundles and chunks

```bash
wakaru bundle.js --unpack -o out/
wakaru bundle.js --unpack --raw -o out/       # raw split, no readability transforms
wakaru bundle.js --unpack=strict -o out/      # structural detection only, no heuristic fallback
wakaru bundle.js --unpack=inspect -o out/     # finer boundaries for static inspection
wakaru entry.js chunk.js --unpack -o out/     # unpack multiple explicit files
wakaru dist/ --unpack -o out/                 # recursively scan a directory
wakaru ./compiled-app --unpack -o out/        # extract a Bun single-file executable
```

Directory inputs are supported only with `--unpack`. Wakaru recursively scans
`.js`, `.mjs`, and `.cjs` files, skips hidden files/directories and
`node_modules`, and includes only files detected as bundles or chunks. Skipped
files are not copied or decompiled. Explicit file inputs keep the normal
fallback behavior when no bundle format is detected.

An explicit PE, Mach-O, or ELF file can be a Bun single-file executable. Wakaru
validates Bun's embedded module graph, selects its JS/JSX/TS/TSX entries, and
then sends those entries through the same unpack pipeline as ordinary bundle
files. It never runs the executable. This `--unpack` path does not write binary
assets; use `wakaru bun extract` when every embedded file is needed. Bun's
internal source-map blobs are not accepted as v3 input maps. Executable
discovery is limited to explicit file paths: directory scans still consider
only `.js`, `.mjs`, and `.cjs` candidates, and stdin remains text input. Use
`--raw` to preserve the exact embedded JavaScript before readability rewrites.
See [bun-standalone.md](bun-standalone.md) for the container format, safety
properties, and current limits.

Unpacked output follows Node's rules for ESM and CommonJS, except for the
cases listed in [Known gaps](unpacking.md#known-gaps). A module that stays
CommonJS is usually named `.js`, and an ESM module imports it with a default
import, which Node resolves to `module.exports`. A bundler that rebuilds the
output reads that import by the `__esModule` convention instead: if the
CommonJS module sets `__esModule`, the import reads its `exports.default`,
usually `undefined`. To rebuild, rename every file without module syntax
(`import`, `export`, `import.meta`, or top-level `await`) to `.cjs`, scripts
included, update the imports and kept `require()` calls that name them, and
add `{"type": "module"}` at the output root.
[Running the output](unpacking.md#running-the-output) explains why.

## Module provenance

```bash
wakaru bundle.js --unpack --provenance -o out/
```

`--provenance` writes `provenance.json` alongside the recovered modules.
Each entry maps an emitted filename to its `input`, extraction `ranges`, and
`extraction` strategy. Ranges are zero-based byte offsets with an exclusive
end, `[start, end)`. They identify the input regions used to recover a module,
not a position-by-position mapping of the rewritten code. Use
`--emit-source-map` for output position mappings.

## Extract every file from a Bun single-file executable

```bash
wakaru bun extract ./compiled-app -o extracted/
wakaru bun extract ./compiled-app -o extracted/ --json
wakaru bun extract ./compiled-app -o extracted/ --include-internals
```

This is a byte-exact container operation, separate from `--unpack`. It writes
every validated Bun file record below `extracted/files/`, including JavaScript,
CSS, file-loader assets, WebAssembly, native add-ons, embedded SQLite databases,
and records using loader IDs unknown to this Wakaru version. It does not parse,
format, decompile, or execute their contents. Bun 1.3.3+ is supported.

`extracted/manifest.json` records each original Bun path, safe output path,
loader ID and known name, encoding, module format, server/client side, entry
status, byte length, and executable byte range. `--json` also prints that
manifest to stdout. Unsafe path components and non-UTF-8 path bytes are
percent-encoded, and case-insensitive filename collisions receive stable
numeric suffixes. Wakaru never writes outside the selected output directory.

`--include-internals` additionally writes Bun's associated opaque source-map,
JavaScriptCore bytecode, and module-info regions below `extracted/internals/`.
These are runtime data, not ordinary project assets. In particular,
`source-map.bunmap` is not a v3 JSON source map.

As with other directory-producing commands, Wakaru requires an empty or new
output directory unless `--force` is passed.

Structural unpacking supports webpack 4/5 (including Vercel ncc CommonJS output
with an IIFE webpack bootstrap), Turbopack production chunks from Next.js 15.3
and later, Browserify, Metro, Closure ModuleManager, SystemJS, esbuild/Bun
helper-based bundles, and AMD/UMD wrappers. Scope-hoisted
Rollup/Vite-style output is handled by the default heuristic fallback. For
supported ncc output, Wakaru extracts the webpack module table and preserves
its inline startup as `entry.js`; separately emitted asset files remain
external to the recovered JavaScript modules. ncc `.mjs` output uses a
top-level runtime and is not structurally split.

Webpack string module IDs keep their safe relative resource path, but emitted
JavaScript never keeps a misleading non-JavaScript filename. IDs ending in
`.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.tsx`, `.mts`, or `.cts` retain that
extension; every other or extensionless resource appends `.js` (for example,
`src/style.less` becomes `src/style.less.js`). Loader queries and URL fragments
are removed from filesystem names, and modules that then collide receive
stable numeric suffixes before consumer references are synthesized.

In normal multi-input unpack, a heuristic scope-hoisted ESM input or structural
esbuild ESM chunk keeps its original safe relative input path as its public
entry filename. Generated children are namespaced beneath that filename's stem
(for example, `assets/index-<hash>.js` with children below
`assets/index-<hash>/`). This preserves sibling static imports, re-exports,
namespace imports, and dynamic imports that still address the physical input.
Plain passthrough inputs keep their relative directory structure too, so
same-named files from different directories coexist and sibling imports
between inputs stay resolvable. Parent-relative invocations (`wakaru --unpack
../dist/*.js`) drop the traversal prefix and keep the in-bounds remainder.
Generated modules yield to these reserved physical-input paths during collision
deduplication. Two physical ESM identities that normalize to the same path
(including a plain input colliding with a facade) fail as ambiguous instead of
silently suffixing either identity. Script-loaded bundle inputs do not reserve
their physical filenames. `--raw` only skips readability transforms, retains
provisional extraction names, and does not apply public-path reservations or
promise a usable module graph.

`--unpack=inspect` recursively retains fine-grained scope-hoist boundaries,
including synthetic clusters whose emitted ESM imports form a cycle. The
resulting module graph may not preserve the bundle's initialization order and
must not be treated as executable reconstruction; the CLI always prints a
warning for this mode. `--raw` remains independent: without it Wakaru still
runs the normal readability pipeline over each inspection module. Normal
`--unpack` continues to merge cyclic components before emission.

With `--provenance`, fine modules split from the same oversized write component
also receive identical optional `context_ranges`. These ranges identify a
coarse evidence-pooling context in the original input; they do not assert that
the children came from one source package. Normal output and ambiguous
synthetic entries omit the field.

For ordinary Browserify numeric module tables, unambiguous dependency-map
request paths become readable output filenames. Missing or conflicting hints
retain the stable `module-<id>.js` fallback; entry names remain stable and
case-insensitive path collisions receive numeric suffixes.

Cocos Creator 2.x project-script bundles using `window.__require` are handled
as Browserify-family output. String-keyed factories are emitted as named
modules, local dependency-map targets are rewritten to those filenames, and
`cc._RF.push/pop` registration calls are preserved, including when production
compression combines them into comma sequences. Dependencies delegated to
another previously loaded Cocos bundle remain unresolved in single-file mode.

Metro plain JavaScript bundles are split from their `__d(...)` module table and
`__r(...)` startup calls, including prefixed definition globals, minified
factory parameters, and dynamic dependency-map metadata. Indexed/file RAM
bundles and Hermes bytecode are not JavaScript AST inputs and remain out of
scope.

Closure Library ModuleManager responses are split at guarded module segments,
using `/*_M:id*/` annotations and `_ModuleManager_initialize(...)` metadata to
validate identities and served order. The outputs remain shared-namespace
fragments: Wakaru preserves the shared top-level/wrapper bootstrap and loader
calls but does not fabricate ESM imports from ModuleManager dependency edges.
If an unguarded statement cannot be placed without guessing, structural
detection leaves the response unsplit; explicit-file fallback preserves the
input intact.

## Formatter

```bash
wakaru input.js --formatter -o output.js
wakaru bundle.js --unpack --formatter -o out/
```

`--formatter` runs a final formatting pass after decompilation. Off by default.

With `--emit-source-map`, each map is rewritten to describe the formatted
output. Mappings at code the formatter removed (such as dropped parentheses)
are dropped. If a map cannot be carried across formatting, that file is
written unformatted with its original map, and Wakaru prints a warning.

## Source maps

```bash
wakaru input.js --source-map input.js.map -o output.js
wakaru input.js --emit-source-map -o output.js    # emit output .map alongside decompiled file
```

Input source maps enable identifier recovery and import deduplication for
single-file decompilation. They are rejected with `--unpack`: extracted modules
have new generated coordinates, so applying the bundle-level map could assign
incorrect or duplicate binding names.

`--emit-source-map` writes a `.map` file alongside each decompiled JavaScript
output file, mapping the output back to the input. Vue SFC sidecars from
`--vue-sfc` do not get source maps. Unlike input `--source-map`, this option is
supported with `--unpack`.

With `--unpack`, each module's map points into the bundle it was extracted
from. Code the unpacker or the rules synthesized (imports, exports, runtime
glue) has no input position and stays unmapped. Unpack maps do not embed the
bundle in `sourcesContent`, because every module's map would repeat it;
single-file maps embed the input. Columns count UTF-16 code units, as the
source map format requires. Each map names its input by a path relative to
the map file, which is how source map consumers resolve `sources`; an input
without a file on disk (stdin, a Bun executable member) keeps its name.

## Vue SFC recovery

```bash
wakaru input.js --vue-sfc -o output.js
wakaru input.js --vue-sfc -o App.vue
wakaru custom/target.min.mjs --vue-sfc -o out/renamed.mjs
wakaru bundle.js --unpack --vue-sfc -o out/
```

`--vue-sfc` is an experimental, best-effort Vue 3 render recovery path. In
single-file mode without `-o`, stdout remains decompiled JavaScript. Use an
output path to write recovered Vue files.

With `-o`, `.vue` paths are Vue-only: `-o App.vue` writes the recovered SFC and
errors if recovery fails. Other output paths are JavaScript-primary: Wakaru
writes normal decompiled JavaScript to the requested path and, when Vue
recovery succeeds, also writes a sibling `.vue` sidecar named from the input
filename. For example, `custom/target.min.mjs --vue-sfc -o out/renamed.mjs`
writes `out/renamed.mjs` and `out/target.min.vue`.

In unpack mode, `--vue-sfc` is additive: every module still gets JavaScript
output, and recovered Vue render modules also get sibling `.vue` artifacts.
See [vue-decompile.md](vue-decompile.md) for the supported recovery scope and
[vue-sfc-recovery-status.md](vue-sfc-recovery-status.md) for current gaps and
follow-up targets.

## Extract original sources

```bash
wakaru extract input.js.map -o src/
```

Writes files embedded in the source map's `sourcesContent` to disk.

## Rewrite level

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

The semantic contract behind the levels — which named assumptions each level
may rely on — is documented in
[rewrite-assumptions.md](rewrite-assumptions.md).

## JSON output

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

## Exit status and failed modules

Wakaru exits nonzero for a fatal error, such as invalid arguments or an I/O
failure, and when any warning is error-class (`is_error: true` in JSON). An
error-class warning does not stop the run: Wakaru still writes every artifact,
`provenance.json`, and the JSON object, then prints the affected modules to
stderr and exits nonzero. A module that reports `decompile_failed` keeps its
extracted code without readability rewrites.

In unpack JSON, `total` counts modules and `failed` counts distinct modules
with at least one error-class warning, of any kind. `failed > 0` therefore
always comes with a nonzero exit. When `failed` is below `total`, the other
modules are still usable; the exit status alone does not separate that partial
result from a run that produced nothing.

| Error-class (`is_error: true`) | Non-error |
|---|---|
| `raw_normalization_failed`, `fact_collection_failed`, `webpack_factory_recovery_failed`, `decompile_failed`, `duplicate_declaration`, `output_parse_recovered`, `output_parse_failed` | `input_parse_recovered`, `tdz_violation`, `import_cycle`, `cross_module_class_call`, `commonjs_export_unrecovered`, `runtime_residual` |

`duplicate_declaration` and the `output_parse_*` checks run only with
`--diagnostics`. Without it, Wakaru does not re-parse emitted modules, so a
zero exit does not prove that every output file parses.

## Machine-readable output compatibility

Within one major version, these outputs change only by addition:

- the `wakaru --version` line, `wakaru <version>`;
- the `--json` object on stdout, for single-file and unpack runs;
- `provenance.json`;
- the exit status rules in the previous section.

Addition means two things. Any object can gain a new field. A string field
that names a category can gain a new value: module `kind` and `status`,
warning `kind`, `safety`, `detected_formats` entries, and the provenance
`format`, `strategy`, and `extraction`. An existing field keeps its name,
type, and meaning, and is never removed. Optional fields such as
`source_filename` and `context_ranges` stay absent when they do not apply.

Consumers should ignore fields they do not know, and treat an unknown category
value as unknown rather than as a parse error. Removing or renaming a field,
or changing its type or meaning, needs a new major version.

Recovered module code is not covered: any release can change it. Pin an exact
version only when you need byte-identical module output. Human-readable output
on stderr is not covered either. `debug` subcommands follow
their own [compatibility policy](cli-debug.md#compatibility-policy).

## Diagnostics and profiling

```bash
wakaru input.js --diagnostics                  # post-transform diagnostic checks to stderr
wakaru input.js --profile trace.json           # Chrome trace (open with chrome://tracing)
wakaru input.js --profile trace.json --profile-rules  # include per-rule spans
```

With `--diagnostics`, emitted declaration checks report `var` conflicts with
`let`, `const`, classes, or imports, including `var` declarations hoisted through
nested blocks. These are error-class `duplicate_declaration` warnings and make
the command exit nonzero. Repeated `var` declarations and legal inner-scope
shadowing do not count as conflicts.

Unpack also reports a non-error `cross_module_class_call` warning, with or
without `--diagnostics`, when a module still invokes an imported constructor
with `.call`/`.apply` after the providing module was allowed to become a
class because that call was expected to become `super()`. The named module may
throw `Class constructor … cannot be invoked without 'new'` at runtime. Rerun
with `--level minimal`, which keeps every cross-module `.call`/`.apply` target
a function.

Decompile and unpack both report a non-error `commonjs_export_unrecovered`
warning, with or without `--diagnostics`, when an output module has ESM
`import` or `export` declarations but still reads or writes an export through
`exports` or `module.exports`. ESM has no `exports` binding, so each such access
throws a `ReferenceError` when it runs. The message lists the export names in
first-appearance order; `exports` or `module.exports` in the list means the
whole object is still used as a value. Output that stayed CommonJS (no `import`
or `export`) does not get this warning, and neither do a direct
`typeof exports` probe or other `module` members such as `module.hot`.

Unpack reports a non-error `runtime_residual` warning when a module keeps a
bundler runtime call that the split output does not define. Today these are
Turbopack runtime members without a module-graph meaning, such as chunk
loading (`__turbopack_context__.L(...)`), path resolution
(`__turbopack_context__.P(...)`), or the host `require`
(`__turbopack_context__.t`). The message names the first such member. The
rest of the module is recovered; that call would throw if executed.

## Overwrite protection

Wakaru refuses to overwrite existing files unless `--force` is passed.
