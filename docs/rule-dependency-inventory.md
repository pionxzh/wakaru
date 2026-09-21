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

UnAssignmentMerging owns safe chained-assignment normalization, including
pure `void <number>` values when RemoveVoid cannot introduce `undefined`.
Its CommonJS receiver checks allow static stores on one `module.exports`
object while retaining chains that would need a receiver or value capture.
UnEsm does not duplicate this splitter. It recovers a remaining top-level
named-export chain as one operation: a repeatable value (identifier,
primitive literal, `void <number>`) is stored per target; a function or
arrow expression, or a `require("literal")` call, is evaluated once into a
fresh `var` named after the innermost free export name, and every target,
including a `module.exports` slot or resolved local head, stores that
binding, so recovered exports stay snapshots. Creating a function runs no
code, and the require form takes the same provider-ordering deviation as the
single `exports.name = require(...)` recovery (`import_hoisting_eagerness`),
so no module-wide receiver analysis is needed. A call rooted, through static
keys, at a provider binding (a top-level `require("literal")` declarator or a
static member of one, declared once and never written) with repeatable
arguments other than the wrapper bindings takes the same stored path at the
chain's own position (`chain_receiver_reference_order`); the binding-use walk
that qualifies provider bindings runs only when such a chain is present. Any
other effectful value stays whole.
`module.exports = exports.default = value` becomes the default-export mirror
pair. The normalization skips a module with direct eval or `with`, because
the recovered exports become module bindings. A chain UnEsm cannot recover
this way (an effectful non-function value, inside control flow or an
initializer, a non-static key or the `exports` key, a `module.exports.default`
mirror, several targets beside a default, or other target kinds) keeps the
whole module at the CommonJS boundary before import/export classification.
The coupled form
`module.exports = exports = value` is not a named-export chain (its inner
target is the `exports` binding) and keeps its separate whole-module
recovery. A single static export followed only by resolved local assignments
stays on the existing classification path, preserving enum and decorator
recovery.

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
UnParameters → UnArrayConcatSpreadRest → UnSpreadArrayLiteral2 → UnEs6Class
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

- **UnComputedProperties** — runs before SimplifySequence, the only rule that
  does. Babel's loose computed-properties lowering is a single comma
  expression (`_n = {}, _n[k] = 1, _n`); once SimplifySequence splits it into
  statements the object-building shape is gone, so this rule has to see the
  sequence intact. It emits `PropName::Str` for string keys and leaves the
  identifier/numeric normalization to UnBracketNotation downstream rather
  than duplicating that logic. Level-gated to `standard` (assumption
  `set_computed_properties`).
- **SimplifySequence** — runs first among the rules that assume flat input;
  nearly everything downstream assumes flat statement lists. Drops provably side-effect-free bare expressions
  (guarded by `unresolved_mark` for call purity). Test pitfall: a bare
  literal statement (`65536;`) is dropped as dead — use `const x = 65536;`.
- **FlipComparisons** — normalizes literals to the right-hand side.
  UnParameters pattern-matches `arg === undefined` with the literal on the
  right.
- **RemoveVoid** — conditional execution: `should_run()` bails if the module
  declares a local `undefined` binding, or contains `with` or a direct `eval`
  that could bind the name (the module-wide skip in
  [Rewrite assumptions](rewrite-assumptions.md#dynamic-scope-limits); a known
  eval source string blocks only when it mentions the name).
  UnParameters, UnOptionalChaining, and UnUndefinedInit all match the
  `undefined` identifier, not `void 0`, so a skipped module keeps `void 0` and
  those rules see nothing to recover.
- **UnInfinity** — conditional execution: `should_run()` bails on the same
  conditions as RemoveVoid, for a local `Infinity` binding.
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
- **UnTypeConstructor** — whole rule is `aggressive` only. `+x` → `Number(x)`
  changes BigInt behavior from throwing to conversion, while `x + ""` →
  `String(x)` changes the coercion hint observed by `Symbol.toPrimitive`.
  Replacing a hole array with `Array(n)` also introduces a mutable global
  constructor lookup. `minimal` and `standard` preserve all three shapes.
- **UnBuiltinPrototype** — whole rule is `aggressive` only, under
  `terser_unsafe_proto`. It reverses Terser's opt-in `unsafe_proto` compression,
  whose producer-side matcher only transforms undeclared builtin references.
  Wakaru accepts that producer assumption in aggressive mode and deliberately
  keeps a compact shape matcher instead of rebuilding a JS+TS scope model.
  `minimal` and `standard` preserve literal receivers.
- **UnEsmoduleFlag** — removes `__esModule` flag statements; confirmed UnEsm
  prerequisite (export classification noise).
- **UnAssignmentMerging** — splits `a = b = val` into one statement per
  target, innermost first (`b = val; a = val;`), which is the order the
  chained form commits its writes (own setters, a throwing `const` write).
  Splitting also moves each target's reference evaluation next to its write,
  so a target is accepted only when that evaluation can neither throw nor
  change between the writes: a plain identifier, or a member rooted at the
  CommonJS wrapper bindings `module`, `exports`, `require`, with identifier,
  private-name, or string/number literal keys. Receivers containing another
  member read (`module.exports.x`, `exports.box.flag`) keep the chain: an inner
  write may replace the intermediate object even with ordinary data properties.
  Everything else keeps the chain at every level: a local root may be in TDZ
  (`root.x = inner = 1; let root = {}` throws before `inner` is written) or be
  reassigned by an inner setter, `this` throws before `super()` in a derived
  constructor, an undeclared global root throws, computed keys may run code,
  call receivers reorder calls. The value is evaluated once per split
  statement, so an identifier value (other than `undefined`) is accepted only
  when no write can change it: identifier targets must be resolved bindings
  or CommonJS names, and CommonJS-rooted member targets rely on
  `commonjs_exports_data_properties` (a setter on the module's own `exports`
  could reassign the value; accepted, not proven). An undeclared global
  identifier target may be a global-object accessor and splits only with a
  literal value. (The webpack unpacker's localized reused parameter,
  `var _publicValue`, now carries a non-unresolved context for this reason.) The remaining chains (`t.prototype.a = t.prototype.b = fn`,
  `o[A] = o[B] = true`, TypeScript's `ns.A = ns.B = void 0` on a local
  namespace object) are source form, not minifier output: swc's
  `merge_sequential_expr` (and terser's `collapse_vars`, from memory) only
  build a chain whose inner target is an identifier, and TypeScript
  synthesizes the `exports` chain against an object it owns. A `standard`
  same-root extension under a new setter assumption was considered and not
  taken for that reason (2026-09-06). Confirmed UnEsm prerequisite:
  `exports.foo = exports.bar = val` must be split before named export
  detection, and UnEsm's coupled `module.exports = helper = val` recovery
  re-merges the innermost-first pair. Also feeds UnVariableMerging.
- **UnVariableMergingDeclsOnly vs UnVariableMerging** — the decls-only subset
  runs early as a confirmed UnEsm prerequisite (one declarator per statement
  so CJS imports classify); the full pass stays later because its for-loop
  initializer extraction interacts with var→let/const conversion and loop
  scoping. The full pass computes the existing test/update `must_keep` set and
  initializer dependency closure, then partitions in source order: a declarator
  with an initializer is extracted only while no kept declarator with an
  initializer precedes it (crossing one would reorder initializer effects); a
  declarator without an initializer is always extracted, since it evaluates
  nothing. The no-init case is load-bearing for UnForOf — the swc/babel
  iterator-protocol matcher needs the bare `step` pulled out of
  `for (var it = x[Symbol.iterator](), step; ...)`.
- **UnBuiltinAliases** — runs after `UnVariableMergingDeclsOnly` so minifier
  aliases such as `var e = Object.freeze, r = Object.defineProperty` have
  already been split into single-declarator statements. Runs before later
  helper-dependent recovery so helper body scanners see canonical
  `Object.freeze(...)` / `Object.defineProperty(...)` calls. `standard+`
  only: relies on `stable_builtins`, rejects `var` aliases with use-before-init,
  writes (including `++`/`delete`), or redeclarations instead of proving full
  var→const convertibility, and skips the whole module for every declaration
  kind when a `with` or direct `eval` is present (the dynamic-scope skip).
- **UnArgumentSpread** — `standard+`. Pattern subtleties:
  `fn.apply(null, args)` and `obj.fn.apply(obj, args)` are safe;
  `obj.fn.apply(null, args)` is *intentionally skipped* — rewriting it to
  `fn(...args)` is not semantics-preserving without cross-module proof that
  the member is a plain imported function (candidate fact reader).
- **UnTemplateLiteral** — level gating is per path, not per rule (the rule is
  always enabled because tagged-template helper recovery is provenance-checked
  and runs at every level). The `+`-chain path (`string_coercion_hint`) and the
  `.concat`-chain path (`concat_coercion_order`) are `standard+` for arbitrary
  substitutions; at `minimal` both rewrite only when every substitution is a
  primitive by syntax. The plus-chain path was deliberately kept at `standard`
  rather than demoted to `aggressive`: Babel loose and TypeScript ≤ 4.4 lower
  templates to plain concatenation, and the private fixtures recover ~3,000
  templates through it.
- **UnArrayConcatSpread** — array-literal arguments flatten at every level.
  An arbitrary concat argument becomes a spread only at `aggressive`, under
  `concat_arguments_are_arrays`; scalars, strings, and general iterables do
  not share concat's spread semantics. At `standard`, the later
  **UnArrayConcatSpreadRest** pass admits only existing rest parameters or
  canonical Babel/TypeScript `arguments`-copy arrays, and only when every use
  after initialization is an eligible concat operand. It runs before
  UnEs6Class, followed by a second UnSpreadArrayLiteral pass, so a proven
  `[this].concat(args)` can expose `Base.call.apply(Base, [this, ...args])`
  without restoring the unsafe general heuristic.
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
  preserve the getter form. Static named CommonJS writes inside control-flow
  statements that run during module activation become module-scoped live ESM
  bindings; reads and later writes to the same property follow that binding.
  A write found only in a function, constructor, or instance field does not
  trigger recovery. Bare receiver uses, dynamic keys, receiver replacement,
  unsupported writes, direct eval/`with`, self-require, pre-existing ESM
  exports, or another CommonJS member access that would survive conversion
  preserve the whole CommonJS boundary. The synthesized declarations use
  fresh resolver contexts and collision-free emitted names, and precede
  VarDeclToLetConst so that rule decides their final declaration kind.
  A leading statement-level `exports.name = void 0` or unresolved
  `undefined` sentinel is omitted once recovery creates the same uninitialized
  binding, provided its prefix contains only directives, imports, hoisted
  function declarations, statically classified require declarations, and
  other sentinels. Later or effectful initializers remain.
  Ordinary `exports.public = local` assignments keep CommonJS snapshot
  semantics when `local` has any direct or deferred write: UnEsm captures its
  value at the assignment instead of emitting a live export alias. A proven
  `Object.defineProperty` or webpack getter remains live.
- **UnIife** — two passes; the second catches IIFEs created by SmartInline.
  Exposes class IIFEs for UnEs6Class and enum IIFEs for UnEnum. Gating:
  param cleanup and literal hoisting are `standard+`; `.call()` unwrapping on
  arrows runs at all levels. Positional argument/parameter pairing stops at
  the first spread argument: its runtime length makes later syntactic
  positions unknown, so only arguments before it are extracted or renamed.

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
  `standard+`. Pattern A bails when the function or a nested arrow can
  observe the `arguments` mapping that a simple parameter list keeps
  (`arguments.length` and literal tail indexes are fine; other uses and direct
  `eval` are not), so a TypeScript rest-parameter copy loop blocks the first
  pass and UnParameters2 recovers the default after ArgRest has replaced the
  loop. Guards convert in ascending parameter order within the first 15
  statements; a guard that follows other statements converts only when the
  default is side-effect-free (literals, function values, `this`, identifier
  reads, structures of those; property reads at `standard+`), no skipped
  statement or hoisted function declaration mentions the parameter, and every
  value the default reads is stable across the skipped statements: a parameter
  it reads must not be written there (directly, by `++`, or by a closure
  created there), and an outer binding, global, or property read requires the
  skipped statements to be inert declarations (`var _this = this;`, `let t;`,
  and at `standard+` object destructuring declarations). A parameter read or
  written before its guard (ws `ping`, commander `_prepareUserArgs`) keeps the
  guard, and so does `seed = 1; if (a === void 0) a = seed;`.
  `Function.length` still drops to the first default's index; that arity
  change is accepted. Pitfall: `stmts_reference_ident` matches by *emitted name*,
  ignoring SyntaxContext — intentional (prevents invalid parameter lists
  after rewriting) but can make folds bail when an alias was inlined to a
  short parameter name.
- **UnEnum** — needs the paired `var X; (function(X){...})(X || (X = {}))`
  visible as adjacent flat statements (SimplifySequence). It also recovers the
  TypeScript CommonJS publication form using the resolver-proven free `exports`
  binding. Split declarations are accepted only when intervening code touches
  neither the local nor public binding, and recovery is rejected when later
  code references the same public `exports` member. Local and exported enum
  recovery both require literal values: computed member initializers are
  preserved because an object-literal rewrite can otherwise duplicate
  evaluation or observe the enum before publication. Numeric forward/reverse
  properties are emitted as consecutive pairs to preserve assignment order.
- **UnNamespace** — `standard+`, after UnEnum and ArrowFunction. It recovers
  simple TypeScript runtime namespace IIFEs as a block containing a stable
  alias to `X || (X = {})`, preserving repeated namespace augmentation and
  existing-object behavior instead of replacing the namespace with an object
  literal. The body must consist only of sequential static member assignments.
  Function-scoped declarations, direct eval (including in nested functions),
  lexical `this`/`arguments`/`new.target`, alias reassignment, and non-canonical
  initializer arguments preserve the IIFE.
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
  `standard+`. Nested arrows read the enclosing function's `arguments`, so
  they are checked and rewritten with the body; nested regular functions and
  class constructors have their own `arguments` and are skipped (a constructor
  is a separate AST node, not a `Function`). A parameter initializer that
  mentions `arguments` blocks the rewrite. The fresh rest name is `args`, or
  `args_1`, `args_2`, ... when the body or parameter list already spells that
  identifier anywhere (a declaration or a reference to an outer binding),
  since printed code has no `SyntaxContext` to keep them apart. A reused copy
  binding gets the same check: if another binding shares its name, choose an
  unused suffix and rename only the copy binding and its resolved references
  before removing the loop. This applies to functions and constructors.
- **ObjMethodShorthand / ArrowFunction** — both consult the shared
  constructor-sensitive value analysis before replacing ordinary function
  values with non-constructible method or arrow syntax. The analysis recognizes
  `new`, `Reflect.construct`, `extends`, `instanceof`, and `.prototype`, then
  propagates requirements backward through exact static-member aliases. Plain
  binding aliases also carry member suffixes (`alias = namespace; new alias.C()`
  protects `namespace.C`) without recursively extending cyclic member paths.
  Both the use-site marking and the alias graph walk the same value wrapper
  shapes the converters protect syntactically — parentheses, sequence results,
  conditional/logical branches, assignment results, and `.bind` targets — so
  `new (cond ? f : g)()` and `bound = f.bind(x); new bound()` protect the
  underlying bindings. Aliases are also recorded for logical assignments
  (`cached ||= ctor`) and object-destructuring bindings (`const { C } = ns`,
  including renames, nested patterns, defaults, and rest bindings). A shared
  pattern-default walker keeps the analysis and both mutators aligned for
  anonymous sources that have no `ValueKey`; constructor-sensitive inline
  defaults and destructured object-literal values stay ordinary functions.
  Logical-assignment object values receive the same contextual member keys as
  plain assignments.
  ObjMethodShorthand is always enabled; its other eligibility checks remain
  unchanged.
- **ArrowFunction → ArrowReturn** — hard chain. ArrowFunction is `standard+`
  even though it checks known blockers (`this`, `arguments`, named function
  expressions, `new.target`, and ordinary-function values required by `new`,
  `Reflect.construct`, `extends`, `instanceof`, or `.prototype` observation).
  The `this`/`arguments`/`new.target`/direct-eval checks cover parameter
  initializers and destructuring defaults as well as the body; both run in
  the function's own activation.
  Arrows lack `prototype` and cannot be constructed, so broad conversion is not
  a `minimal`-safe transform.
- **UnForOf** — `standard+`. TypeScript/Babel/SWC helper recovery is
  conservative: it requires the full emitted cleanup wrapper before removing
  iterator/error temporaries. Closure Compiler is a separate exact producer
  shape: an adjacent `$jscomp.makeIterator(iterable)` assignment plus the
  canonical `.next()` loop. It requires either the unresolved Closure runtime
  namespace or the canonical `var $jscomp = $jscomp || {}` bootstrap, and it
  compares candidate-local uses with the module-wide binding index so it bails
  if the iterator/result bindings escape any enclosing block or the iterator is
  used in the loop body.
- **UnUndefinedInit** — needs RemoveVoid; feeds VarDeclToLetConst.
- **UnPrototypeClass** — runs before ArrowFunction so Closure Compiler's
  single-declarator anonymous function initializers remain available for class
  recovery. It also accepts ordinary function declarations. A constructor sharing
  an enclosing function parameter name stays in prototype form: replacing its
  function declaration with a lexical class would make the body invalid. This
  guard applies to the direct function body, not independently nested scopes. Nested candidates
  must have reached `const` through VarDeclToLetConst; module-level Closure
  variables are handled in place. Function-variable candidates with exact-binding
  pre-references (including references captured by earlier closures), multiple
  declarators, or named function expressions are preserved because converting
  them would change binding or recursion semantics. Function-variable candidates
  also stay in prototype form when an unrecognized interstitial call involving
  the constructor may replace its prototype; this call gate is specific to the
  newly supported variable shape and does not change the existing
  function-declaration recovery policy. Independently of constructor kind, an
  retained whole-prototype write (`Foo.prototype = <expr>`) anywhere in the
  scope — before the constructor, between methods, trailing, inside a nested
  function, or inside a recovered constructor/method body — blocks recovery: a
  class would bake the collected methods into its own non-writable `prototype`
  instead of the replacement object. Exact `Object.create` inheritance writes
  consumed by the recovery remain eligible.
  ObjMethodShorthand remains an upstream normalizer for method bodies.

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
- **MergeDeclarationInit** — runs after SmartInline and UnDestructuring so their
  assignment-form temporaries remain available. Adjacent top-level anonymous
  class assignments can merge when class creation has no user-code execution
  and the class does not reference the outer binding. This exposes an immutable
  initializer for UnExportRename2 without relaxing export write guards.
  After all statement-list and
  narrow top-level merges, it performs one module-wide resolved-write pass and
  promotes only the merged `let` bindings with no remaining direct write or
  relevant direct-eval source. The batched pass closes the ordering gap after
  `VarDeclToLetConst` without rerunning that broader rule or scanning once per
  binding; modules with no merged `let` skip the recheck.
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
