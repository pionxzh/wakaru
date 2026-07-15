# Rule Dependency Inventory

**Authority split:** the registry in `crates/core/src/rules/pipeline.rs`
(`RULE_DESCRIPTORS`, `RuleDescriptor::requires`, per-rule enable gates) owns
*what* — the full rule list, execution order, stage membership, repeat passes,
and enforced ordering edges. This document owns *why* — safety rationale,
level-gating reasons, fragile orderings, and experiment results that code
cannot express. When the two disagree about order or edges, the registry is
right. Rules with nothing non-obvious to say have no entry here; absence of an
entry means "no known constraints beyond the registry", not "undocumented".

See also: [Fact system](fact-system.md) for the cross-module barrier and the
fact-aware rules that shipped, [Rewrite assumptions](rewrite-assumptions.md)
for the named semantic assumptions levels may rely on,
[Debugging](debugging.md) for tracing which rule caused a regression.

## Vocabulary

- **Prerequisite status:** `suspected` (inferred from code reading),
  `confirmed` (validated by test/experiment — see the experiment log below),
  plus qualifiers `soft` (only a narrow subpattern depends on it) and
  `fragile` (current position works only because downstream matchers assume a
  specific shape).
- **Safety** (internal rule metadata): `safe` (semantics-preserving),
  `heuristic` (high-confidence pattern match), `aggressive` (may change
  semantics). This describes how risky the rewrite logic is in principle —
  it is *not* the user-facing level.
- **User-facing levels** (`RewriteLevel` / `DecompileOptions.level`):
  `minimal` prefers direct, local, high-confidence rewrites; `standard`
  (default) recovers common generated-source patterns on strong local
  evidence; `aggressive` enables speculative recovery. Whole-rule gates are
  visible in the registry (each descriptor's enable gate); subpattern gates
  live inside the rule and are documented in the notes below. `minimal` aims
  for runtime-equivalent output within documented dynamic-scope limits;
  `standard`/`aggressive` are readability policies that may rely on named
  assumptions from [rewrite-assumptions.md](rewrite-assumptions.md).

## Confirmed dependency chains

Edges below are `confirmed` by experiment or by a dedicated regression test.
The registry enforces the executable subset via `RuleDescriptor::requires`.

```
UnBracketNotation ──→ UnInteropRequireDefault ──┐
UnIndirectCall ─────→ UnInteropRequireWildcard ──┤
UnAssignmentMerging ────────────────────────────┤
UnVariableMergingDeclsOnly ─────────────────────┤
UnEsmoduleFlag ─────────────────────────────────┤
UnWebpackInterop (pass 1, soft) ────────────────┤
                                                 ↓
                                              UnEsm
                                                 ↓
              consumed inline TS async helper cleanup (UnAsyncAwait)
                                                 ↓
                                       UnWebpackInterop2
```

Other hard chains (consumer directly matches the producer's output shape):

```
UnClassCallCheck ───┬→ UnEs6Class ──→ UnClassFields
UnPossibleConstructorReturn ↗
ArgRest ────────────→ UnRestArrayCopy
ArrowFunction ──────→ ArrowReturn
UnWebpackDefineGetters → UnWebpackObjectGetters
SmartInline ────────→ UnIife2
UnNullishCoalescing ┬→ UnConditionals
UnOptionalChaining ─┘
UnToConsumableArray ┐
UnArgumentSpread ───┼→ UnSpreadArrayLiteral
UnArrayConcatSpread ┘
FlipComparisons ──┐
RemoveVoid ───────┼→ UnParameters
UnConditionals ───┤
UnCurlyBraces ────┘
VarDeclToLetConst ──┬→ UnPrototypeClass
ObjMethodShorthand ─┘
```

| Edge | Status | Evidence |
|------|--------|----------|
| UnInteropRequireDefault → UnEsm | confirmed | Exp 1 |
| UnInteropRequireWildcard → UnEsm | confirmed | Exp 1 |
| UnAssignmentMerging → UnEsm | confirmed | Exp 1 |
| UnEsmoduleFlag → UnEsm | confirmed | Exp 1 |
| UnWebpackInterop (pass 1) → UnEsm | confirmed **soft** | Exp 2: only the getter-wrapped default-access pattern needs it |
| UnEsm → TS async helper cleanup (UnAsyncAwait) | confirmed | Exp 3 |
| UnAsyncAwait → UnWebpackInterop2 | confirmed | Exp 5: async recovery exposes interop wrappers |
| LocalHelperContext → UnAsyncAwait | confirmed | consumes detected helper identities directly |
| UnCurlyBraces position | confirmed **fragile** | Exp 4: interop getter matchers assume expression-body arrows |
| UnWebpackInterop2 → UnEsm | historical / **superseded** | Exp 5 predates the current registry; UnEsm now runs first with late interop cleanup after |

## Rule notes

Grouped by pipeline area. Only rules with non-obvious constraints, safety
rationale, or level gating appear.

### Syntax normalization

- **SimplifySequence** — runs first; nearly everything downstream assumes
  flat statement lists. Drops provably side-effect-free bare expressions
  (guarded by `unresolved_mark` for call purity). Test pitfall: a bare
  literal statement (`65536;`) is dropped as dead — use `const x = 65536;`.
- **FlipComparisons** — normalizes literals to the right-hand side.
  UnParameters pattern-matches `arg === undefined` with the literal on the
  right.
- **RemoveVoid** — conditional execution: `should_run()` bails if the module
  declares a local `undefined` binding. UnParameters, UnOptionalChaining, and
  UnUndefinedInit all match the `undefined` identifier, not `void 0`.
- **UnIndirectCall** — level-gated by shape: `minimal` removes only
  indirect-call wrappers around direct identifier callees (`(0, fn)()` →
  `fn()`), excluding `eval` and calls inside `with`. Member callees and
  `Object(fn)()` wrappers require `standard` because
  `(0, obj.method)()` → `obj.method()` changes the receiver `this`. Enables
  interop helper detection downstream (`(0, x.default)()`).
- **UnBracketNotation** — critical early normalizer: the interop rules,
  UnObjectRest, UnWebpackInterop, and UnEsm all pattern-match dot-form
  `.default` / `.__esModule`.

### Transpiler helper unwrapping

- **UnInteropRequireDefault / UnInteropRequireWildcard** — need
  UnIndirectCall and UnBracketNotation to have normalized call and member
  shapes; both are confirmed prerequisites of UnEsm.
- **UnObjectSpread** — safe because it only transforms when the first
  argument is `{}`. The esbuild `__spreadValues`/`__spreadProps` variant is
  stateful and deliberately rule-local — see
  [helper-detection.md](helper-detection.md).
- **UnObjectRest** — heuristic: a backward scan absorbs property accesses
  into the rest pattern; needs flat statements and dot notation.
- **UnClassCallCheck / UnPossibleConstructorReturn** — remove guard calls and
  return indirection so UnEs6Class sees clean constructor bodies.

### Structural restoration

- **UnCurlyBraces** — position is confirmed *fragile* (Exp 4): moving it to
  Stage 1 wraps arrow expression bodies into blocks
  (`() => expr` → `() => { return expr; }`), which the interop getter
  matchers in `un_webpack_interop.rs` do not recognize. The JS-era wakaru ran
  it first; the Rust pipeline cannot until those matchers handle the
  block-body form. Produces the block shapes UnConditionals and UnParameters
  expect.
- **UnTypeConstructor** — whole rule gated to `standard+`: `+x` → `Number(x)`
  is semantically equivalent but changes readability intent.
- **UnEsmoduleFlag** — removes `__esModule` flag statements; confirmed UnEsm
  prerequisite (export classification noise).
- **UnAssignmentMerging** — splits `a = b = val`; confirmed UnEsm
  prerequisite: `exports.foo = exports.bar = val` must be split before named
  export detection. Also feeds UnVariableMerging.
- **UnVariableMergingDeclsOnly vs UnVariableMerging** — the decls-only subset
  runs early as a confirmed UnEsm prerequisite (one declarator per statement
  so CJS imports classify); the full pass stays later because its for-loop
  initializer extraction interacts with var→let/const conversion and loop
  scoping.
- **UnBuiltinAliases** — runs after `UnVariableMergingDeclsOnly` so minifier
  aliases such as `var e = Object.freeze, r = Object.defineProperty` have
  already been split into single-declarator statements. Runs before later
  helper-dependent recovery so helper body scanners see canonical
  `Object.freeze(...)` / `Object.defineProperty(...)` calls. `standard+`
  only: relies on `stable_builtins`, rejects `var` aliases with use-before-init,
  writes (including `++`/`delete`), redeclarations, or dynamic-scope constructs
  instead of proving full var→const convertibility.
- **UnArgumentSpread** — `standard+`. Pattern subtleties:
  `fn.apply(null, args)` and `obj.fn.apply(obj, args)` are safe;
  `obj.fn.apply(null, args)` is *intentionally skipped* — rewriting it to
  `fn(...args)` is not semantics-preserving without cross-module proof that
  the member is a plain imported function (candidate fact reader).
- **UnArrayConcatSpread** — `standard+`: `[a].concat(b)` → `[a, ...b]` is not
  strictly equivalent for scalars, strings, patched `concat`, or
  `Symbol.isConcatSpreadable`; the useful generated shape is
  `[fixed].concat(args)`.
- **UnNullishCoalescing** — pattern-level gating: strict null checks
  (`x === null || x === undefined`) run at all levels; loose
  `x != null ? x : y` requires `standard+` (assumes `no_document_all`);
  temp-based forms run at `minimal` when binding analysis proves the temp is
  isolated; non-identifier bases (member/computed) require `aggressive`
  because collapsing three reads to one changes getter/proxy semantics
  (assumes `pure_getters`). Must run before UnConditionals, which would
  otherwise consume eligible ternaries.
- **UnOptionalChaining** — needs `undefined` identifiers (RemoveVoid).
  Gating mirrors UnNullishCoalescing: loose null-check recovery at
  `standard` when evaluation count is preserved; Babel loose
  repeated-property call forms
  (`_obj.method == null ? undefined : _obj.method(arg)` → `obj?.method?.(arg)`)
  require `aggressive` (assumes stable property reads). Shares
  structural-equality helpers with UnNullishCoalescing. Must run before
  UnConditionals.

### Bundler artifacts and module system

- **UnWebpackInterop** — three passes, each for a different exposure point:
  pass 1 before UnEsm (confirmed-soft prerequisite — only the getter-wrapped
  default-access pattern needs pre-cleaning, Exp 2); pass 2 after
  UnAsyncAwait (async/regenerator recovery exposes interop getter shapes,
  Exp 5); pass 3 after UnEsm (catches `require.n(importBinding)` shapes
  exposed by import conversion).
- **UnEsm** — the module-system barrier. `standard+`. Its confirmed
  prerequisite chain is diagrammed above; multi-module unpack extracts
  cross-module facts from its output (see
  [fact-system.md](fact-system.md)). Historical experiments that placed it
  elsewhere are superseded — treat the registry as authoritative. Static
  CommonJS live getters (`get: () => dep.member`) become source re-exports
  only when `dep` is a resolver-proven top-level literal `require()` binding
  and every use is a static member read; writes, dynamic reads, and escapes
  preserve the getter form.
- **UnIife** — two passes; the second catches IIFEs created by SmartInline.
  Exposes class IIFEs for UnEs6Class and enum IIFEs for UnEnum. Gating:
  param cleanup and literal hoisting are `standard+`; `.call()` unwrapping on
  arrows runs at all levels.

### Complex pattern restoration

- **UnConditionals** — must run after `??`/`?.` recovery. Produces the
  if-statement form UnParameters needs. Only converts "action-like" branches
  to statements; switch recovery is limited to strict equality over one
  identifier with literal cases. The second pass is the final pipeline rule:
  SmartInline, ArrowFunction/ArrowReturn, and UnReturn expose conditionals
  the first pass could not see.
- **UnParameters** — needs the shapes produced by FlipComparisons,
  RemoveVoid, UnConditionals, and UnCurlyBraces. Pattern A
  (`if (arg === undefined) arg = val`) runs at all levels; `arguments[i]`
  reconstruction, object-alias defaults, and destructured-alias folding are
  `standard+`. Pitfall: `stmts_reference_ident` matches by *emitted name*,
  ignoring SyntaxContext — intentional (prevents invalid parameter lists
  after rewriting) but can make folds bail when an alias was inlined to a
  short parameter name.
- **UnEnum** — needs the paired `var X; (function(X){...})(X || (X = {}))`
  visible as adjacent flat statements (SimplifySequence). It also recovers the
  TypeScript CommonJS publication form using the resolver-proven free `exports`
  binding. Split declarations are accepted only when intervening code touches
  neither the local nor public binding, and every enum value must be literal:
  effectful member initializers are preserved because publishing the object
  before running the IIFE can be observable through cycles.
- **UnJsx** — detects pragma imports via `unresolved_mark`. Dynamic-tag alias
  synthesis (creating `const Component = expr` for non-identifier tags)
  requires `aggressive`, or `standard` with strong JSX shape evidence.
- **UnEs6Class** — needs UnClassCallCheck, UnPossibleConstructorReturn, and
  UnIife (class IIFE wrappers). Static *method* assignment recovery is part
  of class restoration; static *data field* recovery
  (`Ctor.x = value` → `static x = value`) requires `standard+` and is
  skipped for derived classes — inherited static setters make assignment
  observably different from field definition.
- **UnClassFields** — needs UnEs6Class. Babel constructor field recovery
  (`_defineProperty(this, "x", value)` → `x = value`) is `standard+`, base
  classes only, and skips initializers that reference constructor params or
  `arguments`. Direct constructor assignments are preserved unless another
  pattern proves they came from class fields.
- **UnAsyncAwait** — consumes `__awaiter`/`__generator` identities detected
  by `LocalHelperContext` directly (no alias renaming step). Its consumed
  inline TS helper cleanup must run after UnEsm (Exp 3: early cleanup strips
  `__esModule` patterns UnEsm needs for getter detection). Recovery exposes
  new shapes for the late UnObjectRest, UnArgumentSpread, and
  UnWebpackInterop passes.

### Modernization

- **VarDeclToLetConst** — late by design: every rule that introduces new
  variables must run first. The contract cuts both ways: earlier rules that
  construct declarations must emit the consumed statements' kind (or `var`)
  and let this rule decide mutability — it converts `var` to `let`/`const`
  but never widens an existing `const`, so a hardcoded `const` on a binding
  that is later written ships a runtime `TypeError`.
- **ArgRest → UnRestArrayCopy** — hard chain: UnRestArrayCopy detects the
  Babel copy loop for rest params that ArgRest just created. ArgRest is
  `standard+`.
- **ArrowFunction → ArrowReturn** — hard chain. ArrowFunction is `standard+`
  even though it checks known blockers (`this`, `arguments`, named function
  expressions, bindings later used with `new`): arrows lack `prototype`,
  cannot be constructed, and differ for `new.target`, so broad conversion is
  not a `minimal`-safe transform.
- **UnForOf** — `standard+`. Helper recovery is conservative: it requires the
  full emitted cleanup wrapper before removing iterator/error temporaries.
- **UnUndefinedInit** — needs RemoveVoid; feeds VarDeclToLetConst.
- **UnPrototypeClass** — needs `const`/`let` declarations (VarDeclToLetConst)
  and method shorthand (ObjMethodShorthand) to detect class candidates.

### Cleanup and renaming

- **UnWebpackDefineGetters → UnWebpackObjectGetters** — hard chain: the
  second converts the `Object.defineProperties` calls the first produces
  into getter syntax.
- **UnImportRename / UnExportRename** — need UnEsm's import/export
  declarations; both rename via `BindingRenamer`.
- **SmartInline** — needs stable import/export bindings, so it runs after
  the rename rules. It removes alias declarations (`var h = p`) — any rule
  that needs aliases intact must run earlier. It can create new IIFEs, so
  UnIife2 must follow. Generic temp-var inlining is limited to generated-looking
  `const` aliases of proven-frozen local sources whose sole use is in the
  immediately following statement. Existing `let` and long-lived aliases
  remain available for SmartRename to recover use-site names; imports,
  unresolved globals, outer lexicals, dynamic scope, later same-scope writes,
  and any nested/deferred-body write are rejected. Gating: temp-var inlining,
  useState tuple folding, property-destructuring grouping, and builtin/global
  alias inlining (`const E = TypeError` → inline) are `standard` (assumes
  `stable_builtins`); index-based destructuring grouping (`obj[0]`, `obj[1]`
  → array destructuring) is `aggressive`.
- **SmartRename** — after SmartInline (aliases removed, names stabilized).
  Candidate consumer of source-map-recovered names.
- **UnReturn** — removes tail `return undefined`; runs before the final
  UnConditionals pass, which can simplify patterns this exposes.

## Experiment log (2026-04-15, distilled)

Five pipeline-reordering experiments, each run against the full unit suite;
the most promising also ran against the real-world fixture corpus.
**Current-state note:** the live registry now runs UnEsm before UnAsyncAwait
and UnWebpackInterop2, with late interop cleanup after UnEsm. Read these as
evidence about fragile shapes, not as the current edge set.

1. **UnEsm → Stage 2 (after UnAssignmentMerging):** 1 unit failure
   (`webpack_default_getter_collapses_to_import`) — the webpack interop
   getter survives without UnWebpackInterop pass 1. Core `require()` →
   `import` conversion itself worked. Confirmed the interop/assignment
   edges in the table above.
2. **Disable both UnWebpackInterop passes:** same single failure, zero
   snapshot regressions — the dependency is narrow (getter-wrapped default
   access only), hence *confirmed soft*.
3. **TS async helper cleanup → Stage 2:** early cleanup stripped
   `__esModule` patterns before UnEsm could use them. Cleanup must stay
   after UnEsm.
4. **UnCurlyBraces → end of Stage 1:** wrapping arrow bodies into blocks
   made interop getters unrecognizable to `match_interop_cond`. Would be
   safe if the matchers in `un_webpack_interop.rs` handled
   `() => { return cond ? x.default : x; }` — the single change that would
   unlock the most pipeline flexibility.
5. **UnEsm → end of Stage 4:** all unit tests passed but one fixture file
   regressed: an interop wrapper leaked (UnWebpackInterop2 had not run),
   which broke UnJsx detection and degraded SmartRename output.
   **Superseded:** it proved interop wrappers exposed after async
   restoration materially affect output quality — not that UnWebpackInterop2
   must precede UnEsm. The current registry handles this with late interop
   passes.

**Sentinel test:** `webpack_default_getter_collapses_to_import` caught real
issues in 4 of 5 experiments. Treat it as the canary for pipeline-ordering
changes.

## Open questions and ideas

1. **Block-body-aware interop matching** — make `match_interop_cond` in
   `un_webpack_interop.rs` handle
   `() => { return mod && mod.__esModule ? mod.default : mod; }`. Unlocks
   moving UnCurlyBraces to Stage 1 and may simplify getter detection.
2. **Cross-module receiver proof for UnArgumentSpread** — a fact proving
   "binding X is a direct import, not a namespace" would make
   `obj.fn.apply(null, args)` → `fn(...args)` safe to recover.
3. **Source-map names for SmartRename** — feed recovered original names into
   rename decisions.
4. **SmartInline / SmartRename validation** — the late pipeline has complex
   interactions that have not had the same experimental treatment as the
   UnEsm neighborhood.
