# Proposal: Carry the resolved AST in the Vue recovery IR (delete the string lexers)

Status: **DEFERRED — not started.** This is "Phase 4" of the resolver-backed Vue
recovery redesign (issue #196). Phases 0–3 and 2a/2b landed: identifier matching
in `crates/core/src/vue_recovery/` is now fully `SyntaxContext`-gated (helper
recognition, alias/props renaming via `rename_utils::BindingRenamer`, and every
reference collector key on `(name, ctxt)`); the hand-rolled `ScopeStack` and the
two banned `sym`-only renamers are gone. What remains is the last piece of string
machinery, and it is **deferred on ROI grounds** (see "Why deferred"). This doc
records the scope, the traps found while scoping it, and the triggers that would
make it worth doing — so a future evaluation does not start from scratch.

## Problem

The recovery IR (`VueExpr` in `crates/core/src/vue_template.rs`) stores template
expressions as **printed strings**, not ASTs. Everything that needs to reason
about an expression's identifiers therefore re-derives structure from text:

- `crates/core/src/vue_recovery/js_refs.rs` (~288 lines) is a bespoke JS
  lexer/scanner used to collect identifier references and "read" references out
  of printed expressions (template usage analysis, setup dependency ordering).
- `rename_expr_prefix` / `rename_code_segment` in `vue_template.rs` rename an
  identifier prefix inside a printed expression via string scanning (with a best
  effort AST path that falls back to the string scanner).
- `clean_expr` (`vue_recovery/expressions.rs`) applies two **string-level**
  transforms *after* `print_expr` has already produced the string:
  `inline_setup_value_bindings` (inline a setup value binding's initializer for
  `binding.value` reads) and the unref `strip_callee_wrappers` (`unref(x)` → `x`).
- Several synthetic re-parses (`const __wakaru_expr = …`) exist only to recover
  an AST from a string that was printed moments earlier.

This works — its bugs were closed point-wise during the original recovery review
cycles — but it is a parallel, weaker JS front-end running on printed text. The
string scanners hand-handle template-literal nesting, regex-vs-division, quotes,
and unicode idents; those are exactly the cases an AST gets for free.

## Proposed end state

`VueExpr` carries the resolved `Expr` alongside its printed string, following the
existing precedent `VueSetupValueBinding { value: String, expr: Option<Expr> }`
(`vue_recovery/locals.rs`). Reference collection and prefix renaming run on the
carried AST; the emitter still reads only `as_str()`, so the emitter boundary
stays AST-free. `js_refs.rs`, `rename_code_segment`, and the re-parse helpers are
deleted.

## Scope and sequencing

Two units. Do them as separate, individually-green commits — not one pass.

### Phase 4pre — move the string-level expr transforms to AST passes
Prerequisite: today `clean_expr` mutates the *string* after `print_expr`, so a
carried AST captured at `print_expr` time does **not** match the final `printed`
form. Until these move to AST-level `VisitMut` passes inside `print_expr`, a
carried AST cannot faithfully back ref-collection or renaming.
- Unref `strip_callee_wrappers` (`unref(x)` → `x`): small. An AST replace; the
  re-print handles operator precedence, deleting the bespoke precedence handling.
- `inline_setup_value_bindings`: the large one. It is a ~200-line string
  subsystem (`replace_setup_value_bindings_once`, template-literal scanners,
  regex-vs-division detection, `expr_binds_any_name`,
  `setup_value_can_inline_in_expr`). The AST version is conceptually simpler and
  more correct, but is a real rewrite that must preserve behavior across the
  snapshot suite. The value binding's AST is already available
  (`VueSetupValueBinding.expr`).

### Phase 4 — carry the AST in the IR, delete the lexer
- Change `VueExpr` from `String` to `{ printed: String, expr: Option<Expr> }`.
  Hand-implement `PartialEq`/`Eq` comparing only `printed` (`Expr` is not `Eq`),
  so the IR-tree equality used throughout the emitter tests still works.
  `VueExpr::new(String)` stays for emitter tests (`expr: None`); recognition
  populates `Some(cleaned_expr.clone())` from the now-fully-AST `print_expr`.
- Move template ref-collection (`vue_recovery/usage.rs`) and shadow analysis onto
  the carried AST (a scope-aware free-variable walk replaces the `js_refs`
  string scan). **Reuse the existing pattern:** `declared_binding_idents` +
  `IdentRefCollector` in `vue_recovery/context.rs` already do exactly this for the
  script side. Gotcha carried over from Phase 2b: key the declared-binding set on
  the `(name, ctxt)` **pair, not `ctxt` alone** — `resolver()` assigns one context
  per *scope*, so sibling bindings share a context; keying on ctxt alone silently
  drops cross-binding references (it caused 8 snapshot failures in 2b).
- Move `replace_prefix` (`vue_template.rs`) onto the carried AST via a
  `BindingRenamer`-style ctxt rename, then re-print.
- Delete `js_refs.rs`, `rename_code_segment` + its scanners, and the re-parse
  helpers (`parse_printed_vue_expr` in `attrs.rs`, `expr_binds_any_name`, the
  string fallback in `rename_expr_prefix_with_ast`).

Suggested sub-steps (each green): 4pre-unref → 4pre-inline → `VueExpr` struct as a
no-behavior-change foundation → `usage.rs` onto AST → `replace_prefix` onto AST →
delete `js_refs`.

## Scope traps found while scoping (read before estimating)

1. **"Delete `js_refs.rs`" is bigger than `VueExpr`.** Its callers in
   `vue_recovery/locals.rs` and `declarations.rs` collect refs from **plain
   `String` fields on other binding types** — `VueSetupScriptBinding.value`,
   `VueScriptSetupDeclaration.source`, `VueSetupRefBinding.expr` — none of which
   carry an AST today. Fully removing `js_refs` means carrying an `Expr` on those
   types too, or leaving those callers string-based (so `js_refs` survives
   partially). The plan's "~9 `VueExpr` sites" undercounts; budget for the extra
   binding types.
2. **`clean_expr` runs after `print_expr`** (the Phase 4pre prerequisite above).
   Skipping 4pre and capturing the AST at `print_expr` time yields an AST that
   disagrees with `printed`, silently corrupting ref-collection.
3. **`VueExpr` is compared with `==` all over the emitter tests.** Adding a
   non-`Eq` `Expr` field forces a manual `PartialEq`/`Eq` (compare `printed`),
   not a derive.

## Cost / size

**Big — the largest phase.** 4pre ≈ a medium phase (dominated by the
`inline_setup_value_bindings` rewrite); Phase 4 ≈ 1.5–2× the size of the Phase 2a
collector conversion, spread across every `VueExpr` producer (`attrs.rs`,
`nodes.rs`, `slots.rs`) and consumer (`usage.rs`, `selection.rs`,
`setup_bindings.rs`, `locals.rs`, `declarations.rs`, `expressions.rs`). Highest
regression surface of any phase: the payoff is validated only by the snapshot
suite staying green, so a drift is an "investigate a snapshot diff" task rather
than a compile error.

## Why deferred (ROI)

- **No correctness or user-facing benefit.** The correctness bug class (helper
  shadowing) was fixed in Phase 1; the scope-machinery deviation was removed in
  Phase 2. This is pure internal cleanup on an **experimental** feature
  (`--vue-sfc`).
- **The lexer works.** Its edge cases (unicode idents, `${…}` nesting,
  regex-vs-division) are latent — they may never occur in real generated render
  output. The AST would fix them, but that is insurance against a cost not yet
  observed.
- **Worst risk/reward of the phases:** biggest, riskiest, cleanup-only.
- The subsystem is no longer a precedent violation — identifier matching already
  mirrors the main pipeline — so the current state is defensible indefinitely.

## When it becomes worth doing (triggers)

Re-evaluate if any of these fire:
- The string lexer produces a **real** recovery bug on an actual corpus
  (template-literal / regex / unicode mishandling) — then the AST migration pays
  for itself, start with the narrow failing case.
- A **new feature needs the resolved AST** in `VueExpr` (richer template
  transforms, SFC-section source maps, expression-level rewrites) — then carrying
  the AST is a feature enabler, not cleanup, and the ROI flips.
- A deliberate push to **promote `--vue-sfc` out of experimental**, where
  removing the last parallel front-end is part of the hardening bar.

## Related but independent cleanup

`imports.rs` `StrongValueMemberCollector` still hand-rolls its own shadow stack
because imported composable ASTs are parsed without `resolver()`. That cleanup is
**independent of this proposal** (different code path, no ordering constraint) and
is written up separately in
[vue-recovery-resolve-imported-composables.md](vue-recovery-resolve-imported-composables.md).

## References

- Sequencing history and per-phase rationale: issue #196 and the `#196` commit
  history (`git log --grep '#196'` — Phases 0–3, 2a, 2b, with per-commit notes).
- Binding-identity precedent: `crates/core/src/rules/rename_utils.rs`
  (`BindingRenamer`, keyed on `(Atom, SyntaxContext)`).
- Prior art for carrying an AST beside a string: `VueSetupValueBinding`
  (`vue_recovery/locals.rs`).
- Current deviation note: `docs/architecture.md` ("Known deviation: Vue SFC
  recovery").
