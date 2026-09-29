# tslib Helper Matrix

This matrix tests one cross-cutting dimension: whether helper recovery works
when TypeScript keeps helpers inline, loads a tslib namespace with CommonJS,
or emits named ESM imports. The feature-specific matrices continue to cover
larger syntax/control-flow combinations. This matrix deliberately uses small
module sources so helper delivery differences can be compared in one report.

## Producer profiles

TypeScript is pinned to **5.9.3**. Every profile enables `downlevelIteration`
and `esModuleInterop`, and selects one helper delivery mode:

| Profile | Module output | importHelpers | Helper shape |
|---|---|---|---|
| `commonjs-inline` | CommonJS | false | Inline implementation; control case |
| `commonjs-import-helpers` | CommonJS | true | `tslib_1.__helper(...)` |
| `esm-import-helpers` | ESNext | true | `import { __helper } from "tslib"` |

These are actual `ts.transpileModule` outputs, not rewritten helper spellings.
Each profile runs raw, through Terser compression, and through compression plus
name mangling. Mangled ESM imports also test renamed helper aliases. Compiler
packages use the shared repro tool cache and refresh mechanism.

Every source is a module. Adding `importHelpers: true` to a script-only
profile would silently retain inline helpers and miss the namespace bug.
ES5 covers generator state machines, object/array operations, inheritance,
iteration, interop, and tagged templates. Async functions also target ES2015
to cover awaiters wrapping native generators. Private fields target ES2015,
since TypeScript does not support lowering them to ES5. Interop snippets only
run in CommonJS profiles: an ESM-to-ESM pass would not produce an interop helper.

The async snippet also runs through the shared SWC ES2015 producer with both
inline and external helpers, then TypeScript 5.9.3 ES5 CommonJS with
`importHelpers: true`. This leaves an SWC async wrapper around a tslib namespace
generator and tests both the async helper identity and the shared decoder context.
A standalone SWC ES2015 external-helper profile provides the named ESM-import
control. All three profiles run raw and with both Terser variants. The checked-in
rule fixtures pin SWC 1.16.2 and TypeScript 5.9.3; the matrix uses the shared SWC
tool cache.

## Comparison and known gaps

The matrix compares the **complete module** with the modern source or an
explicit `acceptForms` alternative, using the shared `debug normalize --rename`
comparison. This applies to raw output too: tsc itself renames provider imports.
The local comparison adapter canonicalizes export spelling/placement for
functions, classes, and variable declarations with identifier bindings. For
example, `export const Foo = class ...` and `const Foo = class ...; export { Foo }`
share the same body and public export. Public export
names, other imports, declarations, expressions, and side effects still need
to match. It accepts destructuring moved into a parameter via explicit alternate
forms; it does not accept unrecovered indexing as equivalent destructuring.

Private-field snippets cover named class exports, a default class export, and
an exported class expression. The default-export fixture explicitly accepts a
separate `export default Foo` because `Foo` is never reassigned. The class-expression
fixture requires the direct class initializer. `MergeDeclarationInit` folds an
adjacent bare declaration and inert anonymous class assignment, then the late
export rename removes the immutable alias. Split class assignments and local
temporary/export-snapshot forms no longer count as recovered. Heritage,
computed keys, decorators, static initialization, and references to the outer
binding remain outside this narrow merge. The class-expression fixture has no
alternate initialization forms. The default-export alternative remains a
complete-module alternative, not general declaration/export normalization.
Extra calls, reassignment, and unrecovered helper uses still fail comparison.
Rule tests separately cover direct `export default class`, an intervening default
export, single-declarator class expressions, and their lifetime hazards.

Private names are alpha-normalized by their lexical class binding. The
adapter tracks nested classes, outer private references, and heritage
expressions separately; public method/property names and the choice of private
binding at each access remain significant. This allows mangled backing-map
names to recover as `#r` without requiring the original spelling `#x`.

The compiler-injected tslib import declarations from recovered output are
included in the expected module as well. This allows unused helper imports
that Wakaru preserves after recovery. It does **not** strip helper calls or
helper declarations. Thus `async function` containing a leftover helper call,
`yield* tslib.__values(items)`, a missing export, and an extra side effect still
fail. Tests cover these false-positive boundaries.

This measures structural source recovery, not execution equivalence or tslib
runtime compatibility. The script-only execution harness cannot run these
modules; no `execute` check is claimed. The matrix does not install or execute
tslib itself. Runtime behavior tests remain separate from this recovery score.

The current baseline has **165 yes / 0 no / 0 errors across 165 distinct
shapes**. All nine inheritance shapes now recover the TypeScript default
derived constructor and `super.value()` at `standard`, including module-mode
Terser's lifted inline-helper factory. This uses the documented
[`native_class_inheritance`](../../../docs/rewrite-assumptions.md#native_class_inheritance)
source-recovery policy; `minimal` retains the lowered form. The separate
[fixture runtime oracle](../../../crates/core/tests/fixtures/tslib-inheritance/README.md)
checks ordinary parents and the intentional native-versus-helper differences.
Nine additional inheritance shapes cover static factories whose single
statement returns `new Child(value)`. The rule rebinds the original constructor
reference to the recovered class and rejects name capture, constructor writes,
and deferred factories. Other unconsumed wrapper captures, custom constructors,
dynamic method names and method `.apply` calls remain outside this bounded recovery.

All nine array-read shapes recover complete destructuring, including the six
compressed returns whose element bindings were inlined into indexed expressions.
Eighteen additional shapes cover a literal or identifier suffix after the
complete read sequence (`a + b + 1` and `a + b + other`). Identifier/literal
leaves before or between indexed reads, effectful suffixes, incomplete or
escaped materializations remain covered by negative rule tests.

The original tslib awaiter regression passes all 18 ES5/ES2015 profiles.
The mixed producers add six passing async shapes; the standalone SWC external
profile adds three more (each runs raw and with both Terser variants).
Tagged templates and generator delegation now pass across inline, namespace,
and named-import profiles. Private fields pass all 27 class/export/delivery/minifier
shapes; both interop snippets pass every applicable CommonJS shape.
Fixes should turn their failing shapes into `yes`; regenerate `stats.json`
and its README/website aggregate when measured numbers change. A falling
aggregate after adding challenge rows does not mean existing recovery regressed.

## Boundaries

These cases belong in focused rule tests or separate compiler reproductions:

- Authored `import * as tslib` and direct `require("tslib").helper(...)` calls.
  This TypeScript configuration emits CommonJS namespaces or named ESM imports,
  not those exact spellings; relabeling an edited output as tsc output would hide
  its provenance.
- Mixed Babel wrappers and SWC helper kinds other than the async wrapper. The
  mixed profiles cover SWC inline and external async helpers around tslib generators.
- Shadowed bindings, `with`, wrong helper sources, unsupported state-machine
  shapes, and rollback behavior: these are rule safety boundaries.
- Bundled tslib provider facts and webpack entry extraction: these need the
  unpack pipeline rather than single-file helper recovery.

## Running

From the repository root, with a CLI built from this checkout:

```bash
WAKARU="$PWD/target/debug/wakaru" node scripts/repro/tslib-helpers-matrix/matrix.mjs --details
WAKARU="$PWD/target/debug/wakaru" node scripts/repro/tslib-helpers-matrix/matrix.mjs --snippet tagged-template --json
WAKARU="$PWD/target/debug/wakaru" node scripts/repro/tslib-helpers-matrix/matrix.mjs --dump generator-delegation commonjs-import-helpers
WAKARU="$PWD/target/debug/wakaru" node --test scripts/repro/lib/tsc-batch.test.mjs scripts/repro/tslib-helpers-matrix/compare.test.mjs
WAKARU="$PWD/target/debug/wakaru" node scripts/repro/collect-stats.mjs --check --jobs 2
```

The matrix is included in the aggregate collector and Repro Stats CI.
`--check` compares the recorded success/failure counts with a fresh run; it does
not require every known gap to be fixed. It is a count baseline, not a per-row
lockfile: equal-count swaps can escape that check, so inspect `--json` or
`--details` when changing helper recovery.
