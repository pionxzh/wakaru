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

## Unpack bundles and chunks

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

## Formatter

```bash
wakaru input.js --formatter -o output.js
wakaru bundle.js --unpack --formatter -o out/
```

`--formatter` runs a final formatting pass after decompilation. Off by default.

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

## Vue SFC recovery

```bash
wakaru input.js --vue-sfc
wakaru input.js --vue-sfc -o App.vue
wakaru custom/target.min.mjs --vue-sfc -o out/renamed.mjs
wakaru bundle.js --unpack --vue-sfc -o out/
```

`--vue-sfc` is an experimental, best-effort Vue 3 render recovery path. In
single-file mode without `-o`, Wakaru prints a recovered `.vue` artifact when
recovery succeeds and normal decompiled JavaScript otherwise.

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

## Diagnostics and profiling

```bash
wakaru input.js --diagnostics                  # post-transform diagnostic checks to stderr
wakaru input.js --profile trace.json           # Chrome trace (open with chrome://tracing)
wakaru input.js --profile trace.json --profile-rules  # include per-rule spans
```

## Overwrite protection

Wakaru refuses to overwrite existing files unless `--force` is passed.
