# Closure Compiler reproduction matrix

This matrix pins Google Closure Compiler `20260629.0.0` and checks the
reversible, stable syntax shapes Wakaru currently targets:

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

## Scope boundaries

This is intentionally not an “undo ADVANCED optimizations” matrix. ADVANCED
mode can inline functions, fold constants, rename properties, and remove module
boundaries; that information is not generally recoverable.

The matrix also does not claim recovery of Closure's async generator-program
runtime or reconstruction of `goog.module` boundaries after whole-program
flattening. Closure ModuleManager bundle extraction is a separate unpacking
concern and should preserve the shared runtime/namespace model instead of
fabricating ESM imports and exports.

The iterable and optional/nullish snippets invoke their recovered functions and
use the shared execution-equivalence harness. Class inheritance remains a
structural check because executing Closure's full ES5 class runtime also covers
unrelated runtime-helper modernization outside this matrix's scope.

When this matrix finds a new stable compiler shape, first minimize it into a
focused Rust unit test. The matrix is a producer-coverage signal, not a
substitute for rule tests.
