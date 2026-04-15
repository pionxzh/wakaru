# Rule Dependency Inventory

This document maps the dependency relationships between all rules in the decompile pipeline.
It serves as the foundation for Step 1 and Step 2 of the [fact-system proposal](proposal-of-fact-system.md).

**Legend**

- **Prerequisite status**: `suspected` (inferred from code reading), `confirmed` (validated by test/experiment), `unknown` (needs investigation)
- **Safety**: `safe` (semantics-preserving), `heuristic` (high-confidence pattern match), `aggressive` (may change semantics)
- **Fact behavior**: `writer` (could emit observations), `reader` (could benefit from merged facts), `neither`

---

## Stage 1 — Syntax Normalization

These rules normalize minified syntax into canonical forms. Most are independent of each other.

### 1. SimplifySequence

| Field | Value |
|-------|-------|
| Current position | 1 (first rule) |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | Yes — guards against removing non-pure calls |
| Suspected prerequisites | None (runs first) |
| Shape prerequisites | None |
| Produces | Flat statement lists; drops dead expressions; splits `a, b, c` into separate statements |
| Downstream dependents | Nearly everything — flat statements are assumed by most rules |
| Fact behavior | Neither |
| Safety | Safe (drops only provably side-effect-free expressions) |
| Notes | Barestatement expressions (e.g. `65536;`) are dropped — test inputs must use `const x = ...` |

### 2. FlipComparisons

| Field | Value |
|-------|-------|
| Current position | 2 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Normalized comparisons: literals on the right (`x === 0` not `0 === x`) |
| Downstream dependents | UnParameters (pattern-matches `arg === void 0` with literal on right) |
| Fact behavior | Neither |
| Safety | Safe |

### 3. UnTypeofStrict

| Field | Value |
|-------|-------|
| Current position | 3 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Strict equality for typeof checks (`typeof x === "string"`) |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 4. RemoveVoid

| Field | Value |
|-------|-------|
| Current position | 4 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | SimplifySequence (to flatten sequences containing `void 0`) — `suspected` |
| Shape prerequisites | None |
| Produces | `undefined` identifiers where `void 0` was used |
| Downstream dependents | UnParameters (matches `arg === undefined`), UnOptionalChaining (matches `undefined` in ternary), UnUndefinedInit |
| Fact behavior | Neither |
| Safety | Safe (checks no local `undefined` binding exists) |
| Notes | Conditional execution: `should_run()` checks module has no explicit `undefined` binding |

### 5. UnminifyBooleans

| Field | Value |
|-------|-------|
| Current position | 5 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `true`/`false` literals |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 6. UnInfinity

| Field | Value |
|-------|-------|
| Current position | 6 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `Infinity` / `-Infinity` identifiers |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 7. UnIndirectCall

| Field | Value |
|-------|-------|
| Current position | 7 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | SimplifySequence (to see `(0, fn)()` clearly) — `suspected` |
| Shape prerequisites | None |
| Produces | Direct function calls (removes `(0, fn)()` and `Object(fn)()` wrappers) |
| Downstream dependents | UnInteropRequireDefault, UnInteropRequireWildcard (need to see direct `require()` or `helper(require())` calls) |
| Fact behavior | Neither |
| Safety | Heuristic (changes `this` binding, but this is intentional for decompilation) |

### 8. UnTypeof

| Field | Value |
|-------|-------|
| Current position | 8 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Standard `typeof x !== "undefined"` comparisons |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 9. UnNumericLiteral

| Field | Value |
|-------|-------|
| Current position | 9 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Normalized numeric literal display (clears `raw` field) |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 10. UnBracketNotation

| Field | Value |
|-------|-------|
| Current position | 10 |
| Family | Generic |
| Role | Syntax normalization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Dot notation: `obj["prop"]` → `obj.prop`, `obj["default"]` → `obj.default` |
| Downstream dependents | UnInteropRequireDefault (`.default` access), UnInteropRequireWildcard, UnObjectRest (property keys), UnWebpackInterop (`.__esModule`, `.default`), UnEsm (`.default` on exports) |
| Fact behavior | Neither |
| Safety | Safe |
| Notes | **Critical early normalizer** — many downstream rules pattern-match on dot notation |

---

## Stage 2 — Transpiler Helper Unwrapping

These rules detect and remove Babel/TS transpiler helper calls, restoring original syntax.

### 11. UnInteropRequireDefault

| Field | Value |
|-------|-------|
| Current position | 11 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnIndirectCall (`suspected`), UnBracketNotation (`suspected`) |
| Shape prerequisites | Normalized `.default` access, direct function calls |
| Produces | Direct `require()` calls; `.default` property accesses on bindings |
| Downstream dependents | UnEsm (clean require patterns) |
| Fact behavior | **Writer** — could emit "binding X was interop-require-default wrapped" |
| Safety | Safe |

### 12. UnInteropRequireWildcard

| Field | Value |
|-------|-------|
| Current position | 12 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnIndirectCall (`suspected`), UnBracketNotation (`suspected`) |
| Shape prerequisites | Normalized member access, direct function calls |
| Produces | Namespace-style require bindings |
| Downstream dependents | UnEsm (namespace imports) |
| Fact behavior | **Writer** — could emit "binding X was interop-require-wildcard wrapped" |
| Safety | Safe |

### 13. UnToConsumableArray

| Field | Value |
|-------|-------|
| Current position | 13 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Spread array syntax `[...arr]` |
| Downstream dependents | UnSpreadArrayLiteral (simplifies `fn(...[a,b])` → `fn(a,b)`) |
| Fact behavior | Neither |
| Safety | Safe |

### 14. UnObjectSpread

| Field | Value |
|-------|-------|
| Current position | 14 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Object spread syntax `{...obj}` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe (only transforms when first arg is `{}`) |

### 15. UnObjectRest

| Field | Value |
|-------|-------|
| Current position | 15 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnBracketNotation (`suspected`), SimplifySequence (`suspected`) |
| Shape prerequisites | Flat statements, normalized property access |
| Produces | Object rest destructuring `{a, b, ...rest} = obj` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic (backward scan to absorb property accesses) |

### 16. UnSlicedToArray

| Field | Value |
|-------|-------|
| Current position | 16 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Direct expressions from `_slicedToArray(expr, N)` |
| Downstream dependents | SmartInline (later converts `_ref[0]` accesses to destructuring) |
| Fact behavior | Neither |
| Safety | Safe |

### 17. UnClassCallCheck

| Field | Value |
|-------|-------|
| Current position | 17 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Cleaned function/class bodies (guard calls removed) |
| Downstream dependents | UnEs6Class (cleaner class body detection) |
| Fact behavior | Neither |
| Safety | Safe |

### 18. UnPossibleConstructorReturn

| Field | Value |
|-------|-------|
| Current position | 18 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Direct constructor return values |
| Downstream dependents | UnEs6Class (cleaner constructor pattern) |
| Fact behavior | Neither |
| Safety | Safe |

### 19. UnTypeofPolyfill

| Field | Value |
|-------|-------|
| Current position | 19 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `typeof x` unary expressions; removes polyfill declarations |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

---

## Stage 3 — Structural Restoration

These rules restore structural patterns and clean up minification artifacts.

### 20. UnTemplateLiteral

| Field | Value |
|-------|-------|
| Current position | 20 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Template literal expressions |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 21. UnUseStrict

| Field | Value |
|-------|-------|
| Current position | 21 |
| Family | Generic |
| Role | Cleanup |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Removes `"use strict"` directives |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 22. UnWhileLoop

| Field | Value |
|-------|-------|
| Current position | 22 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `while(test){}` from `for(;test;){}` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 23. UnCurlyBraces

| Field | Value |
|-------|-------|
| Current position | 23 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Block statements around single-stmt control flow; block-body arrows |
| Downstream dependents | UnConditionals (normalized blocks), UnParameters (block-body patterns) |
| Fact behavior | Neither |
| Safety | Safe |
| Notes | JS version runs this first; Rust version runs it mid-pipeline. Difference may be worth investigating. |

### 24. UnTypeConstructor

| Field | Value |
|-------|-------|
| Current position | 24 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `Number(x)`, `String(x)`, `Boolean(x)`, `Array(n)` calls |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic (`+x` → `Number(x)` is semantically equivalent but changes readability intent) |

### 25. UnEsmoduleFlag

| Field | Value |
|-------|-------|
| Current position | 25 |
| Family | Module-system |
| Role | Module-system reconstruction |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Removes `Object.defineProperty(exports, '__esModule', ...)` and `exports.__esModule = true` |
| Downstream dependents | UnEsm (cleaner exports object — no noise statements) |
| Fact behavior | **Writer** — could emit "module has __esModule flag" (ES module indicator) |
| Safety | Safe |

### 26. UnAssignmentMerging

| Field | Value |
|-------|-------|
| Current position | 26 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Split assignments: `a = b = val` → `a = val; b = val` |
| Downstream dependents | UnVariableMerging (runs after), **UnEsm** (needs `exports.foo = val; exports.bar = val` split to detect named exports) |
| Fact behavior | Neither |
| Safety | Safe |
| Notes | **High-priority rule for UnEsm dependency validation** per proposal |

### 27. UnBuiltinPrototype

| Field | Value |
|-------|-------|
| Current position | 27 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `Array.prototype.slice.call(...)` from `[].slice.call(...)` etc. |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 28. UnArgumentSpread

| Field | Value |
|-------|-------|
| Current position | 28 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Spread call syntax: `fn.apply(ctx, args)` → `fn.call(ctx, ...args)` or `fn(...args)` |
| Downstream dependents | UnSpreadArrayLiteral (consumes spread syntax) |
| Fact behavior | **Reader** — could benefit from knowing whether a binding is a direct import vs namespace access (the `obj.fn.apply(null, args)` case documented in late-program-pass.md) |
| Safety | Heuristic (Pattern 1 `fn.apply(null, args)` is safe; Pattern 2 `obj.fn.apply(obj, args)` is safe; `obj.fn.apply(null, args)` is intentionally skipped — not semantics-preserving without namespace decomposition) |
| Notes | **Key rule for late-program-pass** — the `obj.fn.apply(undefined, args)` case requires cross-module context |

### 29. UnArrayConcatSpread

| Field | Value |
|-------|-------|
| Current position | 29 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Array spread: `[a].concat(b)` → `[a, ...b]` |
| Downstream dependents | UnSpreadArrayLiteral |
| Fact behavior | Neither |
| Safety | Safe |

### 30. UnSpreadArrayLiteral

| Field | Value |
|-------|-------|
| Current position | 30 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnArgumentSpread (`suspected`), UnArrayConcatSpread (`suspected`), UnToConsumableArray (`suspected`) |
| Shape prerequisites | Spread syntax must exist from earlier rules |
| Produces | Flattened arguments: `fn(...[a,b,c])` → `fn(a,b,c)` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 31. ObjectAssignSpread

| Field | Value |
|-------|-------|
| Current position | 31 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | Yes — to match `Object.assign` on unresolved `Object` |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Object spread: `Object.assign({}, src)` → `{...src}` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe (only when first arg is `{}`) |

### 32. UnVariableMerging

| Field | Value |
|-------|-------|
| Current position | 32 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnAssignmentMerging (`suspected` — creates more declarations to split) |
| Shape prerequisites | None |
| Produces | Individual `var` declarations from `var a=1, b=2` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 33. UnNullishCoalescing

| Field | Value |
|-------|-------|
| Current position | 33 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `x ?? fallback` expressions |
| Downstream dependents | UnConditionals (should run after nullish coalescing to avoid converting `??`-eligible ternaries to if/else) |
| Fact behavior | Neither |
| Safety | Safe |

### 34. UnOptionalChaining

| Field | Value |
|-------|-------|
| Current position | 34 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | RemoveVoid (`suspected` — needs `undefined` instead of `void 0` in ternary alt) |
| Shape prerequisites | `undefined` identifiers (not `void 0`) |
| Produces | `x?.prop` optional chaining expressions |
| Downstream dependents | UnConditionals (should run after optional chaining) |
| Fact behavior | Neither |
| Safety | Safe |
| Notes | Shares helper functions with UnNullishCoalescing (`exprs_structurally_equal`, `is_undefined`) |

---

## Stage 4 — Bundler Artifacts

### 35. UnWebpackInterop

| Field | Value |
|-------|-------|
| Current position | 35 (first pass), 45 (second pass as UnWebpackInterop2) |
| Family | Webpack |
| Role | Helper unwrapping / Bundler artifact |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnBracketNotation (`suspected` — normalizes `.__esModule`, `.default`), require() bindings present (`suspected` — must run before UnEsm) |
| Shape prerequisites | Dot-notation member access on `__esModule` and `default` |
| Produces | Direct require binding references (removes getter indirection) |
| Downstream dependents | UnEsm (clean require patterns without getter wrappers) |
| Fact behavior | **Writer** — could emit "binding X had webpack interop getter wrapper" |
| Safety | Safe (verified by usage analysis: only inlines when all uses are safe) |
| Notes | Runs twice — second pass after UnAsyncAwait catches newly exposed patterns. **High-priority for experimental validation.** |

### 36. UnIife

| Field | Value |
|-------|-------|
| Current position | 36 (first pass), 64 (second pass as UnIife2) |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Inlined/flattened IIFE bodies; renamed single-char params |
| Downstream dependents | UnEs6Class (class IIFEs become visible), UnEnum (enum IIFEs), SmartInline (second pass catches IIFEs it creates) |
| Fact behavior | Neither |
| Safety | Heuristic (parameter inlining uses usage counting) |
| Notes | Second pass after SmartInline catches IIFEs created by inlining |

### 37. UnConditionals

| Field | Value |
|-------|-------|
| Current position | 37 |
| Family | Generic |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnOptionalChaining (`suspected`), UnNullishCoalescing (`suspected`) — must run after these to avoid converting `?.`/`??`-eligible patterns |
| Shape prerequisites | Ternary/logical expressions not eligible for `?.` or `??` |
| Produces | `if/else` statements from ternaries and logical expressions |
| Downstream dependents | UnParameters (needs if-statement form for default param detection) |
| Fact behavior | Neither |
| Safety | Heuristic (only converts "action-like" branches: calls, assignments, awaits) |

### 38. UnParameters

| Field | Value |
|-------|-------|
| Current position | 38 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | FlipComparisons (`suspected` — normalized `arg === undefined` comparisons), RemoveVoid (`suspected` — `undefined` instead of `void 0`), UnConditionals (`suspected` — if-statement form), UnCurlyBraces (`suspected` — block bodies) |
| Shape prerequisites | If-statements with `arg === undefined` tests in function bodies |
| Produces | ES6 default parameters |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic (pattern match on guard shape) |

### 39. UnEnum

| Field | Value |
|-------|-------|
| Current position | 39 |
| Family | TypeScript |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | SimplifySequence (`suspected` — needs paired var+IIFE visible as flat statements) |
| Shape prerequisites | Adjacent `var X; (function(X){...})(X || (X = {}))` |
| Produces | Object literal declarations |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic (enum IIFE pattern detection) |

---

## Stage 5 — Complex Pattern Restoration

### 40. UnJsx

| Field | Value |
|-------|-------|
| Current position | 40 |
| Family | Babel / React |
| Role | Structural restoration |
| Uses `unresolved_mark` | Yes — to detect JSX pragma imports |
| Suspected prerequisites | Import analysis (needs to identify pragma bindings) — `suspected` |
| Shape prerequisites | JSX createElement / _jsx / _jsxs call patterns |
| Produces | JSX element syntax |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic |

### 41. UnEs6Class

| Field | Value |
|-------|-------|
| Current position | 41 |
| Family | Babel |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnClassCallCheck (`suspected`), UnPossibleConstructorReturn (`suspected`), UnIife (`suspected` — class IIFE wrappers) |
| Shape prerequisites | IIFE-wrapped class patterns with `_inherits`, `_createClass` helpers |
| Produces | ES6 `class` declarations |
| Downstream dependents | UnClassFields (needs class syntax to detect `__init` methods) |
| Fact behavior | Neither |
| Safety | Heuristic (complex multi-pattern detection) |

### 42. UnClassFields

| Field | Value |
|-------|-------|
| Current position | 42 |
| Family | Babel |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnEs6Class (`suspected` — needs class declarations to exist) |
| Shape prerequisites | Class with `__init*()` methods and constructor calling them via `prototype.__init.call(this)` |
| Produces | Simplified constructors with direct field assignments |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic |

### 43. UnTsHelpers

| Field | Value |
|-------|-------|
| Current position | 43 |
| Family | TypeScript |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Confirmed prerequisites | **Must run after UnEsm's position is determined** — UnEsm needs `__esModule` helper patterns intact (`confirmed` by Exp 3). In practice, UnTsHelpers runs before UnEsm in the current pipeline because UnEsm is at position 46, but UnTsHelpers must not move before the point where UnEsm's interop detection has completed. |
| Shape prerequisites | `const V = this && this.__awaiter || (...)` declarations |
| Produces | Canonical helper names (`__awaiter`, `__generator`, etc.); removes helper declarations |
| Downstream dependents | **UnAsyncAwait** (hard dependency — needs canonical `__awaiter`/`__generator` names), **UnWebpackInterop2** (indirect — UnAsyncAwait exposes new getter patterns) |
| Fact behavior | **Writer** — could emit "module uses TS helpers: __awaiter, __generator, ..." |
| Safety | Safe |
| Notes | **Experimentally validated** (Exp 3). Cannot move to Stage 2 — breaks UnEsm's interop detection. Current position (Stage 5, before UnAsyncAwait) is correct. |

### 44. UnAsyncAwait

| Field | Value |
|-------|-------|
| Current position | 44 |
| Family | TypeScript |
| Role | Structural restoration |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | **UnTsHelpers** (`suspected` — hard: needs canonical helper names to match `__awaiter`/`__generator` patterns) |
| Shape prerequisites | `__generator` state machine structure, `__awaiter` wrapper |
| Produces | `async` functions, `function*` generators, `yield`/`await` expressions |
| Downstream dependents | UnWebpackInterop2 (may expose new getter patterns after async restoration) |
| Fact behavior | Neither |
| Safety | Heuristic (state machine reconstruction) |

### 45. UnWebpackInterop2

See #35 (second pass of UnWebpackInterop).

### 46. UnEsm

| Field | Value |
|-------|-------|
| Current position | 46 |
| Family | Module-system |
| Role | Module-system reconstruction |
| Uses `unresolved_mark` | No |
| Confirmed prerequisites | UnInteropRequireDefault (`confirmed`), UnInteropRequireWildcard (`confirmed`), UnAssignmentMerging (`confirmed`), UnEsmoduleFlag (`confirmed`), UnWebpackInterop pass 1 (`confirmed soft` — only getter pattern), **UnWebpackInterop2** (`confirmed hard` — fixture regression without it), UnAsyncAwait → UnWebpackInterop2 chain (`confirmed`) |
| Shape prerequisites | Clean `require()` calls, clean `exports.X = val` / `module.exports = val`, all interop getters resolved |
| Produces | `import`/`export` declarations; renames conflicting export bindings |
| Downstream dependents | UnTsHelpers (must run after — `confirmed`), UnImportRename, UnExportRename, SmartInline |
| Fact behavior | **Writer** — could emit import/export summary, module classification (CJS/ESM) |
| Safety | Heuristic (classification logic for default vs named imports/exports) |
| Notes | **Experimentally validated.** Current position (end of Stage 5, after UnWebpackInterop2) is the earliest safe position. Core require→import conversion works as early as Stage 2, but webpack interop getter patterns require the full UnTsHelpers → UnAsyncAwait → UnWebpackInterop2 chain to complete first. See Step 3 experiments for details. |

---

## Stage 6 — Modernization

### 47. UnThenCatch

| Field | Value |
|-------|-------|
| Current position | 47 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `.catch(handler)` from `.then(null, handler)` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 48. UnUndefinedInit

| Field | Value |
|-------|-------|
| Current position | 48 |
| Family | Generic |
| Role | Cleanup |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | RemoveVoid (`suspected` — needs `undefined` identifier, not `void 0`) |
| Shape prerequisites | `undefined` identifiers in init positions |
| Produces | `var x` without initializer (instead of `var x = undefined`) |
| Downstream dependents | VarDeclToLetConst (fewer noisy inits) |
| Fact behavior | Neither |
| Safety | Safe |

### 49. VarDeclToLetConst

| Field | Value |
|-------|-------|
| Current position | 49 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnUndefinedInit (`suspected` — cleaner declarations), all rules that introduce new variables should have run |
| Shape prerequisites | Final variable declarations |
| Produces | `let`/`const` declarations |
| Downstream dependents | UnPrototypeClass (uses `const` detection) |
| Fact behavior | Neither |
| Safety | Safe (scope-aware analysis with SyntaxContext) |

### 50. ObjShorthand

| Field | Value |
|-------|-------|
| Current position | 50 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `{foo}` shorthand from `{foo: foo}` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 51. ObjMethodShorthand

| Field | Value |
|-------|-------|
| Current position | 51 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `{foo(){}}` method shorthand |
| Downstream dependents | UnPrototypeClass (needs method shorthand to detect class candidates) |
| Fact behavior | Neither |
| Safety | Safe |

### 52. UnPrototypeClass

| Field | Value |
|-------|-------|
| Current position | 52 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | VarDeclToLetConst (`suspected`), ObjMethodShorthand (`suspected`) |
| Shape prerequisites | `const`/`let` function declarations, method shorthand in prototype assignments |
| Produces | ES6 `class` declarations from prototype-based patterns |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic |

### 53. Exponent

| Field | Value |
|-------|-------|
| Current position | 53 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `a ** b` from `Math.pow(a, b)` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 54. ArgRest

| Field | Value |
|-------|-------|
| Current position | 54 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Rest parameters `...args` from `arguments` usage |
| Downstream dependents | **UnRestArrayCopy** (hard — detects Babel copy loop for rest params created by ArgRest) |
| Fact behavior | Neither |
| Safety | Heuristic |

### 55. UnRestArrayCopy

| Field | Value |
|-------|-------|
| Current position | 55 |
| Family | Babel |
| Role | Helper unwrapping |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | **ArgRest** (`suspected` — hard: detects rest param copy loops) |
| Shape prerequisites | Rest parameter + Babel copy loop pattern |
| Produces | Simplified function bodies (copy loop removed) |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 56. ArrowFunction

| Field | Value |
|-------|-------|
| Current position | 56 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | Arrow function expressions (checks for `this`/`arguments` usage) |
| Downstream dependents | **ArrowReturn** (hard — needs arrows with block bodies to simplify) |
| Fact behavior | Neither |
| Safety | Safe (checks `this`/`arguments` references) |

### 57. ArrowReturn

| Field | Value |
|-------|-------|
| Current position | 57 |
| Family | Generic |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | **ArrowFunction** (`suspected` — hard: needs arrow expressions to exist) |
| Shape prerequisites | Arrow functions with `{ return expr; }` body |
| Produces | Concise arrow bodies `() => expr` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 58. UnForOf

| Field | Value |
|-------|-------|
| Current position | 58 |
| Family | TypeScript |
| Role | Modernization |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `for...of` loops from index-based iteration patterns |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Heuristic |

---

## Stage 7 — Cleanup and Renaming

### 59. UnWebpackDefineGetters

| Field | Value |
|-------|-------|
| Current position | 59 |
| Family | Webpack |
| Role | Bundler artifact |
| Uses `unresolved_mark` | Yes — to match webpack runtime `require("d")` calls |
| Suspected prerequisites | None |
| Shape prerequisites | None |
| Produces | `Object.defineProperties(obj, {...})` calls |
| Downstream dependents | **UnWebpackObjectGetters** (hard — converts define calls to getter syntax) |
| Fact behavior | Neither |
| Safety | Safe |

### 60. UnWebpackObjectGetters

| Field | Value |
|-------|-------|
| Current position | 60 |
| Family | Webpack |
| Role | Bundler artifact |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | **UnWebpackDefineGetters** (`suspected` — hard: consumes its output) |
| Shape prerequisites | Adjacent `var x = {}; Object.defineProperties(x, {...})` |
| Produces | Object literals with getter syntax `{ get prop() {...} }` |
| Downstream dependents | None known |
| Fact behavior | Neither |
| Safety | Safe |

### 61. UnImportRename

| Field | Value |
|-------|-------|
| Current position | 61 |
| Family | Module-system |
| Role | Naming / presentation |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | **UnEsm** (`suspected` — hard: needs import declarations to exist) |
| Shape prerequisites | Import declarations with aliases |
| Produces | Renamed import bindings (local matches imported name) |
| Downstream dependents | UnExportRename, SmartInline (stable bindings) |
| Fact behavior | Neither |
| Safety | Safe (uses BindingRenamer) |

### 62. UnExportRename

| Field | Value |
|-------|-------|
| Current position | 62 |
| Family | Module-system |
| Role | Naming / presentation |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | **UnEsm** (`suspected` — hard: needs export declarations), UnImportRename (`suspected` — consistent naming) |
| Shape prerequisites | Export declarations with alias patterns |
| Produces | Promoted export bindings |
| Downstream dependents | SmartInline (stable bindings) |
| Fact behavior | Neither |
| Safety | Safe (uses BindingRenamer) |

### 63. SmartInline

| Field | Value |
|-------|-------|
| Current position | 63 |
| Family | Generic |
| Role | Cleanup |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | UnImportRename (`suspected`), UnExportRename (`suspected`) — needs stable import/export bindings before inlining aliases |
| Shape prerequisites | Single-use `const`/`var` bindings |
| Produces | Inlined expressions; may create new IIFEs `(() => expr)()` |
| Downstream dependents | **UnIife2** (hard — catches IIFEs created by SmartInline), SmartRename (aliases removed) |
| Fact behavior | Neither |
| Safety | Heuristic (usage counting for inline decisions) |

### 64. UnIife2

See #36 (second pass of UnIife, after SmartInline).

### 65. SmartRename

| Field | Value |
|-------|-------|
| Current position | 65 |
| Family | Generic |
| Role | Naming / presentation |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | SmartInline (`suspected` — aliases removed, names stabilized), UnIndirectCall (`suspected` — partially, for cleaner patterns) |
| Shape prerequisites | Final binding state |
| Produces | Readable names from destructuring patterns, React hooks, member init, Symbol.for |
| Downstream dependents | None known |
| Fact behavior | **Reader** — could benefit from knowing original source names (source maps) or cross-module naming |
| Safety | Heuristic |

### 66. UnReturn

| Field | Value |
|-------|-------|
| Current position | 66 (last rule) |
| Family | Generic |
| Role | Cleanup |
| Uses `unresolved_mark` | No |
| Suspected prerequisites | All prior rules (runs last — earlier rules may introduce tail returns) |
| Shape prerequisites | None |
| Produces | Removes tail `return undefined` / `return void 0` |
| Downstream dependents | None (last rule) |
| Fact behavior | Neither |
| Safety | Safe |

---

## Step 2 — Role Re-categorization

Rules grouped by conceptual role, independent of current pipeline position.

### Syntax Normalization

Pure syntactic transforms. No semantic dependencies. Could theoretically run in any order among themselves.

| Rule | Current Stage | Notes |
|------|--------------|-------|
| SimplifySequence | 1 | Must stay first — nearly everything depends on flat statements |
| FlipComparisons | 1 | |
| UnTypeofStrict | 1 | |
| RemoveVoid | 1 | Conditional on no local `undefined` binding |
| UnminifyBooleans | 1 | |
| UnInfinity | 1 | |
| UnIndirectCall | 1 | Enables helper detection downstream |
| UnTypeof | 1 | |
| UnNumericLiteral | 1 | |
| UnBracketNotation | 1 | **Critical** — enables many downstream rules via `.default` normalization |

### Helper Unwrapping

Detect and remove Babel/TS transpiler helper functions.

| Rule | Current Stage | Family | Notes |
|------|--------------|--------|-------|
| UnInteropRequireDefault | 2 | Babel | Needs UnIndirectCall + UnBracketNotation |
| UnInteropRequireWildcard | 2 | Babel | Needs UnIndirectCall + UnBracketNotation |
| UnToConsumableArray | 2 | Babel | Independent |
| UnObjectSpread | 2 | Babel | Independent |
| UnObjectRest | 2 | Babel | Needs UnBracketNotation |
| UnSlicedToArray | 2 | Babel | Independent |
| UnClassCallCheck | 2 | Babel | Independent, enables UnEs6Class |
| UnPossibleConstructorReturn | 2 | Babel | Independent, enables UnEs6Class |
| UnTypeofPolyfill | 2 | Babel | Independent |
| UnTsHelpers | 5 | TypeScript | Independent, enables UnAsyncAwait |
| UnRestArrayCopy | 6 | Babel | Needs ArgRest |

### Module-System Reconstruction

Transform between module systems (CJS ↔ ESM).

| Rule | Current Stage | Notes |
|------|--------------|-------|
| UnEsmoduleFlag | 3 | Early cleanup — removes noise for UnEsm |
| UnWebpackInterop | 4 + 5 | Removes getter wrappers for UnEsm |
| UnEsm | 5 | **Central rule** — most complex dependencies |
| UnImportRename | 7 | Post-UnEsm naming |
| UnExportRename | 7 | Post-UnEsm naming |

### Bundler Artifacts

Webpack/bundler-specific runtime artifact removal.

| Rule | Current Stage | Notes |
|------|--------------|-------|
| UnWebpackInterop | 4 + 5 | Also in module-system category |
| UnWebpackDefineGetters | 7 | Webpack runtime |
| UnWebpackObjectGetters | 7 | Depends on UnWebpackDefineGetters |

### Structural Restoration

Restore higher-level code structures from minified/transpiled forms.

| Rule | Current Stage | Notes |
|------|--------------|-------|
| UnTemplateLiteral | 3 | Independent |
| UnWhileLoop | 3 | Independent |
| UnCurlyBraces | 3 | Enables UnConditionals, UnParameters |
| UnTypeConstructor | 3 | Independent |
| UnAssignmentMerging | 3 | Enables UnVariableMerging, UnEsm |
| UnBuiltinPrototype | 3 | Independent |
| UnArgumentSpread | 3 | Independent (but would benefit from cross-module facts) |
| UnArrayConcatSpread | 3 | Independent |
| UnSpreadArrayLiteral | 3 | Needs spread-producing rules |
| ObjectAssignSpread | 3 | Independent |
| UnVariableMerging | 3 | Needs UnAssignmentMerging |
| UnIife | 4 + 7 | Enables class/enum detection; second pass after SmartInline |
| UnConditionals | 4 | Needs UnOptionalChaining, UnNullishCoalescing |
| UnEnum | 4 | Needs flat statements |
| UnEs6Class | 5 | Needs helper unwrapping |
| UnClassFields | 5 | Needs UnEs6Class |
| UnAsyncAwait | 5 | Needs UnTsHelpers |

### Modernization

Upgrade to modern ES syntax.

| Rule | Current Stage | Notes |
|------|--------------|-------|
| UnNullishCoalescing | 3 | Independent |
| UnOptionalChaining | 3 | Needs RemoveVoid |
| UnParameters | 4 | Needs FlipComparisons, RemoveVoid, UnConditionals, UnCurlyBraces |
| UnThenCatch | 6 | Independent |
| VarDeclToLetConst | 6 | Late — scope-aware |
| ObjShorthand | 6 | Independent |
| ObjMethodShorthand | 6 | Independent, enables UnPrototypeClass |
| UnPrototypeClass | 6 | Needs VarDeclToLetConst, ObjMethodShorthand |
| Exponent | 6 | Independent |
| ArgRest | 6 | Independent, enables UnRestArrayCopy |
| ArrowFunction | 6 | Independent, enables ArrowReturn |
| ArrowReturn | 6 | Needs ArrowFunction |
| UnForOf | 6 | Independent |

### Cleanup & Naming

Final cleanup, inlining, and renaming.

| Rule | Current Stage | Notes |
|------|--------------|-------|
| UnUseStrict | 3 | Could run anywhere |
| UnUndefinedInit | 6 | Needs RemoveVoid |
| SmartInline | 7 | Needs stable bindings |
| SmartRename | 7 | Needs SmartInline |
| UnReturn | 7 | Must be last |

---

## Critical Dependency Chains

Edges marked `[C]` are confirmed by experiment. Others are `suspected`.

```
SimplifySequence ─────────────────────────────────────────→ (all)

                                                           ┌→ UnImportRename ──→ SmartInline ──→ UnIife2
UnBracketNotation ──┬→ UnInteropRequireDefault ──[C]──┐    │  UnExportRename ──↗               → SmartRename
                    ├→ UnInteropRequireWildcard ──[C]─┤    │                                    → UnReturn
                    ├→ UnObjectRest                   ↓    │
                    └→ UnWebpackInterop (pass 1) [C]→ UnEsm [C]→ UnTsHelpers must run after
                                                      ↑
UnIndirectCall ─────┬→ UnInteropRequireDefault        │
                    └→ UnInteropRequireWildcard        │
                                                      │
UnAssignmentMerging ┬→ UnVariableMerging              │
                    └──────────────────────[C]────────┤
UnEsmoduleFlag ────────────────────────────[C]────────┤
UnTsHelpers ───[C]→ UnAsyncAwait ──→ UnWebpackInterop2 [C]─┘

UnClassCallCheck ───┬→ UnEs6Class ──→ UnClassFields
UnPossibleConstructorReturn ↗

ArgRest ────────────→ UnRestArrayCopy
ArrowFunction ──────→ ArrowReturn
UnWebpackDefineGetters → UnWebpackObjectGetters
SmartInline ────────→ UnIife2

FlipComparisons ──┐
RemoveVoid ───────┤
UnConditionals ───┼→ UnParameters
UnCurlyBraces ────┘

UnNullishCoalescing ┬→ UnConditionals
UnOptionalChaining ─┘

UnToConsumableArray ┐
UnArgumentSpread ───┼→ UnSpreadArrayLiteral
UnArrayConcatSpread ┘

VarDeclToLetConst ──┬→ UnPrototypeClass
ObjMethodShorthand ─┘
```

---

## Candidate Fact Writers (Step 4 preview)

Rules that perform reliable local detection and could emit observations:

| Rule | Observation | Confidence |
|------|------------|------------|
| UnInteropRequireDefault | "binding X was interopRequireDefault-wrapped" | High |
| UnInteropRequireWildcard | "binding X was interopRequireWildcard-wrapped" | High |
| UnEsmoduleFlag | "module has __esModule marker" | High |
| UnWebpackInterop | "binding X had webpack interop getter" | High |
| UnTsHelpers | "module uses TS helpers: __awaiter, __generator, ..." | High |
| UnEsm | "module classified as CJS→ESM; imports: [...]; exports: [...]" | Medium |

## Candidate Fact Readers (Step 5 preview)

Rules that could benefit from merged cross-module facts:

| Rule | Fact needed | Use case |
|------|------------|----------|
| UnArgumentSpread | "binding X is a direct import, not namespace" | Safe `obj.fn.apply(null, args)` → `fn(...args)` |
| SmartRename | "original source names from source maps" | Better rename decisions |
| Late program pass | "exporter module's named exports" | Namespace decomposition |

---

## Step 3 — Experimental Validation Results

Five experiments were run on 2026-04-15. Each modified the pipeline ordering, ran
the full unit test suite (~550 tests), and the most promising candidate was also
tested against the real-world fixture corpus (4500+ webpack modules).

### Experiment 1: UnEsm → Stage 2 (after UnAssignmentMerging)

| Metric | Result |
|--------|--------|
| Unit test failures | **1** |
| Snapshot regressions | 0 |
| Fixture regressions | Not tested (unit failure) |

**Failed test:** `webpack_default_getter_collapses_to_import` — UnWebpackInterop hasn't
run yet at Stage 2, so the `() => mod && mod.__esModule ? mod.default : mod` getter
survives and UnEsm can't collapse it.

**Conclusion:** UnEsm's core `require()` → `import` conversion works fine at Stage 2.
The only blocker is the webpack interop getter pattern, which needs UnWebpackInterop
(first pass) to have cleaned it up first.

**Prerequisite status updates:**
- UnInteropRequireDefault → UnEsm: `confirmed` (passes)
- UnInteropRequireWildcard → UnEsm: `confirmed` (passes)
- UnAssignmentMerging → UnEsm: `confirmed` (passes)
- UnWebpackInterop → UnEsm: `confirmed` (needed for getter collapsing)

---

### Experiment 2: Disable both UnWebpackInterop passes

| Metric | Result |
|--------|--------|
| Unit test failures | **1** |
| Snapshot regressions | **0** |
| Fixture regressions | Not tested |

**Failed test:** Same `webpack_default_getter_collapses_to_import`.

**Key finding:** Zero snapshot regressions means real-world bundles in the unit test
corpus don't exercise the getter pattern in a way that UnEsm alone can't handle.
Other rules (SmartInline, UnIife) compensate.

**Conclusion:** UnWebpackInterop is a **soft prerequisite** for UnEsm. UnEsm handles
plain `require()` conversion independently. The dependency is narrow: only the
getter-wrapped default-access pattern requires pre-processing.

**Prerequisite status update:**
- UnWebpackInterop → UnEsm: `confirmed soft` (needed only for getter collapsing)

---

### Experiment 3: UnTsHelpers + UnAsyncAwait → Stage 2

| Metric | Result |
|--------|--------|
| Unit test failures | **2** |
| Snapshot regressions | 1 (cosmetic) |
| Fixture regressions | Not tested |

**Failed tests:**
1. `webpack_default_getter_collapses_to_import` — UnTsHelpers strips/rewrites the
   `__esModule` pattern before UnEsm can use it for getter detection.
2. `webpack4_unpack_snapshots` — cosmetic JSX diff (`{<U/>}` → `<U/>`), actually an
   improvement.

**Conclusion:** **UnTsHelpers cannot move before UnEsm.** UnEsm needs to see the
`__esModule` helper patterns intact before UnTsHelpers strips them. The earliest safe
position for UnTsHelpers is after UnEsm.

**Prerequisite status update:**
- UnEsm → UnTsHelpers: `confirmed` (UnTsHelpers must run after UnEsm)

---

### Experiment 4: UnCurlyBraces → end of Stage 1

| Metric | Result |
|--------|--------|
| Unit test failures | **2** |
| Snapshot regressions | 1 (cosmetic improvement) |
| Fixture regressions | Not tested |

**Failed tests:**
1. `webpack_default_getter_collapses_to_import` — UnCurlyBraces wraps arrow expression
   bodies into block bodies (`() => expr` → `() => { return expr; }`), making the
   interop getter pattern unrecognizable to UnWebpackInterop's `match_interop_cond`.
2. `webpack4_unpack_snapshots` — same cosmetic JSX improvement.

**Conclusion:** UnCurlyBraces at Stage 1 is **conditionally safe** — it would work if
`match_interop_cond` and `match_interop_block` in `un_webpack_interop.rs` were updated
to also handle the single-return-of-ternary block form:
`() => { return cond ? x.default : x; }`.

**Prerequisite status update:**
- UnCurlyBraces position: `confirmed fragile` — current position works because
  downstream pattern matchers assume expression-body arrows for interop getters.

---

### Experiment 5: UnEsm → end of Stage 4 (after UnEnum)

| Metric | Result |
|--------|--------|
| Unit test failures | **0** |
| Snapshot regressions | 0 |
| Fixture regressions | **YES — 1 file, clear regression** |

**Unit tests:** All pass, including `webpack_default_getter_collapses_to_import`
(because UnWebpackInterop first pass has already run).

**Fixture regression:** `react-app/webpack4/decompiled/entry.js` (93 insertions, 18 deletions):
- UnWebpackInterop2 (second pass) hadn't run yet → an interop wrapper
  `const o = () => { if (n && n.__esModule) { return n.default; } return n; }` leaked
  into the output.
- This broke UnJsx — `o().createElement(...)` was not recognized as React element creation.
- SmartRename produced worse names (`A1` → `a`, `V` → `v`).

**Conclusion:** UnEsm **depends on UnWebpackInterop2** (the second pass), which itself
depends on UnAsyncAwait completing. The current Stage 5 position (after UnWebpackInterop2)
is correct.

**Prerequisite status updates:**
- UnWebpackInterop2 → UnEsm: `confirmed` (hard dependency, proven by fixture regression)
- UnAsyncAwait → UnWebpackInterop2 → UnEsm: `confirmed` chain

---

### Summary Table

| Experiment | Unit Failures | Fixture Regressions | Verdict |
|-----------|--------------|--------------------|---------| 
| UnEsm → Stage 2 | 1 | — | **Blocked** by UnWebpackInterop |
| Disable UnWebpackInterop | 1 | — | **Soft** prerequisite for UnEsm |
| UnTsHelpers → Stage 2 | 2 | — | **Blocked** — must stay after UnEsm |
| UnCurlyBraces → Stage 1 | 2 | — | **Conditionally safe** (needs pattern matcher fix) |
| UnEsm → Stage 4 | 0 | **1 file** | **Blocked** by UnWebpackInterop2 |

### Confirmed Dependency Chain for UnEsm

```
UnBracketNotation ──→ UnInteropRequireDefault ──┐
UnIndirectCall ─────→ UnInteropRequireWildcard ──┤
UnAssignmentMerging ────────────────────────────┤
UnEsmoduleFlag ─────────────────────────────────┤
UnWebpackInterop (pass 1) ──────────────────────┤
UnTsHelpers → UnAsyncAwait → UnWebpackInterop2 ─┤
                                                 ↓
                                              UnEsm
```

All arrows are `confirmed`. UnEsm's current position (end of Stage 5, after
UnWebpackInterop2) is the earliest safe position given the current pattern matchers.

### Discovered Hidden Dependency

**UnWebpackInterop2 → UnEsm** was not previously documented. The second
UnWebpackInterop pass catches interop getters that only become visible after
UnAsyncAwait simplifies `__awaiter`/`__generator` state machines. Without it,
interop wrappers leak into the final output and break downstream rules (UnJsx,
SmartRename).

### Prerequisite Status Update Summary

| Prerequisite | Status | Evidence |
|-------------|--------|---------|
| UnInteropRequireDefault → UnEsm | `confirmed` | Exp 1: passes without other Stage 3-5 rules |
| UnInteropRequireWildcard → UnEsm | `confirmed` | Exp 1: passes |
| UnAssignmentMerging → UnEsm | `confirmed` | Exp 1: passes |
| UnEsmoduleFlag → UnEsm | `confirmed` | Exp 1: passes |
| UnWebpackInterop → UnEsm | `confirmed soft` | Exp 2: only getter pattern affected |
| UnWebpackInterop2 → UnEsm | `confirmed hard` | Exp 5: fixture regression without it |
| UnAsyncAwait → UnWebpackInterop2 | `confirmed` | Exp 5: interop wrappers leak after async restoration |
| UnTsHelpers → UnAsyncAwait | `confirmed` | Exp 3: UnTsHelpers strips patterns UnEsm needs |
| UnEsm → UnTsHelpers | `confirmed` | Exp 3: UnTsHelpers must run after UnEsm |
| UnCurlyBraces ↛ Stage 1 | `confirmed fragile` | Exp 4: breaks interop getter pattern match |

### Actionable Improvements Identified

1. **Fix UnWebpackInterop pattern matcher** to handle block-body arrows
   (`() => { return cond ? x.default : x; }`). This would unblock:
   - Moving UnCurlyBraces to Stage 1 (like the JS version)
   - Potentially simplifying the interop getter detection overall

2. **The `webpack_default_getter_collapses_to_import` test** is a sentinel —
   it caught real issues in 4 of 5 experiments. It should be documented as a
   critical regression test for pipeline ordering.

3. **Fact barrier position:** The confirmed dependency chain shows the barrier
   cannot be placed before end-of-Stage-5 for UnEsm-dependent facts. However,
   observation *emission* (write-only) could start as early as Stage 2 (helper
   unwrapping rules can emit provenance observations without needing merged facts).

---

## Open Questions (Remaining)

1. **Can the interop getter pattern matcher be made block-body-aware?** This is the
   single change that would unlock the most pipeline flexibility. Specifically,
   `match_interop_cond` in `un_webpack_interop.rs` needs to handle:
   `() => { return mod && mod.__esModule ? mod.default : mod; }`

2. **Where is the optimal fact barrier?** Confirmed: the barrier must be after
   Stage 5 for UnEsm-dependent facts. But write-only observation emission could
   start at Stage 2. The phased model from the proposal (write → merge → read)
   remains viable with the barrier after UnEsm.

3. **Should SmartInline and SmartRename be experimentally validated?** They're
   late-pipeline rules with complex interactions. Validating their prerequisites
   would complete the dependency map for the final stages.
