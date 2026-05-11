# Contributing

Thank you for your interest in contributing to wakaru! This guide covers the practical steps for getting started, adding features, and submitting changes. For deeper dives, see the docs linked throughout.

## Setup

1. Fork the repo and create your branch from `main`.
2. Install a stable Rust toolchain (via [rustup](https://rustup.rs/)).
3. Run `cargo test` from the workspace root to verify everything builds and passes.
4. Make your changes.

You will also want [cargo-insta](https://insta.rs/) for snapshot management:

```bash
cargo install cargo-insta
```

## Checks

Before submitting a PR, make sure all of the following pass:

```bash
cargo fmt --check       # formatting
cargo clippy -- -D warnings  # lints
cargo test              # all tests
```

If you changed rule behavior and snapshots need updating:

```bash
INSTA_UPDATE=always cargo test   # accept all snapshot changes
cargo insta review               # or review interactively
```

## Project Structure

wakaru is a Cargo workspace with three crates:

| Crate | Path | Purpose |
|---|---|---|
| `wakaru-core` | `crates/core/` | Decompile pipeline, transformation rules, unpackers, and public API |
| `wakaru-cli` | `crates/cli/` | CLI binary (`wakaru`) built on `clap` |
| `wakaru-wasm` | `crates/wasm/` | WASM bindings for browser-based decompilation |

Almost all development happens in `wakaru-core`. Key directories within it:

- `src/rules/` -- one file per transformation rule, plus `mod.rs` which defines pipeline ordering
- `src/unpacker/` -- bundle format detection and module extraction (webpack4, webpack5, browserify, esbuild)
- `src/driver.rs` -- orchestrates the full decompile and unpack pipelines
- `tests/` -- per-rule test files, pipeline integration tests, and snapshot fixtures

For the full architecture overview, see [docs/architecture.md](docs/architecture.md).

## Adding a New Rule

This is the most common type of contribution. Here is a step-by-step walkthrough.

### 1. Create the rule file

Add a new file at `crates/core/src/rules/my_rule.rs`. A minimal rule looks like this:

```rust
use swc_core::ecma::ast::Expr;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct MyRule;

impl VisitMut for MyRule {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        // your transformation logic here
    }
}
```

If your rule needs to distinguish free variables (globals) from locally-bound identifiers, take `unresolved_mark`:

```rust
use swc_core::common::Mark;

pub struct MyRule {
    unresolved_mark: Mark,
}

impl MyRule {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}
```

Then guard identifier matches with `id.ctxt.outer() == self.unresolved_mark`. See [docs/architecture.md](docs/architecture.md) for details on why this is necessary.

### 2. Register the rule in the pipeline

In `crates/core/src/rules/mod.rs`:

1. Add `mod my_rule;` to the module declarations at the top.
2. Add `pub use my_rule::MyRule;` to the re-exports.
3. Add a `run!(MyRule, "MyRule");` line (or `run!(MyRule::new(unresolved_mark), "MyRule");`) at the appropriate position in `apply_rules_range_impl()`.
4. Add `"MyRule"` to the `rule_names()` list at the matching position.

Pipeline placement matters -- see the "Pipeline Ordering" section below.

### 3. Write tests

Create `crates/core/tests/my_rule_rule.rs`:

```rust
mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::MyRule;

fn apply(input: &str) -> String {
    render_rule(input, |_| MyRule)
}

#[test]
fn transforms_target_pattern() {
    let input = r#"/* minified input */"#;
    let expected = r#"/* readable output */"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn leaves_unrelated_code_alone() {
    let input = r#"/* code that should not change */"#;
    assert_eq_normalized(&apply(input), input);
}
```

Cover both positive cases (pattern is transformed) and negative cases (unrelated code is untouched).

### 4. Run full pipeline tests

After adding the rule, run the full test suite to check for snapshot changes in other tests:

```bash
cargo test
```

If other snapshots changed, review them carefully. If your rule is placed early in the pipeline, it can cascade through later rules.

## Testing

Tests live in `crates/core/tests/`. The key test helpers are:

| Helper | Use when... |
|---|---|
| `render(source)` | You want to test the full decompile pipeline |
| `render_rule(source, builder)` | You want to test a single rule in isolation |
| `render_pipeline_until(source, stop_after)` | You want to test up to a specific pipeline stage |
| `render_pipeline_between(source, start, stop)` | You want to test a range of rules |
| `assert_eq_normalized(actual, expected)` | Comparing output (normalizes whitespace) |

Common pitfalls:

- Do not use bare expression statements as test inputs (e.g., `65536;`) -- `SimplifySequence` drops them as dead code. Use `const x = 65536;` instead.
- When `render()` gives unexpected results, switch to `render_rule()` to isolate your rule, or use `render_pipeline_until()` to stop at a specific point.

For the full testing guide (snapshot workflows, test organization, all available helpers), see [docs/testing.md](docs/testing.md).

## Debugging

When a rule is not working as expected, start with the rule trace CLI:

```bash
cargo run -- debug trace path/to/input.js
```

This prints a git-style diff for each rule that changes the output, making it easy to see where transformations happen (or fail to happen).

Useful options:

```bash
# Show all rules, including ones that did not change output
cargo run -- debug trace path/to/input.js --all

# Trace only a range of rules
cargo run -- debug trace path/to/input.js --from RemoveVoid --until UnEsm
```

Common symptoms and what to check:

- **Rule not firing** -- An earlier rule may have changed the AST shape. Use `debug trace` to see what the input looks like by the time your rule runs.
- **Unexpected variable names** -- Check for a missing `unresolved_mark` guard.
- **Too many snapshots changed** -- An early pipeline rule is cascading. Check early rules like `SimplifySequence` and `FlipComparisons`.
- **`cargo test` hangs** -- Likely infinite recursion. Run with `RUST_BACKTRACE=1 cargo test -- --nocapture`.

For the full debugging guide (snapshot layers, fixture repo workflow), see [docs/debugging.md](docs/debugging.md).

## Pipeline Ordering

Rules run in a fixed order defined in `crates/core/src/rules/mod.rs`. The pipeline has six stages:

1. **Stage 1: Syntax normalization** -- simplify sequences, flip comparisons, remove void, etc.
2. **Stage 2: Transpiler helper unwrapping** -- remove Babel/TypeScript helpers, reconstruct module systems
3. **Stage 3: Structural restoration** -- template literals, while loops, nullish coalescing, optional chaining
4. **Stage 4: Complex pattern restoration** -- IIFEs, conditionals, parameters, enums, JSX, classes
5. **Stage 5: Modernization** -- arrow functions, let/const, object shorthand, exponentiation
6. **Stage 6: Cleanup and renaming** -- import/export rename, smart inline, smart rename, dead code elimination

Order matters because rules depend on earlier ones having run. For example:
- `UnEsm` (Stage 2) depends on `UnCurlyBraces`, `UnEsmoduleFlag`, and `UnAssignmentMerging` having normalized the AST first.
- `ArrowFunction` (Stage 5) should run after `UnEs6Class` (Stage 4) so that class methods are not incorrectly converted to arrows.

When placing a new rule, consider:
- What AST shape does your rule expect? Place it after the rule that produces that shape.
- Will your rule's output be consumed by a later rule? Make sure it runs first.
- Use `cargo run -- debug trace` on real-world samples to verify your rule fires at the right point.
- Run the full test suite and review any snapshot changes to catch ordering issues.

For the full list of rule dependencies, see [docs/rule-dependency-inventory.md](docs/rule-dependency-inventory.md).

## Commit Message Format

This project follows the [Conventional Commits](https://www.conventionalcommits.org/) specification. Please make sure your commit messages are formatted correctly.

Examples:

```
feat: add UnNullishCoalescing rule
fix: handle nested ternary in UnConditionals
test: add edge case for arrow function with rest params
refactor: extract shared helper into babel_helper_utils
docs: update architecture diagram for two-phase pipeline
```

**Please mention the issue number in the commit message or the PR description.**
