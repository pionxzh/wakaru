# Closure Compiler reproduction matrix

This matrix pins three non-nightly Google Closure Compiler releases spanning
more than two years—`20240317.0.0`, `20250226.0.0`, and `20260629.0.0`—and
checks the reversible, stable syntax shapes Wakaru currently targets:

- ES5 class/prototype lowering, including `$jscomp.inherits`;
- ES5 iterable loops using `$jscomp.makeIterator`;
- optional chaining and nullish coalescing;
- the corresponding ES2020 output, to guard native-syntax pass-through.

```bash
node scripts/repro/closure-compiler-matrix/matrix.mjs --level standard
node scripts/repro/closure-compiler-matrix/matrix.mjs --level standard --details
```

The script installs its pinned compiler under `target/repro-tools/`. `target/`
is ignored by git.

The versions represent the first release with portable native packages across
the matrix's development platforms, a middle release, and the release used to
develop the current recovery. This is deliberately a compatibility sample
rather than every monthly compiler build, and it avoids requiring Java solely
for older producer coverage.

## Scope boundaries

This is intentionally not an “undo ADVANCED optimizations” matrix. ADVANCED
mode can inline functions, fold constants, rename properties, and remove module
boundaries; that information is not generally recoverable.

The matrix also does not claim recovery of Closure's async generator-program
runtime or reconstruction of `goog.module` boundaries after whole-program
flattening. Closure ModuleManager bundle extraction is a separate unpacking
concern and should preserve the shared runtime/namespace model instead of
fabricating ESM imports and exports.

Every snippet invokes the recovered behavior and uses the shared
execution-equivalence harness. The class-inheritance case constructs the
recovered subclass and compares its observable method result in addition to
checking the recovered class syntax.

When this matrix finds a new stable compiler shape, first minimize it into a
focused Rust unit test. The matrix is a producer-coverage signal, not a
substitute for rule tests.
