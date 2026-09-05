# Architecture

Read this first for the shared pipeline, component responsibilities, and
binding rules. Then use [README.md](README.md) to select task-specific docs;
format details live in [unpacking.md](unpacking.md), and the full source
lookup is in [code-map.md](code-map.md).

## What wakaru does

Takes minified or bundled JavaScript and produces readable, modern JavaScript.

Two main operations:

1. **Decompile** — apply transformation rules to a single JS file
2. **Unpack + decompile** — split one or more bundle/chunk inputs into modules,
   then decompile each

## High-level flow

```
                        ┌─────────────┐
                        │  input.js   │
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │   Unpacker  │  detects bundle format,
                        │  (optional) │  extracts module code
                        └──────┬──────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
         module_0.js      module_1.js      entry.js
              │                │                │
              ▼                ▼                ▼
        ┌───────────────────────────────────────────┐
        │              Decompile pipeline            │
        │                                            │
        │  parse → resolver → rules → fixer → emit  │
        │                                            │
        │  (parallel via rayon when unpacking)        │
        └───────────────────────────────────────────┘
              │                │                │
              ▼                ▼                ▼
         readable JS      readable JS      readable JS
```

## Components

| Crate | Responsibility |
|---|---|
| `crates/wakaru` | Published Rust façade: owned `Source` inputs, `decompile`, `unpack`, incremental `UnpackJob`, artifacts, and diagnostics |
| `crates/core` | Internal detection, AST processing, rewrite pipeline, module facts, and output validation |
| `crates/cli` | Command-line input discovery, options, filesystem output, and debug commands |
| `crates/formatter` | Optional formatting of emitted code |
| `crates/wasm` | WebAssembly bindings and result serialization |

`wakaru-core` is published in lockstep and exact-version pinned by the façade
because Cargo requires packaged dependencies in the registry. Its driver and
AST types are not a supported semver contract. See [public-api.md](public-api.md)
for integration boundaries.

### Bun single-file executable containers (`crates/wakaru/src/bun.rs`)

Bun single-file PE, Mach-O, and ELF executables contain a serialized module
graph around compiled outputs. The parser reads it without executing the input.
Normal `--unpack` hands JavaScript records to `UnpackJob`, then the esbuild/Bun
detector recovers inner modules; `bun extract` stops at byte-for-byte container
extraction, including non-JavaScript assets. Binary layouts, opaque internals,
and AST memory ownership belong in [bun-standalone.md](bun-standalone.md).

### Unpackers (`crates/core/src/unpacker/`)

Detection is ordered, with the first match winning. Detectors extract modules
and perform bundler-specific normalization: factory parameter names, module-ID
rewrites, dependency maps, and runtime wrapper removal. They do not run the
normal rewrite pipeline. Outputs carry source text and may carry a private
prepared AST; both enter the same driver boundary.

Unmatched bundles can fall back to heuristic scope-hoisted splitting. Normal
output merges cyclic synthetic clusters to protect initialization order;
`--unpack=inspect` retains finer boundaries for static inspection and may not
execute. `--unpack=strict` disables heuristic fallback. Unsupported shapes must
preserve the appropriate original boundary rather than invent module edges.

Wakaru targets shipped production bundles. Dev-only recovery, such as unwrapping
webpack's eval-string module bodies, is a non-goal. Detection order, individual
format limits, partial-failure policy, and Inspect clustering rules are in
[unpacking.md](unpacking.md).

### Driver (`crates/core/src/driver.rs`, `crates/core/src/driver/`)

The driver owns parsing/resolution, rule execution, the cross-module barrier,
and emission. `UnpackJob` adds incremental intake: each push detects once,
retains a compatible AST for processing, and releases skipped inputs. Prepared
ASTs avoid an emit/parse round trip; raw and source-map modes may materialize
text when their contracts require it.

Single-file decompile:

```
parse_js(source)
  → resolver(unresolved_mark, top_level_mark)
  → apply_rules(module, unresolved_mark, RulePipelineOptions)
  → [optional: source map rename pipeline]
  → fixer()
  → print_js(module)
```

Normal unpack combines detected modules before the two-phase pipeline below,
so facts can see providers across physical inputs. Raw unpack stops after
extraction and bundler-coupled normalization: it retains ESM markers and export
getters for later recovery and has no normal-output graph-quality contract.
Multi-input naming, directory intake, and raw adapters are described in
[unpacking.md](unpacking.md#driver-intake-and-raw-output).

Single-file tracing records per-rule before/after snapshots and formats them as
unified diffs. See [debugging.md](debugging.md) for tracing and output validation;
a single-file trace does not reproduce the unpack fact barrier.

### Rules pipeline (`crates/core/src/rules/`)

Transformation rules implement SWC's `VisitMut` trait and run in a fixed order
through `apply_rules()`. Several rules repeat after later stages expose new
patterns. `crates/core/src/rules/pipeline.rs` stores the ordered
`RuleDescriptor` entries, `RuleStage` metadata, and explicit dependencies.
`RulePipelineOptions` controls ranges, rewrite level, dead-code cleanup, and
optional module facts.

#### Pipeline stages

**The registry is the authoritative roster** — this doc describes only what
each stage is for, with a few representative rules. Do not treat the examples
as exhaustive.

```
Stage 1: Syntax normalization
  small, local de-minification rewrites
  e.g. UnminifyBooleans (!0 → true), SimplifySequence, UnBracketNotation

Stage 2: Transpiler helper unwrapping + module-system reconstruction
  Babel/TypeScript helper recovery and require() → ESM, ending with UnEsm
  e.g. UnInteropRequireDefault, UnObjectSpread, UnWebpackInterop, UnEsm

  ── cross-module barrier (unpack only: fact collection + late pass) ──

Stage 3: Structural restoration
  expression/operator-level syntax recovery
  e.g. UnTemplateLiteral, UnNullishCoalescing, UnOptionalChaining

Stage 4: Complex pattern restoration
  multi-statement/control-flow pattern recovery
  e.g. UnIife, UnParameters, UnJsx, UnEs6Class, UnRegenerator, UnAsyncAwait

Stage 5: Modernization
  idiomatic-ESNext rewrites of already-correct code
  e.g. VarDeclToLetConst, ArrowFunction, ObjShorthand, UnForOf

Stage 6: Cleanup and renaming
  inlining, rename recovery, import/export cleanup, optional DCE
  e.g. SmartInline, SmartRename, ImportDedup, DeadDecls/DeadImports (optional),
  plus most repeat passes (UnIife2, UnJsx2, SmartRename2, ...)
```

`DeadImports` and `DeadDecls` are an optional late cleanup phase controlled by
`DecompileOptions.dce_mode`. CLI output uses transform-only cleanup by default,
preserving dead code that was already dead in the input while removing
transform-induced leftovers. Transform-only cleanup also retains original ESM
import specifiers: even an otherwise-unused default or named import performs an
observable link-time export check. It removes only dead import specifiers that
the rewrite pipeline synthesized. `--dce` opts into a full reachability sweep.
Tests and API callers can set `DceMode::Off` to snapshot structural restoration
separately from dead-code cleanup.

Unpacked bundle modules are the complementary case: their ESM imports are
Wakaru's recovered representation of bundle edges, not source-level link
checks. Transform-only cleanup may therefore remove a recovered specifier when
a later rewrite removes its last use, while retaining the side-effect import.
The unpack driver snapshots specifiers that were already dead at the phase-2
barrier and runs a final recovered-import cleanup after the targeted late
rewrites, so only specifiers made dead by those rewrites are removed.

`DecompileOptions.level` controls rewrite aggressiveness — `minimal` (high
confidence, semantics-preserving), `standard` (default, readability-oriented),
or `aggressive` (speculative recovery). Rules gate risky subpatterns inside the
rule rather than moving entire rules in or out of the pipeline.

See [Rewrite assumptions](rewrite-assumptions.md) for the semantic contract:
which named assumptions each level may rely on, and the reproduce-first policy
for new heuristics.

See [same-module recovery boundaries](fact-system.md#same-module-recovery-boundary)
for CommonJS read/self-import proofs, and [detector-owned facts](unpacking.md#detector-payload-and-runtime-facts)
for the handoff to normal-only runtime restoration.

#### Key design pattern: `unresolved_mark`

After `resolver()` runs, binding identifiers and references carry a
`SyntaxContext`. There are three separate responsibilities:

| Operation | Required check |
|---|---|
| Recognize a known global, such as `Object` or `require` | Match its name and require `unresolved_mark` |
| Follow a local helper, factory parameter, or alias | Match its resolved binding ID `(sym, ctxt)` |
| Insert or rename a binding | Check emitted-name collisions and capture at affected use sites, as well as binding identity |

For global recognition, the visitor takes `unresolved_mark: Mark` and guards
the name match:

```rust
// Guard: only match free-variable references, skip bound inner-scope identifiers
if id.ctxt.outer() != self.unresolved_mark {
    return;
}
```

This rejects a local variable named `Object`. It does not prove that an
unresolved `Object` has unmodified intrinsic behavior; that is governed by the
execution-environment baseline in [rewrite-assumptions.md](rewrite-assumptions.md).

A local helper or factory parameter has a bound context, so requiring
`unresolved_mark` would reject the binding you intended to match. Capture its
resolved ID and compare references to that ID. An inner parameter with the same
spelling has a different ID.

Use `rename_utils::BindingRenamer`, through `rename_bindings_in_module` or
`rename_bindings`, to apply binding renames. Never rename by `sym` alone.
The caller must choose a safe replacement name: different contexts do not make
two identical spellings safe after emission. For example, inserting an import
named `UIBase` can capture an existing global `UIBase` assignment even though
their pre-emission contexts differ.

Inspect existing utilities before writing a collector: `rename_utils.rs`
contains binding renaming and name/shadowing analysis, `analysis/binding_uses.rs`
contains shared binding-use and write analysis, and `js_names.rs` contains
identifier validation. Check each utility's coverage against the operation:
an expression-reference collector is not a complete inventory of assignment
targets, JSX names, or declarations. Binding identity, name availability, and
runtime assumptions are separate proofs.

The experimental Vue recovery path uses resolver-aware matching but still
has string-based expression processing in its IR. That is subsystem debt,
not a precedent for new AST rules. See [vue-decompile.md](vue-decompile.md)
and [the AST IR proposal](proposals/vue-recovery-ast-ir.md).

### Source map pipeline (`crates/core/src/sourcemap_rename.rs`)

Optional enhancement when `--sourcemap` is provided. Runs **after** the rules pipeline for two reasons:

1. Rules detect patterns by minified names (`require`, `__generator`, `__esModule`). Renaming first would break pattern detection.
2. `ImportDedup` needs `UnEsm` to run first (converting `require()` → `import`), and must merge duplicates before rename so we rename one binding instead of five.

```
ImportDedup           → merge repeated imports from same module request
apply_sourcemap_renames → recover original names via position lookup
UnImportRename        → clean up import aliases
```

Name recovery works by:

1. For each identifier at generated position `(line, col)`
2. Look up original position via source map mappings
3. Read the identifier at that position from `sourcesContent`
4. Vote on the best original name per binding (majority wins)

This works even when the `names` array is empty (common in esbuild output).

## Multi-module pipeline (`crates/core/src/driver/unpack/`)

When unpacking bundles, the driver runs a two-phase pipeline:

1. **Phase 1 (parallel):** Obtain a resolved module AST. Source-only detector
   output is parsed and resolved here; webpack5 can hand off its already
   resolved, bundler-normalized AST directly. Apply exact normal-only rewrites
   backed by detector-owned runtime facts, then run the rule registry through
   `UnEsm`, clone that barrier AST for webpack factory-IIFE fact recovery, and
   extract import/export facts. Retain the pre-recovery AST together with its
   `Globals` and unresolved mark.
2. **Phase 2 (parallel):** Resume the retained Phase 1 AST → cross-module late
   pass (exact CommonJS default-object composition, re-export consolidation,
   namespace decomposition, fact-aware helper recovery) → run the registry
   range resuming after `UnEsm`, through `UnReturn` → targeted late
   cleanup/recovery → emit.

The late pass uses facts from Phase 1 to inform cross-module rewrites (e.g.,
repairing a proven CommonJS object or callable-property edge, preserving one
mutable object across an exact ordered CommonJS composition chain, converting
`ns.foo` to `import { foo }`, or recognizing a split helper module). Facts are
extracted in `crates/core/src/facts.rs` and consumed by
`crates/core/src/commonjs_default_object_composition.rs`,
`crates/core/src/provider_import_repair.rs`,
`crates/core/src/namespace_decomposition.rs`,
`crates/core/src/reexport_consolidation.rs`, and fact-aware rules. See
[fact-system.md](fact-system.md) for details.

Normal no-source-map unpack runs the through-`UnEsm` range once and carries the
same `Globals`/`SyntaxContext` lineage across the barrier. If Phase 1 cannot
prepare an AST, Phase 2 retains the best-effort parser fallback. Output
source-map mode also deliberately materializes prepared detector ASTs and uses
the parser path because its mappings depend on parser-owned per-module source
coordinates.

The internal detector handoff is a single aligned payload boundary rather than
a format branch in either phase: each module has source text and may also have a
private prepared AST sidecar. Public/raw unpack APIs materialize every sidecar
back into `UnpackedModule::code`; the normal driver consumes it once at the
Phase 1 boundary. Aggressive nested scope splitting likewise materializes first
because that pass operates on emitted module text.

## File structure

Use [code-map.md](code-map.md) for the full source lookup. Common starting points:

| Work | Entry point |
|---|---|
| Rule ordering and execution | `crates/core/src/rules/pipeline.rs` |
| Binding reads, writes, and renaming | `crates/core/src/analysis/binding_uses.rs`, `crates/core/src/rules/rename_utils.rs` |
| Helper recognition and lifecycle | `crates/core/src/rules/transpiler_helper_utils/`, `match_context.rs`, `helper_matcher.rs`; see [helper-detection.md](helper-detection.md) |
| Detector dispatch and extraction | `crates/core/src/unpacker/mod.rs`; see [unpacking.md](unpacking.md) |
| Cross-module proofs and consumers | `crates/core/src/facts.rs`; see [fact-system.md](fact-system.md) |
| Test helpers and verification | `crates/core/tests/common/mod.rs`; see [testing.md](testing.md) |
| Public operations and types | `crates/wakaru/src/lib.rs`; see [public-api.md](public-api.md) |

## Related docs

- [Unpacking](unpacking.md) -- detector shapes, extraction, and fallback boundaries
- [Code map](code-map.md) -- source entry points by subsystem
- [Testing](testing.md) -- test patterns, helpers, and organization
- [Debugging](debugging.md) -- rule tracing, snapshot debugging, fixture workflow
- [Helper detection](helper-detection.md) -- transpiler helper detection design
- [Fact system](fact-system.md) -- cross-module fact system
- [Rule dependency inventory](rule-dependency-inventory.md) -- rule dependency relationships and experimental validation
- [Rewrite assumptions](rewrite-assumptions.md) -- semantic assumptions and rewrite policy
- [Vue decompile](vue-decompile.md) -- no-sourcemap Vue render recovery and SFC printer scope

## References

- [SWC Architecture](https://github.com/swc-project/swc/blob/main/ARCHITECTURE.md)
- [SWC Rustdoc](https://rustdoc.swc.rs/swc/)
