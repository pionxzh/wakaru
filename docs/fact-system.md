# Cross-Module Fact System

See also: [Architecture](architecture.md) for the multi-module pipeline design,
[Rule dependency inventory](rule-dependency-inventory.md) for where fact-reading
rules fit in the pipeline.

## What it is

A barrier-and-read mechanism that lets Phase 2 rules read provider shape from
**other** modules in the same bundle. Most facts describe normalized ESM
imports/exports. Two deliberately narrow pre-`UnEsm` facts preserve CommonJS
provider shape that `UnEsm` would otherwise erase: the proven identity and
statically declared properties of an object assigned directly to
`module.exports`, and positively observed properties attached to a stable
callable before it becomes `module.exports`. For object defaults, the identity
proof is independent of the property list: an empty list may describe a proven
empty object rather than an unknown value. An empty callable-property list is
not such a proof.

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

Multi-module unpack runs in two parallel phases with a single barrier between
them (`crates/core/src/driver/unpack.rs::unpack_multi_module`):

```
Phase 1 (per module, parallel):
    obtain resolved AST (prepared detector AST, or parse → resolver)
    normalize exact detector-proven webpack runtime branches
    collect raw CommonJS default-object / callable-property facts
    rule range through UnEsm
    clone barrier AST → recover webpack factory IIFE ESM shapes
    collect_module_facts(&facts_clone)                ← pure AST → facts
    retain original barrier AST + Globals + unresolved mark

──── barrier: ModuleFactsMap assembled from all modules ────

Phase 2 (per module, parallel):
    resume retained barrier AST
    run_provider_import_repair(&mut module, facts)    ← proven CJS property edges
    run_provider_namespace_repair(&mut module, facts) ← proven ESM namespace edges
    run_reexport_consolidation(&mut module, facts)
    run_namespace_decomposition(&mut module, facts)  ← reads cross-module facts
    downgrade_unused_synthetic_imports(&mut module)  ← preserve require effects
    registry rule range resuming after UnEsm, through UnReturn
    targeted late cleanup/recovery
```

The normal no-source-map path runs the through-`UnEsm` range once. The retained
AST crosses the barrier together with the exact `Globals` and unresolved mark
that produced its `SyntaxContext`s, so downstream ctxt-sensitive rules keep a
continuous binding identity. Webpack5 and Browserify-family detectors can enter
Phase 1 with a resolved AST produced by detector normalization, avoiding an
emit → parse → resolver round trip. Source-map mode materializes that private
AST sidecar and follows the parser path because emitted mappings need
parser-owned module coordinates.

A structurally identified numeric-ID webpack factory whose reused loader
parameter cannot be localized safely is deliberately absent from this fact
system. Phase 1 inserts empty facts and a stable
`webpack_factory_recovery_failed` diagnostic; Phase 2 returns the raw extracted
body without parsing or transforming it. Its ID is also excluded from
synthesized module-edge maps, so neither that opaque provider nor its consumers
can create a false cross-module fact. The missing import report makes
dead-module elimination conservatively retain the graph. Named-ID containers
retain whole-input fallback because a path-like unresolved call is not safely
distinguishable from an authored ESM dependency downstream.

## Facts

`crates/core/src/facts.rs`:

- `ImportFact { local, source, kind: Default | Namespace | Named(imported) }`
- `ExportFact { exported, local, kind: Default | Named }`
- `HelperExportFact { exported, local, kind }`
- `ModuleFacts { imports, exports, helper_exports,
  commonjs_default_object, commonjs_default_attached_properties,
  has_export_all, ts_helper_exports,
  ts_helper_namespace_factory_exports, passthrough_target }`
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

`collect_commonjs_default_attached_properties` separately records static
identifier properties assigned directly and unconditionally to a stable
top-level function before the sole direct `module.exports = functionBinding`
assignment. It is a positive-membership fact only: a recorded property can be
repaired, but an absent property says nothing about the callable's runtime
surface. Computed, conditional, nested, post-export, or reassigned shapes fail
closed. Neither collector mutates the AST or shared state.

Normal processing also restores webpack's runtime-created `module.exports = {}`
only when structural webpack detection proves that a normalized extracted
factory body is empty. That synthetic statement passes through the ordinary
`UnEsm` path, while `--raw` keeps the detector's empty-body passthrough
unchanged. Non-empty factories are not generalized from this narrow runtime
fact.

The same detector-owned boundary permits a normal-only webpack runtime
normalizer for two exact inner-UMD expressions whose CommonJS branch is
provable only because webpack initializes every factory's `module.exports`.
It selects a truthy `module.exports` arm, or recovers an undefined-guarded
`factory.apply(exports, [])` assignment only when the immediate factory returns
a stable, non-reassigned function binding. The module shell must be exact,
exactly one rewrite must match, and no unresolved CommonJS runtime references
may remain. Synthetic children from recursive splitting do not inherit the
proof; `--raw` remains detector passthrough.

Helper export facts are still pure AST facts. They only record helper identity
when the exported local binding matches a known helper body shape or runtime
export shape after Stage 2. They do not speculate from consumer-side usage.

## Rules that read facts

- **`provider_import_repair`** — repairs only dummy-span imports synthesized by
  `UnEsm` for `require("./x").name`. If provider facts prove that the raw
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
- **`provider_namespace_repair`** — changes a dummy-span default import
  synthesized for a whole-object `require("./x")` into a namespace import when
  the provider facts prove a named or `export *` surface and no default export.
  It accepts static member reads, `Object.keys(namespace)`, and namespace values
  used as `Object.assign` sources. Simple local aliases preserve that proof; an
  exact, unconditional top-level replacement assignment ends an alias's
  namespace lifetime after its right-hand side is evaluated. Hoisted function
  declarations are always checked against the original lifetime regardless of
  textual position. Authored imports, namespace mutation, conditional/nested
  alias replacement, arbitrary value escape, computed/meta reads, and
  `__esModule` observation remain unchanged.
  The existing namespace decomposition pass can then recover narrower named
  imports where its own gates allow that rewrite.
- **`namespace_decomposition`** — rewrites `import r from "./x"; r.foo()` into
  `import { foo } from "./x"; foo()` when `./x` exports `foo` and no collision
  prevents the rewrite. Handles aliased pre-existing specifiers, inner-scope
  shadowing, mixed default+named imports, and readability backoff when too many
  collisions would force aliasing. For imports synthesized from `require()`, it
  also discards an otherwise-inert top-level binding read left by interop-helper
  removal when the original namespace/default specifier can be removed in full;
  substantive whole-object reads and every write still fail closed.
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
  miss yours, leaving an undefined reference. For newly-created import
  specifiers, `SyntaxContext::empty()` on both binding and usage is fine (they
  match each other and the resolver isn't re-run).

## Non-goals

- No shared mutable state between rules in the same phase.
- No multi-round merging.
- No speculative facts ("this might be an X"). A fact holds iff the normalized
  post-Stage-2 AST says it does, or the narrow pre-`UnEsm` collector proves the
  exact raw CommonJS assignment shape.

Rules that need heavier semantic conclusions (e.g. "this namespace projection
is always equivalent to a direct import binding") should derive them inside the
rule from the facts they read — not emit them back into the map.
