# Cross-Module Fact System

See also: [Architecture](architecture.md) for the pipeline overview,
[Unpacking](unpacking.md) for detector and extraction boundaries,
[Rule dependency inventory](rule-dependency-inventory.md) for where fact-reading
rules fit in the pipeline.

## What it is

A barrier-and-read mechanism that lets Phase 2 rules read provider shape from
**other** modules in the same bundle. Most facts describe normalized ESM
imports/exports. Deliberately narrow pre-`UnEsm` facts preserve CommonJS
provider shape that `UnEsm` would otherwise erase: the proven identity and
statically declared properties of an object assigned directly to
`module.exports`, whether that assignment is the module's only CommonJS
runtime use, an exact ordered default-object composition shell, and positively
observed properties attached to a stable callable before it becomes
`module.exports`, and whether the module marks itself `__esModule`; on the
consumer side, one record per `require()` call (its form, interop, and
uses). For object defaults, the identity proof is independent of
the property list: an empty list may describe a proven empty object rather
than an unknown value. An empty callable-property list is not such a proof.

## Why it's simpler than the original proposal

The original design imagined rules writing per-module "observations" into
shared state, merging them at a barrier, and reading back immutable facts.
We do not need that. After `UnEsm` runs, ESM `import`/`export` declarations are
already a normalized, AST-level representation of module shape. That AST *is*
the fact. The exceptions are syntactic facts collected from the resolved input
before the rule range reaches `UnEsm`: whether raw `module.exports` receives an
object literal (or a stable local alias to one), and whether a stable top-level
function has static properties assigned unconditionally before that exact
binding becomes `module.exports`. `UnEsm` otherwise erases the important
distinction between these CommonJS values and ESM named exports. All collectors
remain pure functions of one module — no rule-written observations and no
merge step.

## Shape

Multi-module unpack runs in two parallel phases with a barrier between them
(`crates/core/src/driver/unpack/phases.rs`). Phase 1 is itself split right
before `UnEsm`, so every module reaches `UnEsm` before any runs it
(barrier 0), and `UnEsm` can read a plan built from all modules:

```
Phase 1a (per module, parallel):
    obtain resolved AST (prepared detector AST, or parse → resolver)
    normalize exact detector-proven webpack runtime branches
    collect raw CommonJS default-object / callable-property facts
    collect require edges: form and interop, keyed by call span
    rule range up to, not including, UnEsm
    add each edge's uses, on the AST UnEsm will see
    collect the interop helpers the module exports, on the same AST

──── barrier 0: InterfacePlan built from every module's require edges
     and helper exports ────

Phase 1b (per module, parallel):
    UnEsm, reading the plan; it records the import tags
    clone barrier AST → recover webpack factory IIFE ESM shapes
    collect_module_facts(&facts_clone)                ← pure AST → facts
    collect_import_call_edges(&barrier_ast)           ← imports still under [[Call]]
    collect constructed-member suffixes(&barrier_ast) ← names only, same AST
    retain original barrier AST + Globals + unresolved mark + import tags

──── barrier: ModuleFactsMap assembled from all modules;
     CommonJsDefaultObjectCompositionPlan and CallRequiredPlan built;
     constructed-member suffixes unioned (not stored on ModuleFacts) ────

Phase 2 (per module, parallel; the late ESM recovery and the source-map
path's rerun through UnEsm read the same plan):
    resume retained barrier AST
    run_commonjs_default_object_composition(module, plan) ← exact mutable default
    run_provider_import_repair(&mut module, facts)    ← proven CJS property edges
    run_provider_namespace_repair(&mut module, facts) ← proven ESM namespace edges
    run_reexport_consolidation(&mut module, facts)
    run_cross_module_lowered_dynamic_imports(...)    ← helper export facts
    run_cross_module_webpack_lazy_imports(...)       ← provider facts
    run_cross_module_interop_imports(...)            ← helper export facts
    import_unproven_helper_requires(...)             ← provider facts
    run_namespace_decomposition(&mut module, facts)  ← reads cross-module facts
    downgrade_unused_synthetic_imports(&mut module)  ← preserve require effects
    registry rule range resuming after UnEsm, through UnReturn
      (UnEs6Class / UnPrototypeClass read this module's CallRequiredPlan pins)
    targeted late cleanup/recovery
    run_kept_require_default_reads(...)              ← a kept require of a provider now ESM
```

Each phase, Phase 1a and 1b included, hands modules to the Rayon pool largest-first by source length
(`driver/unpack/schedule.rs`). Per-module cost is heavily skewed, and Rayon's
default range splitting can leave the largest module to start after most
workers have gone idle, so it becomes the critical path of the phase.
Dispatching in descending size order is the longest-processing-time-first
heuristic; results are placed back in input order, so output never depends on
scheduling.

The normal no-source-map path runs the through-`UnEsm` range once. The retained
AST crosses the barrier together with the exact `Globals` and unresolved mark
that produced its `SyntaxContext`s, so downstream ctxt-sensitive rules keep a
continuous binding identity. Webpack5 and Browserify-family detectors can enter
Phase 1 with a resolved AST produced by detector normalization, avoiding an
emit → parse → resolver round trip. Source-map mode materializes that private
AST sidecar and follows the parser path because emitted mappings need
parser-owned module coordinates.

A structurally identified webpack factory whose reused `module`, `exports`, or
loader parameter cannot be localized safely, or whose rename would capture a
free reference, is deliberately absent from this fact system. Phase 1 inserts
empty facts and a stable `webpack_factory_recovery_failed` diagnostic; Phase 2
returns the raw extracted body without parsing or transforming it. Its ID,
numeric or string, is also excluded from synthesized module-edge maps, so
neither that opaque provider nor its consumers can create a false cross-module
fact. A consumer's output is the one it would have if the provider were absent
from the table, even when a named ID is also a valid relative path to the
opaque file. The missing import report makes dead-module elimination
conservatively retain the graph.

## Facts

`crates/core/src/facts.rs`:

- `ImportFact { local, source, kind: Default | Namespace | Named(imported) }`
- `ExportFact { exported, local, kind: Default | Named }`
- `HelperExportFact { exported, local, kind }`
- `ReexportFact { exported, imported, source }` — `export { A as B } from`
- `ImportCallEdge { source, imported, consumed_by_exports }`
- `ModuleFacts { imports, exports, helper_exports,
  commonjs_default_object, commonjs_default_attached_properties,
  require_returns_exports_object, commonjs_value_is_default, replaces_module_exports,
  marks_es_module, require_edges, has_export_all, export_star_sources, reexports, import_call_edges,
  default_object_ident_properties, ts_helper_exports,
  ts_helper_namespace_factory_exports, receiver_observing_exports,
  stays_commonjs, passthrough_target }`
- `ModuleFactsMap` — keyed by normalized module specifier
  (handles `./foo`, `foo`, `foo.js` variants)

Extraction (`collect_module_facts`) reads the post-Stage-2 AST. Before Stage 2,
`collect_commonjs_default_object` records only direct unresolved
`module.exports = {...}` assignments and stable top-level object aliases. Its
`Option<CommonJsDefaultObjectFact>` distinguishes an unknown value from a
proven object whose declared-property list is empty. Multiple whole-value
assignments, reassigned aliases, non-object values, nested callbacks, and
`exports.default` fail closed. Computed keys and spreads are omitted from the
declared-property list without weakening the proven object identity.

The same object fact separately records two stronger proofs without weakening
that ordinary property fact. `default_assignment_is_only_commonjs_use` scans
the complete resolved AST, including deferred function bodies, and holds only
when the establishing assignment is the sole unresolved `module`/`exports`
use and no direct eval exists. `composition_sources` is populated only when
the complete executable body is an optional strict directive followed by
`module.exports = {}` and one or more exact ordered
`Object.assign(module.exports, require(relativeSource) || {})` calls. Dynamic
or bare sources, extra statements, alternate targets, and alternate fallback
shapes leave that list empty.

`collect_commonjs_default_attached_properties` separately records static
identifier properties assigned directly and unconditionally to a stable
top-level callable around the sole `module.exports` assignment. This includes
the localized webpack form `var alias = module.exports = functionBinding`
followed by direct alias-property writes, including writes in the
guaranteed-once right-hand side of a top-level `for ... in`. It is a
positive-membership fact only: a recorded property can be repaired, but an
absent property says nothing about the callable's runtime surface. Computed,
conditional, nested, or reassigned shapes fail closed. Neither collector
mutates the AST or shared state.

`collect_require_returns_exports_object` records that the module has no
unresolved `module` reference anywhere and no direct eval, so requiring it
returns its `exports` object. The `module` argument of a Cocos registration
frame does not count (`cocos_registration_frame` in
[rewrite-assumptions.md](rewrite-assumptions.md)). `UnEsm` turns both `exports.default = v` and
`module.exports = v` into a default export; only the first is one property
of the required value, which a consumer's whole `require` reads as the
namespace.

`collect_commonjs_value_is_default` records the other case: the module
assigns `module.exports` exactly once, in a position that runs on every
evaluation of its body (a top-level statement, sequence element, or `var`
initializer), and uses `module` only as `module.exports`. Its CommonJS value
is then that assignment's value, which becomes the default export. The
unpack driver also sets it for a webpack factory whose only ESM declaration
is the `export default` a CommonJS runtime normalizer wrote, and, through
`collect_default_object_tail`, for a module that ends in Next's
default-object tail (`module.exports = exports.default` when the default is a
function or an object, `default_object_tail_value` in
[rewrite-assumptions.md](rewrite-assumptions.md)). A consumer that
stays CommonJS keeps `require` of it, which returns the ES module's
namespace, so `run_kept_require_default_reads` reads `.default` where the
value was read. `collect_replaces_module_exports` records any assignment of
`module.exports`, for the warning when that pass cannot tell what the value
was.

The Phase 2 passes do not combine these booleans themselves:
`ProviderInterface::of` (`crates/core/src/provider_interface.rs`) derives,
from one provider's facts alone, what `require` of it returned
(`RequiredValue`: a module that stays CommonJS, `module.exports = v` alone or
beside named exports, a module marked `__esModule`, a value replaced another
way, or the `exports` object), which import reads that value, whether a
namespace import holds what a default import meant, and which properties of
a proven default value a `require("./p").name` captured. The consumer's side
is the module's own require edges (`require_edges`, below), which each pass
queries.

`RequireEdges::uses_whole_value` is the consumer half. It holds for a source
the module uses as the plain required value: every `require("src")` call is
a top-level `var x = require("src")`, and no call or binding of it is tested
for `__esModule` or passed to `require.n` or an interop helper. The helper
stage and `UnWebpackInterop` later replace an interop default
(`() => x && x.__esModule ? x.default : x`) with the plain binding, and
`UnEsm` merges every binding of one source into one default import, so after
Phase 1 the two meanings can no longer be told apart.

`RequireEdges::reads_through_interop` is the opposite: the required value of
the source, or its top-level binding, goes to `require.n` or an interop
helper or has its `__esModule` read. Unpack reads it only for the
`commonjs_require_of_esm` warning: a module that stays CommonJS and read a
marked provider through an interop now meets its namespace, which carries
the marker only when the provider has a default export, and only while the
interop is still there to test it.

A wildcard interop helper is the exception that needs the provider's
`__esModule` marker. It returns a marked provider unchanged, so the binding is
the whole required value; for an unmarked provider it builds a copy whose
`default` is the whole `exports` object. `collect_marks_es_module` records the
provider half: a top-level `__esModule` marker (`Object.defineProperty`,
assignment, or webpack's `require.r(exports)`), read before `UnEsm` removes
it. The marker matcher is the one `UnEsmoduleFlag` uses, and it does not check
the descriptor's value. The marker also counts as an element of a top-level
sequence statement, where a minifier joins it with the statements after it.
`RequireEdges::uses_only_wildcard_interop` is the consumer half: every
`require("src")` call in the module is the argument of a top-level
`var x = wildcard(require("src"))`, with the helper declared or inlined at
the call site (a minifier inlines a single-use helper). Other positions, such
as a wildcard inside a lowered `import()`, do not count.

`require_edges` (`crates/core/src/require_edges.rs`) records each
`require("src")` call of the module as one edge, keyed by the call's span:
the form it is written in (a declarator, a destructure, a member read, an
interop helper argument, a bare statement, or inline), whether it sits in a
top-level statement, the interop around the call or its top-level binding,
and whether `__esModule` is read on it. These are read on the input, before
the rule range erases the interop. Phase 1a then adds how the
value is used on the AST `UnEsm` sees: static member reads, receiver-sensitive
member calls, member writes (assignment, update, `delete`, or the target of
`Object.assign` / `Object.defineProperty`), and escape. A call whose span is
dummy or shared with another call keeps what the input pass read but gets no
key and no uses. A detector that builds the module AST itself gives each
`require()` it translates the span of the input call it replaces (the
Turbopack translator does); what stays dummy is a call with no input call
behind it, such as the requires in Turbopack's synthesized facade and loader
modules. The webpack extractors inline `require.n(require(x))`
(`RequireNRewriter`) by cloning the inner call, so the clones share one span
and get no key.

What crosses barrier 0 is the plan (`InterfacePlan`,
`crates/core/src/interface_plan.rs`), built from each module's require edges
and whether requiring the module returned a writable CommonJS `exports`
object (it has no import or export declaration and is not a webpack harmony
module). A provider that some edge writes to is kept CommonJS: `UnEsm`, in
Phase 1b, in the late ESM recovery, and in the source-map path's rerun, leaves
the module as it is. An ES module has no `exports` object, so this is the only
format in which the consumer's write lands. An edge counts as a write for a
member assignment, update, or `delete` on the value, the target of an
`Object` method that writes, or the value passed to a function of the same
module that writes that parameter. The use collector follows such a call one
level, when the call reaches the function directly (a function literal, or a
binding declared once and never written, in a module without `with` or
direct eval) and the function does not read `arguments`. TypeScript's
guarded helper `this && this.__name || F`, declared or called where `this`
is the module's own, reaches its fallback `F` (both branches of
`c ? F1 : F2`), and writes when either branch does
([`typescript_helper_guard`](rewrite-assumptions.md#typescript_helper_guard)).
Interop and re-export helpers, read-only builtins (`Object.keys`,
`Object.prototype.hasOwnProperty.call` and the other `Object.prototype`
methods that only read their receiver), and functions that only read the
parameter do not count. A callee in another module, or one the collector
cannot identify, does not count either.

One written provider may still become ESM: one whose only export, read on
the input before any rule, is a single top-level `module.exports = v` (no
other `module.exports` member, no reference to the free `exports`, no
`__esModule` marker; `exports_only_whole_value`), when every write lands on
`v` itself. A write lands on `v` when its consumer is CommonJS in the input
(an ES module or webpack harmony consumer writes through a namespace
import), reads the value with no interop helper and no `__esModule` test,
and names neither `default` nor `__esModule` (a member, a literal key of an
`Object` or `Reflect` method, or such a write by a same-module function;
`RequireUses::interop_key_write`). `UnEsm` then writes `export default v`.
A consumer that becomes ESM writes through its default import, which holds
`v`, and one that stays CommonJS reads `require(p).default` after the
barrier (`run_kept_require_default_reads`), the same `v`. The decision reads
only what both sides show before `UnEsm`, never the format a consumer ends
up in. Such a provider still counts as written, so its helpers are dropped.
When `UnEsm` leaves it CommonJS for another reason (a kept `require`), the
fact recovery and the Phase 2 late ESM recovery keep it CommonJS too
(`keep_commonjs` in `RulePipelineOptions`): rules after `UnEsm` can expose a
shape that recovery converts, such as a `require` in a `for` head that
`UnVariableMerging` hoists, and its consumers' `.default` rewrite reads the
facts, which would then say CommonJS for an ES module.

A top-level
`var h = require("src").name` that no statement writes makes `h(x)` a call
of that module's `name` export, read from the same bindings `UnEsm` trusts
for a bundled helper (`top_level_required_bindings`). After the fact
barrier, a consumer that passes a provider now read through its namespace to
such a callee gets the `commonjs_exports_escape` warning (a callee the plan
proves is an interop helper export of its module is exempt). Sources resolve
to modules as `ModuleFactsMap::get_from` resolves them. A harmony provider is
left alone: its exports are getters the runtime defined.

The plan also carries the interop helpers each module exports
(`ProviderHelpers`, `crates/core/src/provider_helpers.rs`), read at the end
of Phase 1a on the AST `UnEsm` will see. A bundle can carry tslib,
`@babel/runtime`, or `@swc/helpers` as its own module, and a consumer then
reaches the helper through `require("./m.js")`, whose name proves nothing.
An export is a helper when the export storage analysis maps its name to a
function the module declares once and never writes (a getter, a mirrored
binding, or a property written once, a function literal included), and that
function has the body of a default or wildcard interop helper
(`classify_interop_helper_function`). The analysis runs without the
module's top-level `__esModule` markers, as `UnEsm` removes them, so
`require.r(exports)` does not fail its module gate. A module whose single
`module.exports = v` statement is its CommonJS value exports `v` itself, and
the Babel runtime shim's `module.exports.default = module.exports` adds a
`default` member when it is the only write of that property. `@babel/runtime`'s
versioned `interopRequireWildcard` replaces `module.exports` and its own
binding with a copy of itself on its first call; that module is proven only
when the copy has the helper's kind, the function's setup before the call
has no effect outside it, and the binding has no other write, so the value
a consumer reads is a helper of that kind before and after the first call
(`SelfReplacement`). A written
provider's helpers are dropped. Each `UnEsm` run that reads the plan adds the
helpers of the sources its module requires to its `LocalHelperContext`
(`with_bundled_helpers`): `ns.name(…)` through a never-assigned top-level
`ns = require("./m.js")`, the inlined `require("./m.js").name(…)`, and, for
a provider that is the helper, the binding of `require("./m.js")` or its
`.default`. `UnEsm` then unwraps those calls as it does for a local or
`tslib` helper, so a getter that re-exports through a bundled tslib's
`__importDefault` becomes `export { default } from`. The rules before
`UnEsm` ran before the barrier and do not see these helpers.

The passes after the barrier recognize an import synthesized from a require
by the import tags (`ImportTags`), not by a dummy span: when `UnEsm` turns a
`require()` into an import, it records each specifier's local binding,
after its own collision renames, with the key of the call it came from. A
specifier the input wrote that `UnEsm` merged into such an import is
recorded as merged, and the passes treat it as synthesized, as they did when
they read the dummy span. Another conversion `UnEsm` runs (an interop or
getter helper turning `var x = helper(require("./p"))` into an import) is
tagged by its declarator, and so is the cross-module interop import pass;
`provider_import_repair` carries the tag to the default import it creates.
The tags cross the barrier with the retained AST; the source-map path's
second run of the range records its own. Imports the default-object
composition pass and `import_unproven_helper_requires` create are not
tagged: each binds a value the pass reads at once, as an `Object.assign`
source or a helper argument, which neither the cleanup of unused
synthesized bindings nor namespace decomposition acts on.

Normal processing also restores webpack's runtime-created `module.exports = {}`
when structural webpack detection proves that a normalized extracted factory
body is empty. A non-empty factory gets the same restoration only when it has a
literal local self-require, cannot observe the original wrapper-created object
through unresolved `module` / `exports`, and contains no function-level
`this`, `arguments`, `new.target`, module syntax, or direct eval. The synthetic
assignment is inserted after the directive prologue and passes through the
ordinary `UnEsm` path, giving both the self edge and downstream consumers a
real default export. All other non-empty factories remain fail closed, and
`--raw` keeps detector output unchanged.

The same detector-owned boundary permits a normal-only webpack runtime
normalizer for two exact inner-UMD expressions whose CommonJS branch is
provable only because webpack initializes every factory's `module.exports`.
It selects a truthy `module.exports` arm, or recovers an undefined-guarded
`factory.apply(exports, [])` assignment only when the immediate factory returns
a stable, non-reassigned function binding. The module shell must be exact,
exactly one rewrite must match, and no unresolved CommonJS runtime references
may remain. Synthetic children from recursive splitting do not inherit the
proof; `--raw` remains detector passthrough.

For a module consisting solely of a synchronous IIFE called with `exports`,
normal processing also restores the runtime-created object, passes it to the
original IIFE, and exports that same mutable object as the CommonJS default.
The argument must be the entire free CommonJS runtime surface: any other
`module`, `exports`, or `require` reference, direct eval, or `with` blocks this
recovery. The IIFE body and its object aliases stay intact; no named exports
are guessed. Async/generator calls, spread or extra arguments, and conditional
or deferred calls are excluded. This uses the same detector-owned boundary and
does not change raw extraction.

That boundary also restores two exact CSS-loader runtime values without
attempting CSS source recovery. A three- or four-field CSS list tuple whose
first item is free `module.id` receives the numeric value preserved from its
actual webpack container key; legacy `module.i` is accepted only when the
detector also proves a Webpack 4-style container. Quoted numeric and named keys
stay unproven. This identity substitution is independent of ordinary
`module.exports` recovery: an unrelated default assignment continues through
the existing `UnEsm` path instead of suppressing the proven numeric value.
It still requires a stable module-identity surface: every free `module` use
must be a static or computed member read, `typeof`, or a static member write
to a property other than `id`/`i`. A rebinding, bare escape, computed write,
or direct identity write fails the substitution closed.
When the same module has one exact top-level conditional
`module.exports = value.locals` switch (an `if` or generated `&&` expression),
normal processing models webpack's initial empty exports object and lets
`UnEsm` recover a default export. That stronger proof fails closed on extra
CommonJS references, computed or multiple candidates, mismatched locals
values, and nested switches. Direct eval disables both recoveries. Synthetic
recursive children do not inherit the key facts, and `--raw` continues to
expose the detector's extracted factory body unchanged.

Helper export facts are still pure AST facts. They only record helper identity
when the exported local binding matches a known helper body shape or runtime
export shape after Stage 2. They do not speculate from consumer-side usage.

### Same-module recovery boundary

UnEsm also recovers narrowly proven same-module CommonJS reads: a later
`module.exports` read may use the sole stable default binding, and a later
`exports.name` read may use a uniquely assigned stable export binding. Direct
method calls additionally require a receiver-insensitive function. Earlier
`undefined` export declarations are permitted; multiple value writes, later
resets, computed writes, value escapes, receiver-sensitive calls, early
reads, direct eval, and hoisted function declarations remain fail closed.

When a literal unresolved `require()` resolves to the current output module,
`UnEsm` verifies the recovered self-import against that same module's export
surface. A synthesized default self-import with no default export rolls the
whole rule back for that module, keeping its CommonJS boundary visible.
A matching default export continues through ordinary recovery. A named-only
self surface may use the existing conservative provider-namespace proof, but
only when every observed use supports namespace semantics; whole-value,
mutable, computed, escaping, or otherwise incompatible reads still roll
back. This does not guess a namespace representation for CommonJS partial
exports.

Same-module `module.exports` / `exports.name` read recovery is not a
cross-module fact. `UnEsm` proves and consumes that identity within one resolved
AST before this barrier. An export whose storage is the property itself is
rewritten to one module binding at every read and write, including inside
function bodies. An export whose storage is a local that every write copies
into the property (`commonjs_export_mirror_coverage`) is exported live from
that local; the copies are dropped and every read becomes the local. Calls through a function that reads `this`, and
names that no storage model fits, stay as visible CommonJS residuals and are
reported as `commonjs_export_unrecovered`. The same local proof can recover Babel-style lazy
default helpers when one top-level function binding and `module.exports` are
replaced together on every whole-value write. The property mirrors remain real
mutations of that binding, while a live ESM default specifier exposes later
replacements. Consumers therefore observe the current implementation rather
than the CommonJS require-time snapshot: call results match the original
lazy-helper pattern, but object identity across a replacement does not.
Neither a live nor a snapshot binding reproduces require-time capture exactly;
the live side is the deliberate choice. Independent or compound writes,
optional-chained `module` access, early observation, direct eval,
receiver-sensitive calls, and additional CommonJS object escapes fail closed.

Two same-module proofs handle the exact
default-compatibility postamble. The named-only proof treats it as a dead branch
only when every `exports` access outside the postamble is a static, non-default
member access to an ordinary name. The default-bearing proof instead rewrites
the generated CommonJS adapter onto the recovered binding when the complete
surface is a set of top-level enumerable getters that each return a local
binding under a distinct name, one of them `default`, the postamble is the
final statement, and there is no authored ESM or other unresolved
`exports` / `module` use. An import `UnEsm` made earlier in the same
conversion, from a require the module wraps in an interop helper, is not
authored ESM: it adds no export the copy would read. Its observable `__esModule` definition and the copy
onto the default value remain: `.default = self` when `default` is the only
getter, otherwise `Object.assign(self, { ... })` listing every getter's binding
in definition order. Native ESM replaces only the final `module.exports`
reassignment. The copy has to stay because an unpacked consumer that imported
both the default and a named export reads them through one default binding.
A one-argument type helper in the guard need not be identified: its call and
receiver remain in place, and only the exact live `exports.default` getter
read becomes its recovered binding. Computed access, whole-object escape,
unresolved getter helpers, data properties, re-export getters, duplicate
getters, prototype-mutating member names (`__proto__`, `__defineGetter__`,
`__defineSetter__`), meaningful `module` use, and direct eval all fail closed.
Neither proof creates a default-object fact available to consumers.

## Rules that read facts

- **`UnEs6Class` / `UnPrototypeClass` cross-file `[[Call]]` pins** — a module
  that still calls an imported constructor with `.call` / `.apply` (including
  `F.call.apply(G, …)`, whose callee is `G`) needs that export to stay an
  ordinary function. Phase 1 records these import edges on the barrier AST,
  before late ESM recovery, because that is the AST class recovery resumes
  from. `CallRequiredPlan` resolves each edge to its defining export at the
  barrier: named-import renames, `export { A as B } from`, one `export *`,
  `export { t as Foo }`, `export default` IIFE returns, and
  `export default { Foo: local }` members are followed. Cycles, several
  `export *` sources, computed members, local aliases of an import, external
  specifiers, and a relative specifier that does not resolve from the
  importing file do not pin. Phase 2 seeds the pinned bindings into
  `CallabilityIndex` before alias propagation, so the constructor an exported
  IIFE returns stays a function too. Single-file `decompile()` has no plan.
  An edge whose call sits in an `extends` IIFE constructor is a prediction:
  Phase 1 runs the `UnEs6Class` matcher on the barrier AST, and when that
  subclass would become a class the call becomes `super()`, so it pins its base
  only if the subclass export is itself pinned. This is the one fact here that
  describes a later rewrite instead of the barrier AST. A misprediction that
  claims conversion leaves the original bug in place (base becomes a class, the
  consumer keeps `.call`). Keep the probe (`super_params_consumed_by_class_recovery`)
  in step with `UnEs6Class` and the spread rules that run before it. The
  probe covers `var C = IIFE` wrappers only; a wrapper in `return` position
  (a mixin's returned class) is never predicted, so its base stays pinned.
  `Minimal` makes no prediction: every cross-file `.call` / `.apply` target
  stays a function. At `Standard` and above, the plan remembers definitions it
  left unpinned only on a prediction; after Phase 2, a module that still calls
  one of them gets a `cross_module_class_call` warning.

- **`commonjs_default_object_composition`** — builds a monotone fixed point at
  the barrier. A provider seeds it only when the raw assignment is its sole
  CommonJS runtime use — residual `require` identifiers (conditional or nested
  requires that never become import facts) count as runtime uses and block the
  seed — and its recovered surface is exactly one default export with no
  imports. Provider bodies are surface-proven only; their side-effect
  statements move ahead of the consumer's copies under
  `import_hoisting_eagerness` (see rewrite-assumptions.md). A composition enters only after every relative provider is
  already proven, so cycles never become eligible. Phase 2 then re-matches the
  complete normalized body before introducing default imports, one stable
  local object, the original ordered `Object.assign` calls, and a default
  export of that same object. Authored ESM defaults, mixed surfaces, unresolved
  providers, dynamic sources, extra consumer behavior, and cycles remain
  visible CommonJS residuals.
- **`provider_import_repair`** — repairs only named imports synthesized by
  `UnEsm` for `require("./x").name` (the import tags below). If provider facts prove that the raw
  CommonJS value is the recovered default object, or positively prove `name`
  was attached to a stable callable default, and `name` is not a true named
  export, the pass imports that default value and captures `.name` into the
  original local binding at the original `require` declaration position. An
  object property absent from a proven empty object is also covered: CommonJS
  returns `undefined`, whereas a guessed ESM named import fails during linking.
  Callable-property absence is never inferred.
  Authored ESM imports, unknown default values, synthesized bindings without
  either a source position or a proven mutable-local capture, and providers
  with `export *` remain unchanged. A whole-object CommonJS
  consumer that mutates provider properties is also deliberately unresolved:
  an ESM namespace is read-only, so preserving that case requires a separate
  mutable facade design.
- **`provider_namespace_repair`** — changes a default import `UnEsm`
  synthesized for a whole-object `require("./x")` into a namespace import when
  the provider facts prove a named or `export *` surface and no default export,
  or a default export whose provider requires as its `exports` object
  (`require_returns_exports_object`) while the consumer used the whole value
  without an interop wrapper (`uses_whole_value` on the consumer's require
  edges), or through a wildcard interop of a provider marked `__esModule`
  (`uses_only_wildcard_interop` and `marks_es_module`).
  It accepts static member reads, `Object.keys(namespace)`, and namespace values
  used as `Object.assign` sources, and a namespace passed on as a value
  (`use(ns)`, webpack's `require.t(ns, 2)` namespace object): the default
  import it replaces cannot link. Simple local aliases preserve that proof; an
  exact, unconditional top-level replacement assignment ends an alias's
  namespace lifetime after its right-hand side is evaluated. Hoisted function
  declarations are always checked against the original lifetime regardless of
  textual position. Authored imports, namespace mutation, conditional/nested
  alias replacement, computed/meta reads, and `__esModule` observation remain
  unchanged.
  Barrier 0 does not move this decision before `UnEsm`: whether a provider
  ends with a default export is known only after its own `UnEsm`, so `UnEsm`
  still makes the default import and this pass corrects it.
  The existing namespace decomposition pass can then recover narrower named
  imports where its own gates allow that rewrite. Single-file decompilation
  has no facts; the `UnEsm` runner makes the same rewrite
  for relative sources from evidence in the importing module alone
  (`relative_require_esm_provider` in rewrite-assumptions.md).
- **`run_cross_module_lowered_dynamic_imports`** — restores a lowered
  `import()`, `Promise.resolve().then(() => _(require(x)))`, when `_` is
  imported from a module whose helper export fact proves it is the wildcard
  interop helper (swc's `externalHelpers` output, with `@swc/helpers` bundled
  as its own module, or tslib's `__importStar` under TypeScript's
  `importHelpers`). `UnEsm` restores the same shape with a local helper,
  and with a bundled one the plan proved at barrier 0; this pass covers a
  helper proven only by the facts after the provider's own `UnEsm`: a helper
  module the input already wrote as ESM, which barrier 0 does not read
  (tslib's ES module build passed as another input), a helper export only
  the late ESM recovery exposes, or an export storage barrier 0 cannot map
  (`lowered_dynamic_import_source_semantics` in rewrite-assumptions.md).
- **`run_cross_module_webpack_lazy_imports`** — restores `import()` from a
  webpack or Rspack lazy load whose module id unpack resolved to a sibling
  path, `require.e(1).then(() => require("./p"))` or
  `Promise.resolve().then(() => require.t("./p", 19))`, and drops its chunk
  loads. The provider's facts decide between `import("./p")` and
  `import("./p").then((ns) => ns.default)`
  (`ProviderInterface::required_value_import`, as for a kept require), or
  keep the loader. Phase 1 leaves the require of such a
  loader to this pass, so the consumer can still become ESM
  (`webpack_lazy_import_source_semantics` in rewrite-assumptions.md).
- **`import_unproven_helper_requires`** — after the interop pass, a
  `helper(require("./p"))` binding it left in an ES module (the helper is
  not proven, or the provider's facts give no import plan) keeps its call,
  and its require becomes an import of the value it returned, by the
  provider's facts (`ProviderInterface::required_value_import`). `UnEsm`
  made the module ESM
  expecting a later stage to take that require
  (`import_hoisting_eagerness`). With bundled helpers proven at barrier 0,
  the calls left here are of a function that is no interop helper (a
  CommonJS function another module exports, `wrap(require("./p"))`), or of
  a helper neither barrier 0 nor the facts prove.
- **`run_cross_module_interop_imports`** — turns a top-level
  `x = helper(require("./p"))` into an import of `./p` when `helper` is
  imported from a module whose facts prove it is a default or wildcard interop
  helper bundled as its own module: tslib under TypeScript's `importHelpers`
  (`__importDefault` matches Babel's `interopRequireDefault` body;
  `__importStar` is a raw TypeScript helper fact), `@babel/runtime`, or
  `@swc/helpers`. The import form follows the
  wrapped provider's facts: a marked provider gives its default export to
  `__importDefault` and its namespace to `__importStar`; for an unmarked one
  the helpers see the whole required value, which is the default export from
  `module.exports = value` or the namespace of a provider with named exports
  and no default. `.default` reads become the binding; other uses that the
  import cannot represent keep the call. `UnEsm` makes the same rewrite with a
  local or `tslib` helper, and with a bundled helper the plan proved at
  barrier 0 (see [Shape](#shape)); this pass covers a helper proven only by
  the facts after the provider's own `UnEsm`, the same three sources as the
  lowered `import()` restore above.
- **`namespace_decomposition`** — rewrites `import r from "./x"; r.foo()` into
  `import { foo } from "./x"; foo()` when `./x` exports `foo` and no collision
  prevents the rewrite. On a namespace import, `ns.default` becomes a default
  import named after the namespace when `./x` has a default export; on a
  default import, `.default` is a property of that value and blocks the
  rewrite. Handles aliased pre-existing specifiers, inner-scope
  shadowing, mixed default+named imports, and readability backoff when too many
  collisions would force aliasing. For imports synthesized from `require()`, it
  also discards an otherwise-inert top-level binding read left by interop-helper
  removal when the original namespace/default specifier can be removed in full;
  substantive whole-object reads and every write still fail closed. A name
  the provider's `receiver_observing_exports` lists (a function that reads
  `this`) and that the consumer calls as `r.foo()`, `r.foo?.()`, or a tag of
  `` r.foo`…` `` keeps that member call and the namespace import, at every
  level: the call passes `r` as `this`. Reads and receiverless `(0, r.foo)()`
  calls of it still decompose. A function that only forwards `this`
  (`g.apply(this, arguments)`, Babel's async wrapper), or an export whose value
  is a wrapping call (`_asyncToGenerator(function* () { … })`), is listed when
  what it forwards to reads `this`; `call_receiver_independence` in
  [rewrite-assumptions.md](rewrite-assumptions.md) has the rule. An export
  initialized through an assignment chain (`export const size = f.size =
  function () { … }`, as `UnEsm` recovers `module.exports.size = …`) is read
  at the chain's last value. A recognized transpiler helper is not listed.
- **`UnObjectSpread`** — in multi-module unpack, recognizes object spread
  helpers imported from a helper module whose default/named export fact proves
  it is Babel's `extends` or `objectSpread` helper. This covers helpers split
  into their own module before consumer calls are rewritten to object spread
  syntax. The consumer import is retained for now because helper identity does
  not by itself prove that evaluating the helper module is side-effect-free.
- **`UnRegenerator`** — in multi-module unpack, recognizes async-to-generator
  helpers that were hoisted into their own module and consumed through generated
  `require()`/interop aliases such as `h.default(...)`, but only when the target
  module's helper export fact proves the default export is the async helper.
- **`UnAsyncAwait`** — recognizes direct imports and namespace members only
  when raw TypeScript helper facts prove `__awaiter` / `__generator`. For
  scope-hoisted CommonJS wrappers, a separate provider-side fact proves that
  the imported zero-argument factory returns the registered helper namespace;
  consumer-side property spelling alone is never sufficient.

## Adding a new fact-reading rule

For a cross-module late pass that naturally runs at the Stage 2 barrier:

1. Put the pass in `crates/core/src/` as a free function taking
   `(&mut Module, &ModuleFactsMap)`.
2. Call it from `unpack_multi_module` between
   `apply_rules(..., RulePipelineOptions::until("UnEsm"))` and the
   post-`UnEsm`-through-`UnReturn` rule range.
3. Do all AST mutation locally to the module — never write back to
   `ModuleFactsMap`.
4. Add unit tests following `crates/core/tests/namespace_decomposition_rule.rs` (use
   `facts_for(source)` to synthesize a target module's facts).

For an existing rule that must stay at its current pipeline position, add an
optional fact-aware constructor and thread `ModuleFactsMap` through the
multi-module rule runner only. Single-file `decompile()` should keep using the
normal constructor.

### Gotchas when synthesizing new idents

- **Use `DUMMY_SP` for new import specifiers, aliases, and rewritten usage
  idents.** `apply_sourcemap_renames()` skips idents only when `span.is_dummy()`;
  real spans would cause the source-map rename pass to vote on positions the
  bundler never emitted.
- **Propagate `SyntaxContext` when reusing an existing binding.** If your
  rewrite replaces `R.foo` with a reference to an *existing* local, stamp the
  existing local's ctxt on the new ident — otherwise later `(sym, ctxt)` passes
  (e.g. `UnImportRename` Stage 6) will rename the binding + original usages but
  miss yours, leaving an undefined reference. A newly created binding (an
  import local, an alias) gets its own context from
  `decl_utils::fresh_binding_ident`; clone that ident for its usages so binding
  and references share it. The resolver isn't re-run, so a context that is
  merely equal on both sides also matches, but every empty-context binding in
  the module then shares one identity.

## Non-goals

- No shared mutable state between rules in the same phase.
- No multi-round merging.
- No speculative facts ("this might be an X"). A fact holds iff the normalized
  post-Stage-2 AST says it does, or the narrow pre-`UnEsm` collector proves the
  exact raw CommonJS assignment shape. The one bounded exception is the
  class-recovery prediction on `ImportCallEdge` described above.

Rules that need heavier semantic conclusions (e.g. "this namespace projection
is always equivalent to a direct import binding") should derive them inside the
rule from the facts they read — not emit them back into the map.
