# Contributing

Contributions to Wakaru are welcome: bug reports, small fixes, documentation,
and new recovery patterns. This guide covers getting started and submitting
work. Use [docs/README.md](docs/README.md) to find the detailed guides.

## Choose a focused contribution

Keep each PR focused on one problem. Large PRs take much longer to review and
merge. Split independent changes into separate PRs, or open an issue to discuss
the scope before implementing a broad feature or redesign.

If the bug or problem is already clear, a well-written issue may be the best
contribution. Include a small reproduction, the Wakaru version and command,
and expected versus actual behavior. You do not need to implement a fix to
report a problem. The [bug report form](.github/ISSUE_TEMPLATE/bug_report.yml)
lists the useful details.

## Setup

1. Fork and clone the repository, then create a branch from `main`.
2. For Rust changes, install [rustup](https://rustup.rs/). Commands run inside
   the checkout use the toolchain pinned in
   [rust-toolchain.toml](rust-toolchain.toml), including rustfmt and clippy.
3. Run Cargo commands from the workspace root. Start with a focused test for
   the code you plan to change, then use the checks below before submitting.

For example, this runs one existing rule's tests:

```bash
cargo test -p wakaru-core --test un_double_negation_rule
```

For full suites, install nextest once:

```bash
cargo install cargo-nextest --locked
```

Optionally install `cargo-insta` for interactive snapshot review:

```bash
cargo install cargo-insta --locked
```

Documentation-only contributions do not require a Rust build. The first Rust
build may take time to fetch the formatter's pinned OXC git dependencies; see
[the optional shallow-fetch setup](docs/testing.md#optional-shallow-fetch-git-dependencies).

## Find the right code

Read [docs/architecture.md](docs/architecture.md) for the shared pipeline and
[docs/code-map.md](docs/code-map.md) for source entry points.

| Crate | Path | Purpose |
|---|---|---|
| `wakaru-core` | `crates/core/` | Internal engine: rules, unpackers, and decompile pipeline |
| `wakaru` | `crates/wakaru/` | Published Rust facade; see [public API guidance](docs/public-api.md) |
| `wakaru-cli` | `crates/cli/` | CLI binary and filesystem input/output |
| `wakaru-formatter` | `crates/formatter/` | Optional output formatter using pinned OXC crates |
| `wakaru-wasm` | `crates/wasm/` | WebAssembly bindings |

## Add or fix a rule

Start with the input pattern and intended output. Identify the producer and
version when known, the rewrite level, and any assumptions the change needs.
Read [rewrite assumptions](docs/rewrite-assumptions.md) before choosing the
behavior, and [rule dependencies](docs/rule-dependency-inventory.md) before
choosing the pipeline position.

When the expected input and output are known, write the failing regression
before implementation. Extend the existing rule test file for a fix; create
`crates/core/tests/my_rule_rule.rs` for a new rule.

Here is a complete test example for the existing `UnDoubleNegation` rule:

```rust
mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnDoubleNegation;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnDoubleNegation)
}

#[test]
fn strips_double_bang_in_if() {
    let input = "if (!!x) { a(); }";
    let expected = "if (x) { a(); }";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_strip_in_assignment() {
    let input = "const x = !!flag;";
    assert_eq_normalized(&apply(input), input);
}
```

The first case removes a redundant boolean conversion in a condition. The
second preserves it where the boolean value matters. Both come from
[the existing tests](crates/core/tests/un_double_negation_rule.rs).

For a new rule, implement SWC's `VisitMut` in
`crates/core/src/rules/my_rule.rs`. Export it from `rules/mod.rs`, then add its
runner and descriptor to [the registry](crates/core/src/rules/pipeline.rs).
Follow a neighboring rule with the same context requirements.

Known globals need an `unresolved_mark` check. Local helpers and aliases need
resolved binding identity `(sym, ctxt)`. Use `BindingRenamer` for renaming and
check emitted-name capture separately. The
[architecture guide](docs/architecture.md#key-design-pattern-unresolved_mark)
explains these responsibilities.

To inspect the AST shape reaching a rule, use the trace command:

```bash
cargo run -p wakaru-cli -- debug trace path/to/input.js
```

See [testing](docs/testing.md) for isolated and pipeline tests, and
[debugging](docs/debugging.md) for traces and snapshot investigation.

## Verify your change

Use the [verification checklist](docs/testing.md#required-verification-before-commit)
for the changed surface:

- Core/rule changes: run focused tests, the full core suite, formatting,
  clippy, and applicable reproduction and fixture checks.
- Other crates, release tooling, CI, or web changes: run their relevant tests,
  lint, and build checks. Include core checks if shared behavior is affected.
- Documentation: check links, commands, API references, consistency, and
  `git diff --check`. No Rust test run is required.

Changed snapshots fail tests and produce `.snap.new` files. Review each diff
before accepting it with `cargo insta review` or `cargo insta accept`.
Snapshot updates alone do not replace focused regression coverage.

Skip the private fixture suite if you do not have access to `wakaru-fixtures`.

## Submit a pull request

Describe the problem, the resulting behavior, and why the change addresses it.
Include a before/after example when useful, the checks you ran, and any known
limits or checks you could not run. Link a related issue when one exists.
Use [reviewing.md](docs/reviewing.md) for review scope and verification handoffs.

### Allow maintainer edits

For PRs from a fork, please enable
[Allow edits from maintainers](https://docs.github.com/en/pull-requests/how-tos/work-with-forks/allowing-changes-to-a-pull-request-branch-created-from-a-fork)
when the option is available. GitHub may label it
**Allow edits and access to secrets by maintainers** for forks with workflows.

I often help rebase a PR, fix review nits, or harden edge cases directly on its
branch, then merge it. If you prefer to make all changes yourself or want
another review round before merging, please say so clearly in the PR description.

### Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/). Add a scope
when it helps identify the affected area, for example:

```text
fix(un-esm): preserve mutable require aliases
feat(unpack): support a new bundle format
test(npm): check platform package consistency
docs(contributing): clarify contributor setup
```
