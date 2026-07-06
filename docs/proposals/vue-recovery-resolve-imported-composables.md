# Proposal: Resolver-back imported composable ASTs (retire the last hand-rolled shadow stack)

Status: **DEFERRED — not started.** Independent of the Phase 4 AST-IR proposal
([vue-recovery-ast-ir.md](vue-recovery-ast-ir.md)); no ordering constraint either
way. This closes the one remaining gap in the resolver-backed Vue recovery
redesign (issue #196): identifier matching is `SyntaxContext`-gated everywhere
*except* on imported composable module sources.

## Problem

After Phases 0–3 / 2a / 2b, every recovery collector keys on `(name, ctxt)` and
the hand-rolled `ScopeStack` is gone — **except** the collectors that run on
*imported* composable sources. `composable_ref_props_from_source`
(`crates/core/src/vue_recovery/imports.rs`) parses imported module text with
`parse_module` and **does not run SWC's `resolver()`** (neither the direct parse
nor the `unpack_bundle` fallback path). Those ASTs therefore carry empty
`SyntaxContext`s.

Consequences:
- `StrongValueMemberCollector` keeps its own hand-rolled shadow stack
  (`shadowed: Vec<HashSet<Atom>>`) — the **last hand-rolled scope tracking** in
  recovery — and `RefLocalCollector` / `collect_object_pat_ref_bindings` /
  `collect_pat_ref_bindings` match by name.
- Latent correctness gap: on empty-context ASTs, identity is name + a coarse
  shadow stack rather than resolver's precise per-scope contexts. The stack
  handles the common cases, but it is weaker than the `(name, ctxt)` identity used
  on the (resolver-processed) local composable and render paths.

## Proposed change

1. **Run `resolver()` on imported composable ASTs** at parse time in
   `composable_ref_props_from_source` — both the direct `parse_module` and the
   `unpack_bundle` module path — inside a `GLOBALS.set`, mirroring the recovery
   entry points in `vue_recovery.rs`. Real per-scope contexts; behavior-preserving
   for the existing name-based analyses that also run on this AST.
2. **Convert `StrongValueMemberCollector` to `(name, ctxt)`:** drop `shadowed` +
   `is_shadowed`/`enter_shadowed`/`exit_shadowed` + the `visit_block_stmt` /
   `visit_function` / `visit_arrow_expr` scope overrides; `value_member_object`
   returns `(Atom, SyntaxContext)`; collect `(base.sym, base.ctxt)`
   unconditionally — shadowing falls out of context identity, exactly like the
   render-side member collectors converted in Phase 2a.
3. **Thread `(name, ctxt)` through the consumers:**
   `function_strong_value_member_bindings` return type,
   `RefLocalCollector.strong_value_member_refs`, the `visit_var_declarator` match
   (`binding.id.sym`, `binding.id.ctxt`), and `collect_object_pat_ref_bindings` /
   `collect_pat_ref_bindings`.
4. **Remove the now-dead helpers:** `block_shadowed_bindings`,
   `collect_decl_bound_atoms`, `pat_bound_atoms`, `collect_pat_bound_atoms`.

## Prior attempt (reverted — read before retrying)

Steps 2–4 were prototyped in the Phase 2b session **without step 1** and reverted.
Without `resolver()` on the imported AST, `(name, ctxt)` degenerates to
name-matching (every ident has the empty context), so a
`subscribe((itemList) => { itemList.value.push(...) })` callback param was
conflated with a top-level `itemList`, wrongly classifying it as a ref. This is
caught by the regression test
`preserves_imported_composable_shadowed_callback_value_members`
(`vue_recovery/tests.rs`). The collector conversion (steps 2–4) is correct **once
step 1 lands**; reconstruct that diff from git history if useful. **Do step 1
first.**

## Size / risk

Small–medium, self-contained. The collector conversion is mechanical (mirrors
Phase 2a). The resolver-wrapping is a few lines but touches the **shared** imported
composable parse path: every imported-composable analysis
(`composable_ref_props_from_module`, `local_injected_composable_ref_props`,
ref-returning-function detection, …) runs on that AST. They match by name, so
adding contexts should be behavior-preserving — the full core suite (many
composable-ref tests) is the guard.

## ROI / when to do it

Low priority. It retires the last hand-rolled shadow tracking and makes imported
composable analysis ctxt-safe (a latent correctness improvement), but it is
cleanup on an experimental feature and the shadow stack works today. Worth doing
if a shadow bug surfaces in imported composable ref detection, or as part of a
"fully ctxt-safe recovery" / promote-`--vue-sfc`-out-of-experimental push.

## References

- Binding-identity pattern: `crates/core/src/rules/rename_utils.rs`
  (`BindingRenamer`) and the Phase 2a render-side member collectors in
  `crates/core/src/vue_recovery/context.rs`.
- The separate, unrelated cleanup: Phase 4 —
  [vue-recovery-ast-ir.md](vue-recovery-ast-ir.md).
- issue #196 sequencing plan.
