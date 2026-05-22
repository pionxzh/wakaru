# Cross-Module Fact System

See also: [Architecture](architecture.md) for the multi-module pipeline design,
[Rule dependency inventory](rule-dependency-inventory.md) for where fact-reading
rules fit in the pipeline.

## What it is

A barrier-and-read mechanism that lets Phase 2 rules read import/export shape
from **other** modules in the same bundle. Used today by
`namespace_decomposition` and helper recovery rules that need to prove a helper
identity across module boundaries.

## Why it's simpler than the original proposal

The original design imagined rules writing per-module "observations" into
shared state, merging them at a barrier, and reading back immutable facts.
We do not need that. After `UnEsm` runs, ESM `import`/`export` declarations are
already a normalized, AST-level representation of module shape. That AST *is*
the fact. Fact extraction is then a pure function of the module — no rule-
written observations, no merge step.

## Shape

Multi-module unpack runs in two parallel phases with a single barrier between
them (`crates/core/src/driver.rs::unpack_multi_module`):

```
Phase 1 (per module, parallel):
    parse → resolver → Stage 1 + Stage 2 (UnEsm etc.)
    collect_module_facts(&module)                    ← pure AST → facts
    AST discarded

──── barrier: ModuleFactsMap assembled from all modules ────

Phase 2 (per module, parallel):
    parse → resolver → Stage 1 + Stage 2
    run_namespace_decomposition(&mut module, facts)  ← reads cross-module facts
    Stage 3+
```

Stage 1+2 runs twice per module — the first pass feeds fact extraction, the
second runs the real pipeline. Re-parsing is required because SWC's
`SyntaxContext` must remain continuous across the entire pipeline; reusing the
Phase 1 AST would break downstream ctxt-sensitive rules.

## Facts

`crates/core/src/facts.rs`:

- `ImportFact { local, source, kind: Default | Namespace | Named(imported) }`
- `ExportFact { exported, local, kind: Default | Named }`
- `HelperExportFact { exported, local, kind }`
- `ModuleFacts { imports, exports, helper_exports, passthrough_target }`
- `ModuleFactsMap` — keyed by normalized module specifier
  (handles `./foo`, `foo`, `foo.js` variants)

Extraction (`collect_module_facts`) reads the post-Stage-2 AST and returns
these structures. No mutation, no shared state.

Helper export facts are still pure AST facts. They only record helper identity
when the exported local binding matches a known helper body shape or runtime
export shape after Stage 2. They do not speculate from consumer-side usage.

## Rules that read facts

- **`namespace_decomposition`** — rewrites `import r from "./x"; r.foo()` into
  `import { foo } from "./x"; foo()` when `./x` exports `foo` and no collision
  prevents the rewrite. Handles aliased pre-existing specifiers, inner-scope
  shadowing, mixed default+named imports, and readability backoff when too many
  collisions would force aliasing.
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

## Adding a new fact-reading rule

For a cross-module late pass that naturally runs at the Stage 2 barrier:

1. Put the pass in `crates/core/src/` as a free function taking
   `(&mut Module, &ModuleFactsMap)`.
2. Call it from `unpack_multi_module` between `apply_rules_until("UnEsm")` and
   the Stage 3+ rule range.
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
- No speculative facts ("this might be an X"). A fact holds iff the post-Stage-2
  AST says it does.

Rules that need heavier semantic conclusions (e.g. "this namespace projection
is always equivalent to a direct import binding") should derive them inside the
rule from the facts they read — not emit them back into the map.
