# Cross-Module Fact System

## What it is

A barrier-and-read mechanism that lets Phase 2 rules read import/export shape
from **other** modules in the same bundle. Used today by
`namespace_decomposition`; designed to be extended by further cross-module
rules.

## Why it's simpler than the original proposal

The original design imagined rules writing per-module "observations" into
shared state, merging them at a barrier, and reading back immutable facts.
We do not need that. After `UnEsm` runs, ESM `import`/`export` declarations are
already a normalized, AST-level representation of module shape. That AST *is*
the fact. Fact extraction is then a pure function of the module — no rule-
written observations, no merge step.

## Shape

Multi-module unpack runs in two parallel phases with a single barrier between
them (`src/driver.rs::unpack_multi_module`):

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

`src/facts.rs`:

- `ImportFact { local, source, kind: Default | Namespace | Named(imported) }`
- `ExportFact { exported, local, kind: Default | Named }`
- `ModuleFacts { imports, exports }`
- `ModuleFactsMap` — keyed by normalized module specifier
  (handles `./foo`, `foo`, `foo.js` variants)

Extraction (`collect_module_facts`) reads the post-Stage-2 AST and returns
these structures. No mutation, no shared state.

## Rules that read facts

- **`namespace_decomposition`** — rewrites `import r from "./x"; r.foo()` into
  `import { foo } from "./x"; foo()` when `./x` exports `foo` and no collision
  prevents the rewrite. Handles aliased pre-existing specifiers, inner-scope
  shadowing, mixed default+named imports, and readability backoff when too many
  collisions would force aliasing.

## Adding a new fact-reading rule

1. Put the rule in `src/` as a free function taking
   `(&mut Module, &ModuleFactsMap)`.
2. Call it from `unpack_multi_module` between `apply_rules_until("UnEsm")` and
   `apply_rules_between("UnTemplateLiteral", …)`.
3. Do all AST mutation locally to the module — never write back to
   `ModuleFactsMap`.
4. Add unit tests following `tests/namespace_decomposition_rule.rs` (use
   `facts_for(source)` to synthesize a target module's facts).

## Non-goals

- No shared mutable state between rules in the same phase.
- No multi-round merging.
- No speculative facts ("this might be an X"). A fact holds iff the post-Stage-2
  AST says it does.

Rules that need heavier semantic conclusions (e.g. "this namespace projection
is always equivalent to a direct import binding") should derive them inside the
rule from the facts they read — not emit them back into the map.
