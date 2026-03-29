# Claude Code — Onboarding Guide for wakaru-rs

## What This Is

A Rust rewrite of the wakaru JavaScript unminifier, using the `swc_core` AST ecosystem.
- **Input:** Minified/bundled JavaScript (webpack4 / webpack5 / browserify / plain source)
- **Output:** Readable, modern ESNext code
- Spec: `../RS.md` | Progress: `../TODO.md`

---

## Project Layout

```
src/
  lib.rs            — public API surface (decompile, unpack, unpack_webpack4_raw)
  main.rs           — CLI entry point
  driver.rs         — decompile() and unpack() pipeline orchestration
  rules/
    mod.rs          — apply_default_rules(): the ordered rule pipeline
    *.rs            — one file per transformation rule
  unpacker/
    mod.rs          — re-exports
    webpack4.rs     — webpack4 bundle splitter + per-module decompiler
  utils/
    matcher.rs      — helper predicates

tests/
  common/mod.rs     — shared test helpers: render(), normalize(), assert_eq_normalized()
  *_rule.rs         — per-rule unit tests
  webpack4_unpack.rs          — end-to-end snapshot tests (post-rules output)
  webpack4_unpack_raw.rs      — end-to-end snapshot tests (pre-rules / raw output)
  snapshots/
    webpack4_unpack__*.snap       — decompiled output, pinned per module
    webpack4_unpack_raw__*.snap   — raw extracted output, pinned per module
```

---

## Core Pipeline (`driver.rs` → `rules/mod.rs`)

```
parse_js()
  └─ resolver(unresolved_mark, top_level_mark)   ← gives every ident a SyntaxContext
       └─ apply_default_rules()                   ← ordered VisitMut passes
            └─ fixer()                            ← fixes operator precedence parens
                 └─ print_js()
```

Rule order in `apply_default_rules()` matters — some rules depend on earlier ones.
`UnIife` runs twice: once early, once after `SmartInline` (which can create new IIFEs).

---

## Webpack4 Unpacker Pipeline (`unpacker/webpack4.rs`)

Each factory function `function(e, t, n){...}` in the webpack bundle is extracted and processed:

```
extract body stmts → build synthetic Module
  └─ resolver()                   ← MUST run first; marks free vars with unresolved_mark
       └─ ParamRenamer            ← e→module, t→exports, n→require (scope-aware)
            └─ RequireIdRewriter  ← require(N) → require("./module-N.js")
                 └─ RequireNRewriter        ← require.n(x) → explicit interop getter
                      └─ WebpackRuntimeNormalizer  ← require.r() removed, require.d() → exports.x=
                           └─ apply_default_rules() [optional, skipped for raw mode]
                                └─ fixer()
                                     └─ emit
```

### Critical: Scope-Aware Identifier Matching

All four visitors that match webpack factory params (`ParamRenamer`, `RequireIdRewriter`,
`RequireNRewriter`, `WebpackRuntimeNormalizer`) use this guard:

```rust
if id.ctxt.outer() != self.unresolved_mark {
    return; // skip bound inner-scope identifiers
}
```

**Why:** `resolver()` marks free-variable references with `unresolved_mark`. Inner-scope
bindings (e.g. a `combineReducers(e)` param that happens to be named `e`) get a different
`SyntaxContext`. Without this guard, every `e` / `t` / `n` in the entire module body
gets renamed — including ones in completely unrelated inner functions.

**Pattern to follow when adding new visitors:** always take `unresolved_mark: Mark` and
gate identifier matches on `id.ctxt.outer() == self.unresolved_mark`.

> **Why not use SWC's built-in `rename()`?**
> `swc_ecma_transforms_base::rename::rename(map: &FxHashMap<Id, Atom>)` exists and is
> battle-tested, but requires pre-building a map of `(Atom, SyntaxContext)` keys — which
> is the same information our `unresolved_mark` guard checks. For the narrow
> webpack factory-param use case our approach is simpler and equally correct.
> If a more general rename feature is ever needed, migrate to `rename_with_config()`.

---

## Testing Workflow

```bash
# Run all tests
cargo test

# Run a specific test file
cargo test --test simplify_sequence_rule

# Update snapshots after an intentional rule change
INSTA_UPDATE=always cargo test

# Review snapshot diffs interactively
cargo insta review
```

### Two Snapshot Layers

- `webpack4_unpack__*.snap` — final decompiled output (what users see)
- `webpack4_unpack_raw__*.snap` — raw after webpack normalization, before rules

When debugging a rule issue: diff the raw vs. decompiled snapshot for the affected module
to isolate exactly which rule produced (or failed to produce) the change.

### Writing Unit Tests

Use `render(input)` for full-pipeline output, `assert_eq_normalized()` for comparison
(both sides are parsed+re-emitted to normalize whitespace/parens):

```rust
mod common;
use common::{assert_eq_normalized, render};

#[test]
fn my_rule_test() {
    let input = r#"void 0"#;
    let expected = r#"undefined"#;
    assert_eq_normalized(&render(input), expected);
}
```

Don't use bare literal expression statements as test inputs (e.g. `65536;`) — they are
dead code and `SimplifySequence` drops them. Use variable declarations instead:
`const x = 65536;`

## Definition Of Done

Do not consider a Rust rewrite task "done" just because a local unit test passed.
When you finish a change, verify all of the following that apply:

1. Run the focused rule tests you touched.
2. Run the relevant pipeline tests:
   - `cargo test --test noop_pipeline`
   - `cargo test --test webpack4_unpack`
   - `cargo test --test webpack4_unpack_raw`
   - plus any bundle-specific tests such as `bundle_unpack`
3. If a change affects rename behavior, verify both:
   - rule-level shadowing tests
   - webpack snapshots for real modules, especially cases with reused short names
4. If snapshots change, inspect the diff before accepting it.
   - "tests passed with updated snapshots" is not enough
   - confirm the changed output is semantically better, not just different
5. Compare raw vs final snapshots for the touched module when debugging pipeline behavior.
6. Before committing, check `git status --short` and make sure you are not sweeping in
   unrelated files or stale `.snap.new` artifacts.

For rename-related work specifically, verify:

- identifiers are matched by binding identity (`sym + SyntaxContext`), not symbol text alone
- shadowed locals/params are not renamed by top-level export/import/readability passes
- property keys are not accidentally renamed
- webpack snapshots do not reintroduce leaks like `StrictMode` / `Profiler` / `Fragment`
  replacing unrelated local bindings

For unpack / interop work specifically, verify:

- raw snapshots still show the intended normalization shape
- final snapshots improve or preserve semantics
- webpack5 and browserify coverage still passes if the change touches unpacking logic

---

## Key Rules and Gotchas

### SimplifySequence

Splits `(a(), b(), c())` into separate statements. Also drops pure no-op literal
expression statements (`0;`, `false;`, `null;`) — these are dead code artifacts from
bundlers replacing `if (process.env.NODE_ENV !== 'production') {...}` with `0`.
String literals are intentionally kept (they may be `"use strict"` directives).

### Rule Ordering Matters

Some rules must see the AST in a specific state:
- `UnEsm` depends on `require()` calls being already normalized to string paths
- `SmartInline` can create new IIFEs → second `UnIife` pass cleans them up
- Modernization rules (`VarDeclToLetConst`, `ObjMethodShorthand`, `ArrowFunction`) run
  after structural restoration rules

### Adding a New Rule

1. Create `src/rules/my_rule.rs` implementing `VisitMut` + `Rule` trait
2. Add `mod my_rule;` and `pub use my_rule::MyRule;` to `src/rules/mod.rs`
3. Add `module.visit_mut_with(&mut MyRule);` at the right position in `apply_default_rules()`
4. Create `tests/my_rule_rule.rs` with unit tests
5. Run `INSTA_UPDATE=always cargo test` to regenerate webpack4 snapshots

---

## Debugging Tips

- **Unexpected variable names in output:** Check if a visitor is matching identifiers without
  the `unresolved_mark` guard, or if a rename pass is matching by `sym` instead of
  `(sym, SyntaxContext)`. Compare raw vs. decompiled snapshots.
- **Snapshot diff shows many modules changed:** A rule earlier in the pipeline is probably
  changing something that cascades. Check `SimplifySequence` or `FlipComparisons` first.
- **Rule not firing:** Confirm the AST shape using the raw snapshot — the input to your
  rule may look different than expected after earlier passes.
- **`cargo test` hangs:** Usually a panic in a rule causing infinite recursion. Run with
  `RUST_BACKTRACE=1 cargo test -- --nocapture`.

# Review

OpenAI Codex will review your changes.
