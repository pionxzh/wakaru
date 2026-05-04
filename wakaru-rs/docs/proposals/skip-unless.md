# Proposal: Source-text pre-check for rule skipping (`skip_unless!`)

Status: **Deferred** — documented for future consideration when the pipeline is
stable and cross-module helper detection is settled.

## Problem

In the multi-module unpack path, thousands of modules each run through ~50
rules. Many rules target specific transpiler patterns (Babel helpers, webpack
interop, TS downlevel) that are absent from most modules. Each rule still
performs a full AST walk even when its target pattern cannot possibly exist in
the module.

## Idea

Before running a guarded rule, do a cheap `source.contains(needle)` check on
the original module source text. If the needle string is absent, skip the rule
entirely — no AST walk needed.

```rust
macro_rules! skip_unless {
    ($rule:expr, $name:expr, $($needle:expr),+) => {
        if started {
            if $(source_has(source, $needle))||+ {
                module.visit_mut_with(&mut $rule);
            }
            // observer + stop_after handling omitted for brevity
        }
    };
}

fn source_has(source: Option<&str>, needle: &str) -> bool {
    source.is_none_or(|s| s.contains(needle))
}
```

When `source` is `None`, the guard is transparent — rules always run. The
optimization only activates in the multi-module path where the original module
source text is available.

## Needle inventory

Each needle was chosen by reading the rule's visitor implementation. The needle
must appear in any source text that contains a pattern the rule can transform.

| Rule | Needle(s) | Rationale |
|------|-----------|-----------|
| UnInteropRequireDefault | `__esModule` | Interop check: `e && e.__esModule` |
| UnInteropRequireWildcard | `__esModule` | Same interop check in wildcard form |
| UnToConsumableArray | `Array.isArray` | Body-shape marker in Babel helper |
| UnObjectSpread | `Object.assign`, `getOwnPropertyDescriptor` | Extends uses Object.assign; objectSpread2 uses getOwnPropertyDescriptors |
| UnSlicedToArray | `Symbol` | Body contains `Symbol.iterator` |
| UnDefineProperty | `defineProperty` | Body contains `Object.defineProperty(...)` |
| UnClassCallCheck | `instanceof` | Body contains `instanceof` operator |
| UnPossibleConstructorReturn | `ReferenceError` | Body contains `throw new ReferenceError(...)` |
| UnTypeofPolyfill | `Symbol` | Body checks `typeof Symbol` |
| UnEsmoduleFlag | `__esModule` | All three patterns reference `__esModule` |
| UnUseStrict | `use strict` | The literal directive string |
| UnWebpackInterop | `__esModule` | Getter pattern checks `base.__esModule` |
| UnClassFields | `__init` | Matches `__init*` method names |
| UnTsHelpers | `__awaiter`, `__generator`, `__assign`, `__rest`, `__extends`, `__importDefault`, `__importStar` | Canonical TS helper names |
| UnAsyncAwait | `__generator`, `__awaiter` | Pattern wraps in these helpers |
| UnPrototypeClass | `prototype` | Matches `Foo.prototype.method = ...` (very common, rarely skipped) |
| UnWebpackObjectGetters | `defineProperties` | Calls `Object.defineProperties(...)` |

## Key invariant

Needles are checked against the **original source text**, not the current AST.
This is safe because rules only *remove* transpiler patterns — they never
synthesize new ones. If a future rule introduces code containing a guarded
needle, the guard would incorrectly skip that rule.

## Source range

The source text is `&unpacked.code` — the code of the **current split module**:
- Webpack factories: raw extracted factory body
- Esbuild factories: raw extracted factory body
- Esbuild scope-hoisted modules: re-emitted code (`emit_items()`)

Helper detection (`collect_helpers`) only scans the current module. A helper
defined in module A and called in module B would not be detected in module B,
so skip_unless correctly skips the rule there too (it would be a no-op anyway).

## Benchmark results

Tested on a large esbuild bundle (thousands of scope-hoisted modules):

| Metric | With skip_unless | Without | Delta |
|--------|-----------------|---------|-------|
| Wall time | ~6.9s | ~7.7s | **-10%** |

On webpack bundles (hundreds of modules) the difference is in the noise because
most modules contain the guarded patterns.

## Why deferred

1. **Maintenance burden**: Two macros (`run!` and `skip_unless!`) with
   duplicated range-tracking logic. Every pipeline change must update both.

2. **Mental overhead**: Each guarded rule requires reasoning about what strings
   must appear in the source for the rule to match. Wrong needles silently
   break transformations.

3. **Cross-module helper detection**: If we later support detecting helpers in
   one module and transforming calls in another, the skip_unless model breaks —
   the calling module's source won't contain the helper body markers.

4. **Modest impact**: ~10% on the largest fixture. Other optimizations (parse
   once, parallel writes, scope-hoisted extraction) provide larger wins without
   the invariant complexity.

## When to reconsider

- Pipeline is stable and rule set is frozen
- Cross-module helper detection design is finalized
- Profiling shows rule dispatch is a bottleneck on real-world inputs
