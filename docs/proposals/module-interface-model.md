# Module interface model: provider and consumer facts before `UnEsm`

Status: done (2026-10-21). Step 1: the explicit execution mode
(`PipelineMode`), the late-recovery fix it exposed, and `ProviderInterface`.
Step 2: `RequireEdges` (`crates/core/src/require_edges.rs`), the import tags
that replace the dummy-span check, and `wakaru debug interop`; the
binding-use analyzers did not move. Step 3: barrier 0, the written-provider
decision, and the bundled-helper provider identity. Step 4 removed no pass:
each Phase 2 interop pass still serves inputs barrier 0 does not read (see
step 4 and [Done when](#done-when)). The maintainer decided the direction
(see [Decisions](#decisions)) and closed the line with that result. This proposal absorbs the consumer-aware provider format proposal
(deferred 2026-10-15), whose problem statement is
[Consumer writes to a named-only provider](#consumer-writes-to-a-named-only-provider).

## Problem

### Phase 1 guesses, Phase 2 repairs

In unpack, `UnEsm` decides each module's format and each require's import
form in Phase 1, where every module runs alone
([fact-system.md](../fact-system.md#shape)). Other modules' facts exist only
after the barrier. So Phase 1 guesses, and Phase 2 repairs the guess against
the providers it can now see: `provider_import_repair`,
`provider_namespace_repair`, `reexport_consolidation`, the lowered `import()`
restore, the webpack lazy-load restore, the bundled interop helper import,
`import_unproven_helper_requires`, `namespace_decomposition`,
`downgrade_unused_synthetic_imports`, and `run_kept_require_default_reads`
after the rule range.

Every one of these repairs the consumer. Where a gap needs the provider's
own decision to change, no Phase 2 pass can reach it: there is no transform
from ESM back to CommonJS, and none from CommonJS to ESM after the barrier.
Two gaps have that form.

#### Consumer writes to a named-only provider

Requiring a CommonJS module returns its `exports` object, and a consumer may
write to it:

```js
// provider
exports.f = function f() {};
// consumer
var p = require("./provider.js");
p.f = function patched() {};
```

The provider writes named exports only, so unpack recovers it as an ES
module (`export const f = …`). An ES module has no object a consumer can
write to: the module namespace is not extensible, and its properties cannot
be assigned from outside. The consumer then fails in one of two ways:

- **It becomes ESM.** `UnEsm` gives it `import p from "./provider.js"`.
  Phase 2 turns a default import of a provider with no default export into
  `import * as p`, but not when the binding is written, because a namespace
  cannot take the write. The default import stays, and the output fails to
  link (`does not provide an export named 'default'`) before any of the
  module graph that imports it runs.
- **It stays CommonJS** (it keeps another require). `require` of the
  provider now returns its namespace, so the write throws a TypeError in
  strict code and is lost in sloppy code.

In a sample of bundles from real websites, about one site in seven loads a
bundle whose unpack output breaks this way. Most writes come from a polyfill
library's internal modules, which replace a helper exported as `exports.f`;
others patch a utility module (`util.inherits = require("inherits")`).
Writes from code outside the input are a separate, accepted deviation
([`exports_object_closed_to_consumers`](../rewrite-assumptions.md#exports_object_closed_to_consumers)).
Unpack reports both failure modes today (`missing_imported_name`,
`commonjs_require_of_esm`).

The fix keeps a written provider CommonJS. Then a default import gets its
`module.exports`, `require` returns the same object, and every write lands
where it did before. That decision needs every consumer's writes before the
provider's `UnEsm` runs.

#### A provider whose helper lives in another module

tsc with `importHelpers` lowers `export { default } from "./widget"` to a
getter around tslib's `__importDefault`:

```js
const widget_1 = require("./widget");
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return tslib_1.__importDefault(widget_1).default; },
});
```

A single-file decompile proves the helper through `require("tslib")` and
writes `export { default } from "./widget"`. When a bundle carries tslib as
its own module, the helper is `require("./tslib.js")`. Phase 1 cannot prove
it, so `UnEsm` leaves the getter alone and keeps the provider CommonJS, and
no later stage turns it into ESM. Its consumer runs (Phase 2 imports the
CommonJS provider), but the provider's ES module form is lost.

### No single record of a module's interface

The same decisions are made in several places, each from its own reading of
the AST:

- **Format.** `UnEsm::visit_mut_module` makes several conversion attempts and
  rolls back to CommonJS when they fail; the export-storage gate sets
  `keep_commonjs`; `kept_require::blocking_require_calls` keeps a module
  CommonJS when a require would stay; Phase 1 predicts the late-recovered
  shape on a clone (`stays_commonjs`), and Phase 2 runs that recovery again
  on the retained AST with no check that both agree.
- **What `require(p)` returns.** `kept_require::provider_shape` and
  `required_value_import`, the `namespace_provider` closure in
  `provider_namespace_repair`, and the default-object gate in
  `provider_import_repair` each combine the `ModuleFacts` booleans
  (`commonjs_value_is_default`, `require_returns_exports_object`,
  `replaces_module_exports`, `marks_es_module`, `stays_commonjs`) their own
  way.
- **How a consumer uses a require.** `UnEsm` turns a whole require into a
  default import. Before step 2, how the consumer used it survived only as
  per-source lists collected before `UnEsm` (`whole_require_sources`,
  `interop_require_sources`, `wildcard_require_sources`), and Phase 2
  passes recognized a synthesized import by its dummy span. Each pass that
  reads a binding's uses brings its own analyzer instead of
  `analysis::BindingUseIndex`.
- **Cross-barrier contracts.** `blocking_require_calls` lists by hand the
  require forms that four Phase 2 passes accept. `run_un_esm` infers whether
  it runs in a single-file decompile from the stop rule.

So a new provider or consumer shape touches `UnEsm`,
`blocking_require_calls`, a collector, and one to three Phase 2 passes, and
nothing shows a reader which decision was made where, or why.

## Decisions

Recorded 2026-10-20 by the maintainer:

1. In unpack, `UnEsm` may read facts about other modules when it decides a
   module's format and a require's import form. It reads only facts
   observed on ASTs that exist when `UnEsm` runs, never a prediction of
   what a later rule will produce. A missing fact falls back to today's
   behavior: Phase 2 repairs.
2. The facts cross a new barrier that splits Phase 1 right before `UnEsm`
   (option A of the absorbed proposal).
3. Provider identity and consumer use are one line and cross the same
   barrier.
4. The model comes first. A barrier that carries more `ModuleFacts`
   booleans is ruled out.

### Rejected alternatives

- **Re-run written providers after the existing barrier** (option B of the
  absorbed proposal). It closes only the consumer-write gap, needs each
  re-run provider's input kept or parsed again, and makes every fact that
  depends on a re-run provider (a re-exporter's, for example) stale until
  refreshed.
- **A pre-pass over raw ASTs before Phase 1.** Formats without a prepared
  detector AST would parse twice, and a helper proved on the raw AST is a
  prediction of what the rule range will expose.
- **Run the whole pipeline to a fixed point, or replace the AST rule
  pipeline with a new IR.** The AST stays the place where shapes are
  recognized, bindings are proved, and edits are made. What changes is that
  it stops being the only record of cross-module meaning.

## Design

### The model

Three records, each with one owner:

- **`ProviderInterface`**, per module: how its CommonJS value relates to
  its exports (the value of `module.exports = v`, an `exports` object with
  named properties, replaced another way, or unknown), its named surface
  (known names, or open through `export *`), whether it marks itself
  `__esModule`, which transpiler helpers it exports and under which names,
  and whether any consumer needs its mutable `exports` object.
- **`RequireEdge`**, per `require()` call in a consumer: the source, the
  form of the call (a declarator, a member read, a helper argument, inline),
  the interop around it (none, default, wildcard, tested for `__esModule`),
  and how the value is used: whole value, static member reads (names),
  member writes (assignment, update, `delete`, `Object.assign` or
  `Object.defineProperty` target), receiver-sensitive calls, or escape.
- **The plan**: per module, the format chosen and why; per edge, the import
  form chosen (default, namespace, named, or a kept require with its read
  adapter) and why.

`ModuleFacts` keeps what it describes well, the recovered `import`/`export`
declarations. Its pre-`UnEsm` CommonJS booleans become inputs to
`ProviderInterface`, and its per-source consumer lists are replaced by
edges.

### The barrier before `UnEsm`

```
Phase 1a (per module, parallel):
    obtain resolved AST, normalize detector-proven runtime branches
    existing raw CommonJS collectors
    create RequireEdges with their interop, keyed by call span  ← new
    rule range up to, not including, UnEsm
    add each edge's uses and writes, by the same call span      ← new
    collect provider identity (helper exports) on this AST      ← new

──── barrier 0: ProviderInterfaces and the plan built from all modules ────

Phase 1b (per module, parallel):
    UnEsm, reading the plan                                  ← new input
    clone → late ESM recovery → collect_module_facts
    retain the barrier AST

──── barrier 1: as today ────

Phase 2: as today, reading the plan instead of re-deriving it
```

Edges are collected in two passes, and both observe an AST that exists at
that point; neither predicts a later rewrite.

- **Interop, on the input.** Rules before `UnEsm` erase it:
  `UnWebpackInterop` removes webpack's `.n` getter and `UnIndirectCall`
  removes `(0, x.f)()`, so on the AST `UnEsm` sees, a default-interop read
  and a whole-value use look the same. This is why
  `interop_require_sources` was collected before the rule range.
- **Uses and writes, on the AST `UnEsm` sees.** The extractor has already
  rewritten `t(100)` to `require("./module-100.js")`, and
  `SimplifySequence` has split comma sequences. Writes still sit anywhere:
  in chained assignments (`a.f = b.f = fn`), under `||`, in nested blocks
  and function bodies, and on an inlined `require("./p.js")` with no
  binding. The collector walks the whole tree.

The edge key is the `require()` call's span. A binding is not a stable key:
`UnWebpackInterop` renames before `UnEsm`, `UnEsm` renames on a collision,
and `SmartRename` runs later. The call's span survives the whole rule
range. When `UnEsm` turns a call into an import, it records each import
specifier's local binding, after its own renames, against the edge. The
barrier pre-passes read that table instead of the dummy span.

A hand-written specifier that `UnEsm` merges into a synthesized import is
recorded as merged, and the pre-passes still treat it as synthesized, as
the dummy span made them do. Step 2 kept that on purpose: its provider is
converted from CommonJS too, so a hand-written `import { name }` of a
`module.exports = { name }` provider stops linking unless
`provider_import_repair` rewrites it. A hand-written import of such a
provider with no require of the same source beside it is not repaired
today; whether every hand-written import of a converted provider should be
is open.

A detector that builds the module AST itself (a prepared AST) can write
`require()` calls with a dummy or shared span. Such a call keeps what the
input pass read, so the per-source questions still count it, but it has no
key, no uses, and no tag key. The Turbopack translator gives each require
it translates the span of the input call it replaces; what stays without a
key is a require in a synthesized facade or loader module, and the clones
of the inner call that the webpack extractors make when they inline
`require.n(require(x))`.

Provider identity needs more than today's helper facts, which read the
recovered ESM exports (`collect_helper_exports` on `ExportFact`). Before
`UnEsm`:

- the helper body is recognized by shape (`LocalHelperContext`), Terser's
  renamed locals included;
- the export name that reaches it needs a new reader: the export-storage
  analysis covers `require.d` getters once `require.r(exports)` is stripped
  as `UnEsm` strips it, and `module.exports = fn` (with
  `module.exports.default = module.exports`) needs the whole-value
  collector;
- the plan carries each export's helper kind. Unlike
  `require("tslib")`, a bundled provider is not trusted by name.

`UnEsm` reads the plan through `RuleRunContext`. With a plan entry it keeps a
written provider CommonJS and proves a helper through its provider's
recorded identity, as it does for `require("tslib")` today. Without one it
behaves as it does now.

Both AST paths need the plan. The retained-AST path runs Phase 1b on the
Phase 1a AST. The source-map path discards the Phase 1 AST and runs the
through-`UnEsm` range again in Phase 2 (`phases.rs`, the
`parse_js_with_recovery` branch); that run must read the same plan.

What barrier 0 cannot see stays with Phase 2:

- interfaces exposed only by late ESM recovery of factory IIFEs, which runs
  after `UnEsm`;
- helper exports visible only after the helper provider's own `UnEsm`;
- a UMD helper build that exports through a callback
  (`exporter("__importDefault", __importDefault)`), which no stage proves
  today.

### Explicit execution mode

`run_un_esm` tells a single-file decompile from unpack Phase 1 by the stop
rule (`stop_after != Some("UnEsm")`). Barrier 0 stops the range before
`UnEsm`, so that inference breaks. Replace it with an explicit mode on the
pipeline options: single file, unpack Phase 1b, unpack late recovery, raw.
Each mode states which later stages will still run, which is what
`later_unpack_stages` approximates today. Late recovery in Phase 2 runs
after the cross-module passes it would defer to; a test should pin what
`UnEsm` assumes there.

Step 1 found that the late recovery assumed every later stage still ran, so
a module first made ESM there could keep a `require` no stage takes. Now
`PipelineMode` (`SingleFile`, `UnpackPhase1`, `UnpackLatePrediction`,
`UnpackPhase2`, `UnpackRaw`) maps to `kept_require::LaterUnpackStages`:
all stages in Phase 1, none in a single file, raw output, or Phase 2's late
recovery, and only the lazy-load restore, which takes a load in a CommonJS
module, for Phase 1's prediction of that recovery. A prediction with no
stage would disagree with Phase 2 on a module whose only such require is a
lazy load. Barrier 0 adds a mode for Phase 1a, which does not run `UnEsm`.

### A provider passed to a function

Decided 2026-10-20 by the maintainer. `install(prov)` with
`function install(m) { m.f = … }` keeps the call in unminified output, and
Terser can inline it into a direct write, so counting only direct writes
would give the same source a different format per producer.

- **Counts as a write**: the callee is a function in the same module,
  declared once, never reassigned, and not reading `arguments` (the
  conditions of `Evidence::callee_params` in `rules/arrow_function.rs`),
  and it writes that parameter: a member assignment, update, or `delete`,
  or the target of `Object.assign` / `Object.defineProperty`. The provider
  stays CommonJS.
- **Does not count**: interop and re-export helpers, builtins that only
  read (`Object.keys`), same-module functions that only read the
  parameter, unknown callees, and callees in other modules. The provider
  becomes ESM as today; unknown and cross-module callees fall under the
  same accepted deviation as writes from outside the input.
- **Warning**: a named-only provider passed to an unknown or cross-module
  callee gets a non-error warning, since a write there breaks at runtime
  and nothing reports it today.

A write that works at runtime proves the provider's value was a mutable
CommonJS `exports` object (an ES module namespace and webpack's
getter-only exports reject writes), so keeping it CommonJS loses no
correct recovery. Passing a value to a function proves nothing: ES modules
pass namespaces to `Object.keys` and registries. Counting every escape as
a write was rejected: in a sample of bundles from real websites it kept
providers CommonJS with no write behind any of them.

## Steps

1. **One provider derivation.** Build `ProviderInterface` from `ModuleFacts`
   at barrier 1. `kept_require`, `provider_namespace_repair`, and
   `provider_import_repair` read it instead of combining booleans. Add the
   explicit execution mode. No output change: unit tests and the fixture
   check stay unchanged.
2. **Edge records.** Collect `RequireEdge`s in the two passes above.
   `UnEsm` tags each import it synthesizes with its edge, and Phase 2
   passes read the edge instead of the dummy span and the per-source lists.
   Some readers still need the current AST (an alias's lifetime, a use
   count, the site to rewrite); they keep reading it. Passes that
   read a binding's uses move to `BindingUseIndex`. Add a debug command
   that prints each module's interface and each edge's plan with the
   reason, like `wakaru debug cjs-exports`. No output change.

   Done as planned, except: a call with a dummy or shared span keeps its
   input-pass record (prepared detector ASTs write such calls, and dropping
   them would change the per-source answers); hand-written specifiers
   merged into a synthesized import still count as synthesized (see
   [The barrier before `UnEsm`](#the-barrier-before-unesm)); and no
   analyzer moved to `BindingUseIndex`: it has no kind for a member call,
   an optional chain, the position of a whole-value read (callee, argument,
   alias initializer), or the name of a `delete` target, which the
   analyzers tell apart. The cross-module interop import pass and namespace
   decomposition carry tags to the imports they build; the default-object
   composition pass and `import_unproven_helper_requires` do not.
3. **Barrier 0.** Split Phase 1, build the plan at barrier 0, and let
   `UnEsm` read it. Output changes: written providers stay CommonJS, and
   providers whose helper lives in another module recover their ES module
   form. Measure wall time and peak RSS before and after
   ([performance learnings](../learnings/performance-round-2026-09.md) set
   the measurement rules). Some cost is an accepted tradeoff; see
   [Done when](#done-when).

   First half done: Phase 1 split at `UnEsm` (`run_rules_before_un_esm`,
   `run_un_esm_range`), the uses of every edge read at the end of Phase 1a,
   and `InterfacePlan` built at barrier 0. `UnEsm` keeps a written provider
   CommonJS, in Phase 1b, the late ESM recovery, and the source-map rerun.
   The escape policy is implemented as decided, and
   `commonjs_exports_escape` reports a namespace provider passed to a
   callee in another module (not a transpiler helper module) or an unknown
   one. Barrier 0 needs no new `PipelineMode`: the mode matters only where
   `UnEsm` runs. `LocalHelperContext` is no longer shared between the rules
   before `UnEsm` and `UnEsm`; Phase 1a builds one for the edge uses and
   `UnEsm` builds its own. A webpack harmony provider stays ESM when written;
   adding a new property to its exports object works in the bundle, so that
   case is a known gap ([unpacking.md](../unpacking.md#known-gaps)).

   Second half done: provider identity. Phase 1a reads each module's
   interop helper exports (`ProviderHelpers`) on the AST `UnEsm` will see:
   the export storage analysis, run without the `__esModule` markers, maps
   each export to a function declared once and never written, or the
   `module.exports = v` value with its Babel runtime `default` mirror, and
   the interop-only body matchers classify it. The plan carries each
   unwritten provider's helpers and, per consumer, the sources that reach
   them. `run_un_esm` adds them to the `LocalHelperContext` it gives
   `UnEsm` (`with_bundled_helpers`), per export and kind: a namespace
   binding's member, the inlined `require("./m.js").name(…)`, and a
   provider that is the helper. A getter that re-exports through a bundled
   tslib's `__importDefault` now recovers as `export { default } from`
   (webpack 5, webpack 5 + Terser, webpack 4, rspack), and a consumer of a
   bundled Babel runtime or `@swc/helpers` interop helper converts in
   Phase 1b. The `commonjs_exports_escape` exemption now needs a proven
   interop helper export instead of any export of a helper module. Only
   default and wildcard interop helpers are carried; tslib's `__exportStar`,
   the UMD `tslib.js` exporter callback, and a helper module written as ESM
   stay unproven ([unpacking.md](../unpacking.md#known-gaps)).
4. **Remove what barrier 0 makes redundant.** Candidates, each to verify:
   the default-versus-namespace guess in `provider_namespace_repair`, the
   bundled interop helper import, the lowered `import()` restore. A pass
   still needed for late-recovered interfaces stays.

   Measured after step 3 by counting the modules each pass changes, with
   and without the bundled-helper proof: on the private fixtures and the
   artificial cases, the lowered `import()` restore no longer changed any
   module, and the bundled interop helper import changed only consumers of
   `@babel/runtime`'s versioned `interopRequireWildcard`, whose module
   replaces `module.exports` and its own binding on the first call. Barrier
   0 now proves that module by the exact replacement (`SelfReplacement` in
   `provider_helpers.rs`: the installed copy has the helper's body, the
   setup before it has no outside effect, and the binding has no other
   write).

   Measured again with that proof, on the same inputs: neither the lowered
   `import()` restore nor the bundled interop helper import changes a
   module (without the proof both still do); `import_unproven_helper_requires`
   changes only consumers of a CommonJS function that is no interop helper
   (`var x = wrap(require("./p"))`); `provider_namespace_repair` changes
   as many modules in the fixtures as without the proof, and a few fewer in
   the artificial cases, where the proof converts some consumers in Phase 1b.

   Step 4 removed nothing. Each pass still accepts an input no earlier stage
   handles, recorded in its doc comment and in
   [fact-system.md](../fact-system.md):

   - The lowered `import()` restore and the bundled interop helper import
     prove a helper from the facts after the provider's own `UnEsm`. Barrier
     0 does not read a helper module the input wrote as ESM (tslib's ES
     module build passed as another input, pinned by the multi-file unpack
     tests), a helper export only the late ESM recovery exposes, or an
     export storage it cannot map. Without the passes such a consumer
     becomes ESM around a `require`. Removing them trades that recovery
     away, which is a maintainer decision; reading ESM helper modules at
     barrier 0 would leave them only the late-recovered and unmapped
     exports.
   - `import_unproven_helper_requires` serves wrappers that are no helper.
   - `provider_namespace_repair`: barrier 0 moved no part of its
     default-versus-namespace decision before `UnEsm`. Whether a provider
     ends with a default export is known only after its own `UnEsm` and late
     recovery, which the plan does not carry.

   `kept_require::blocking_require_calls` and `LaterUnpackStages` are
   unchanged, as every stage they list still runs. No ordering edge changed
   in the [rule dependency inventory](../rule-dependency-inventory.md).

## Done when

- `missing_imported_name` and the `commonjs_require_of_esm` write reason no
  longer report a consumer that writes to a named-only provider in the
  input.
- A re-export through a bundled helper provider recovers as
  `export { default } from`.
- Fewer Phase 2 interop passes after step 4 than before step 3: not met,
  and accepted when the line closed: barrier 0 reads only what exists
  before `UnEsm`, so the Phase 2 passes keep the inputs it cannot see. The
  lowered `import()` restore and the bundled interop helper import serve
  helper modules the input wrote as ESM and helpers exposed only by late
  recovery; `import_unproven_helper_requires` serves wrappers that are not
  interop helpers; `provider_namespace_repair` decides default versus
  namespace from facts that exist only after the provider's own `UnEsm`.
  Reading ESM-input helper modules at barrier 0 would shrink the first two,
  not remove them.
- Barrier 0 does not multiply wall time or peak RSS. A cost short of that
  is a tradeoff the maintainer weighs against the recovered output, not a
  regression to remove. A cost of several times is grounds to revert it and
  record why in `docs/learnings/`.

## Open questions

- **Barrier 0 cost.** Every module waits for the slowest to reach `UnEsm`;
  the largest-first schedule limits but does not remove that. On the
  source-map path, ASTs that Phase 1 discards today must survive barrier 0.

## Not in scope

- Consumers outside the input
  ([`exports_object_closed_to_consumers`](../rewrite-assumptions.md#exports_object_closed_to_consumers)).
- A provider object that escapes into an unknown call that might write to
  it.
- Opaque webpack factories whose `exports` parameter is reassigned; they
  never reach `UnEsm`.
