# CommonJS Export Storage Recovery

Status: **IMPLEMENTED.** Steps 1 to 5 are implemented: the per-name
analysis (`wakaru debug cjs-exports`), the `commonjs_export_unrecovered`
warning, A (property storage), B (mirror storage), C for getters of a local
binding, and the swc, esbuild, sucrase, and webpack getter forms feeding C.
Step 4 was first narrowed to one bug fix and done later; see
[One getter path](#one-getter-path). Evidence comes from the
[CommonJS export-storage matrix](../../scripts/repro/cjs-export-storage-matrix/README.md);
see [Step 1 results](#step-1-results), [Step 2 results](#step-2-results),
[Step 3 results](#step-3-results),
[Steps 4 and 5 results](#steps-4-and-5-results),
[`export * as ns`](#export--as-ns),
[Single-file import interop](#single-file-import-interop), and
[Lowered `import()`](#lowered-import). The last three are outside the
storage model but blocked matrix rows. The compiler profiles are at
297 / 298. The webpack profiles, added after the implementation, are at
83 / 87; [Remaining gaps](#remaining-gaps) lists what is left for both.
[Statement-path pre-pass audit](#statement-path-pre-pass-audit) records
which older `UnEsm` pre-passes still do work next to the storage rewrite.

Ground rules: follow [AGENTS.md](../../AGENTS.md), including a focused unit
test for every change. Use synthetic names in tests and commits. Record every
new or changed assumption in [rewrite-assumptions.md](../rewrite-assumptions.md),
and update [fact-system.md](../fact-system.md) where it describes same-module
`exports` read recovery.

## Problem

`UnEsm` recovers exports **statement by statement**. Each top-level
`exports.X = value` is classified as an export declaration, and later passes
patch up the reads and writes that the classification left behind. Producers
do not encode exports that way. What a producer decides is **where the
exported value lives**. Its `exports.X = ...` statements are either writes to
that storage, copies that keep the property in sync with a local binding, or
nothing at all when a getter exposes the binding.

When `UnEsm` guesses the wrong storage, the recovered module either keeps an
`exports.X` access (a `ReferenceError` in ESM) or exports a snapshot where the
source had a live binding. Both happen on plain compiler output, not on
unusual hand-written code.

## Evidence

The matrix compiles 28 small ESM modules with 11 producer profiles, decompiles
each CommonJS file back to ESM, and compares runtime behavior of the original
ESM, the CommonJS, and the recovered ESM. Rows where the producer's CommonJS
already behaves differently from the ESM source are excluded.

| Binary | Behavior preserved |
|---|---|
| `main` at the time of writing | 27 / 291 |
| `main` + an A-class prototype (see [Relation to existing paths](#relation-to-existing-paths)) | 108 / 291 |

By producer, with the prototype: TypeScript 19 of 26–28 per profile,
rollup 17 of 23, Babel 6 of 25–27, sucrase 3 of 24, swc and esbuild 0.

The remaining failures, by cause:

| Cause | Rows | Producers |
|---|---:|---|
| Getter helper not recognized (C below) | 77 | swc, esbuild |
| Mirror writes kept or snapshotted (B below) | 50 | Babel, sucrase, TypeScript aliases |
| Property storage only partly recovered (A below) | 33 | TypeScript, rollup, sucrase |
| Single-file import interop (default import of a module without one) | 8 | all |
| String export names (`exports["a-b"]`) | 5 | TypeScript, Babel, rollup |
| TypeScript `__exportStar` | 4 | TypeScript |
| Other esbuild splitting errors | 6 | esbuild |

## How producers store an export

All shapes below are actual output of the pinned producers in the matrix.

### A. The property is the storage

TypeScript (`export let`, `export var`, `export const` with a non-identifier
initializer), rollup, and sucrase after the declaration keep no local binding.
Every read and write goes through the property:

```js
exports.count = 0;                                  // TypeScript, rollup
function bump() { exports.count += 1; return exports.count < exports.limit; }
function step() { exports.n++; exports.n || (exports.n = 9); }
function swap() { [exports.a, exports.b] = [exports.b, exports.a]; }
function each(xs) { for (exports.a of xs); }

exports.y = void 0;                                 // declaration without initializer
exports.y = compute();

let n = 0; exports.n = n;                           // sucrase: local only seeds the property,
function step() { exports.n++; }                    // every later access uses the property
exports.y;                                          // sucrase: `export let y;`
```

A write can be anywhere: top level, nested control flow, a function, a class
member, or a pattern target. The property can be written more than once at the
top level (`exports.x = 1; exports.x = 2; exports.x += 3;`).

### B. A local binding is the storage; writes are mirrored

Babel and sucrase keep the local binding and copy its new value into every
exported name after each write. TypeScript does the same for
`export { local as alias }` and for a reassigned exported function.

```js
let count = exports.count = 0;                      // Babel declaration
exports.count = count = count + 1;                  // Babel assignment, compound
exports.n = ++n;                                    // Babel prefix update
_n = n++, exports.n = n, _n;                        // Babel postfix update
[a, b] = [b, a]; exports.a = a, exports.b = b;      // Babel pattern: mirror in the next statement
for (let _a of xs) { exports.a = a = _a; }          // Babel for-of head
exports.other = exports.value = (internal++, internal); // TypeScript alias
exports.impl = impl = function () { ... };          // TypeScript/Babel reassigned function
impl = exports.impl = function () { ... };          // sucrase, reversed
exports.f = f;                                      // hoisted function export, written once
```

Every write to the property has a value that is the local's current value.
Every write to the local is mirrored in the same statement or the next one.

### C. A local binding is the storage; a getter exposes it

swc and esbuild never write the property. TypeScript and Babel use the same
form for re-exports, and webpack uses it for every export.

```js
_export(exports, { get count() { return count; } }); // swc 1.16
_export(exports, { count: function () { return count; } }); // older swc
__export(mod_exports, { count: () => count });      // esbuild, then
module.exports = __toCommonJS(mod_exports);
Object.defineProperty(exports, "live", { enumerable: true, get: function () { return dep_1.live; } });
__webpack_require__.d(exports, { count: () => count });
```

## Where the current model breaks

`UnEsm` has several mechanisms that each handle one statement shape:

- **Statement classification** (`classify_item`): each top-level
  `exports.X = v` becomes an export. A non-identifier value becomes
  `export const X = v`. An identifier value becomes `export { L as X }`, or a
  snapshot `export const X = L` when `L` has any direct write. That snapshot
  rule is correct for hand-written CommonJS. Under B it is wrong: the mirror
  writes make the property follow `L`, so the source export was live. The
  output keeps the mirror writes (`exports.count = count = count + 1`) next to
  a snapshot export and throws when they run.
- **Stable named read recovery** (`recover_stable_commonjs_reads`): replaces
  later reads of a property that has exactly one write. It skips function
  declaration bodies because hoisting can run them before the export
  statement. Under A that skip is not conservative: ESM has no `exports`, so
  every skipped read throws.
- **Conditional named export recovery** (`recover_conditional_named_exports`):
  activation-time writes inside top-level control flow become `export let X`
  with reads and writes redirected. A compound write or any leftover access
  sends the whole module back to CommonJS.
- **Getter pre-passes**: webpack getters are lowered to assignments marked
  live, and `Object.defineProperty(exports, ...)` getters become live exports
  or re-exports. swc's `_export` and esbuild's `__export`/`__toCommonJS`
  single-file shapes are not recognized at all. esbuild single-file CommonJS
  is even split as a scope-hoisted bundle under `--unpack`.

Three of these mechanisms (classification + snapshot, stable read recovery,
conditional recovery) partly implement A, with different entry conditions.
None implements B. Their gaps are the matrix's A and B failures.

There is also an ordering problem. `UnAssignmentMerging` runs before `UnEsm`
and splits chains with repeatable values:

```js
exports.count = count = 0;   // Babel mirror
count = 0;                   // after UnAssignmentMerging
exports.count = 0;
```

After the split, the mirror is only "the same literal written twice". The
fact that the property copies `count` is no longer visible in the syntax.
`SimplifySequence` similarly turns `_n = n++, exports.n = n, _n` into three
statements, which keeps the mirror recognizable (next statement).

## Proposed design

Replace statement classification with one decision **per export name**,
based on a whole-module inventory of how that name is accessed.

### 1. Inventory

Walk the resolved module once and record, for each static export name
(identifier or string key) on the unique unresolved `exports` binding:

- every access, with its position (top-level statement, nested top-level,
  deferred inside a function or class member) and kind (read, plain write,
  compound or logical write, update, pattern target, call target, `typeof`);
- for each plain write, the value shape: `L`, `L = e`, `++L`, a sequence ending
  in `L`, another export write whose value has one of these shapes, or other;
- getter definitions for the name (`Object.defineProperty`, the swc, esbuild,
  and webpack helpers) and the binding or member each getter returns.

The module-level gates are the ones the current proofs already use: one
`exports` binding, only static member access (no escape, computed key,
`delete`, or prototype-mutating member), no `module.exports` replacement (or
`module` absent), no direct `eval` or `with`. If a gate fails, keep the
current fallback for the whole module and report it (see
[Unrecovered names](#4-unrecovered-names)).

### 2. Classify each name

Check in this order. The first match wins.

**C (getter).** The name has a getter definition and no other write. Export
the returned binding live: `export { L as X }`, or `export { m as X } from`
for a getter returning a member of a `require` binding. Replace reads of
`exports.X` with `L`.

**B (mirror).** There is one module-level binding `L` such that:

1. every write to the property has a mirror value shape for `L`; and
2. every write to `L`, including an initializing declarator, is in a
   statement that also writes the property with a mirror value, or is
   followed by such a statement with only other mirror statements in
   between. A mirror statement only copies local identifiers into `exports`
   properties (`exports.a = a, exports.b = b;`).

Alternatively, condition 2 holds as a **final copy**: the property is never
read or called in the module, its only write is one top-level statement, and
every write to `L` is an earlier top-level statement outside any function.
Only an importer can then observe the property, and it sees the final value
either way. Rollup places every `exports.x = x` copy at the end of the module
in this shape.

A copied parameter or function-local binding is not a candidate for `L`: a
local copied into the property is a value, not the export's storage.

Export `L` live: `export { L as X }`. Drop the mirror writes, keeping `L`'s
own write (`exports.X = L = e` becomes `L = e`). Replace reads of
`exports.X` with `L`. Condition 2 is what separates compiler mirrors from
hand-written CommonJS. If a write of `L` is not mirrored, the property lags
behind `L`, and a live export would change behavior.

"Same statement" covers chains once `UnAssignmentMerging` leaves them whole
(see [Relation to existing paths](#relation-to-existing-paths)). "Next
statement" is still needed for two shapes that do not come from that split:
Babel emits a pattern write and its mirror as two statements
(`[a, b] = [b, a]; exports.a = a, exports.b = b;`), and `SimplifySequence`
splits Babel's postfix form `_n = n++, exports.n = n, _n` into three
statements, so the mirror follows the write. Babel and TypeScript also copy
one binding into several names, or several bindings after one declaration,
as consecutive statements (`let [first, second] = pair; exports.second =
second; exports.first = first;`), so a mirror can sit behind other mirror
statements. Those statements write properties of other names from
identifiers; they read no property and run no code. A mirror behind any
other statement does not count, and hand-written shapes are added case by
case when data shows them.

**A (property storage).** Otherwise, the property is the storage. Introduce
one binding for the name and rewrite every access, in any position, to it.
Declare it `var`:

- at the first top-level plain write as `export var X = value`, when such a
  write exists;
- otherwise as `export var X;` at the top of the module.

A hoisted `var` is `undefined` until a write runs, which is exactly an
unassigned property. Therefore reads before the declaration, writes inside
hoisted functions, and repeated top-level writes keep their behavior
regardless of position. `VarDeclToLetConst` then narrows the kind with its
full write and use-before-declaration analysis. If the name is reserved,
invalid as an identifier, already used by an unrelated binding, or shadowed
at an access site, use a fresh local and `export { local as X }`, including
`export { local as "a-b" }` for string names.

A direct call through the binding (`exports.f()`) passes `exports` as the
receiver. Rewrite a call target only when the binding is never written after
its declaration and its value cannot observe the receiver (the current
`is_receiver_insensitive_function_value`, since removed with stable read
recovery). Otherwise leave the name
unrecovered (see [Unrecovered names](#4-unrecovered-names)). The
implementation uses a looser condition; see [Step 2 results](#step-2-results).

### 3. Readability passes

These do not change semantics and run after the classification:

- **Seed alias** (sucrase): `let L = v; exports.X = L;` where `L` has no
  other reference becomes `export var X = v`.
- **Sentinels**: `exports.X = void 0` writes in the leading prefix are
  dropped for A and B names. Under A they are writes of the same value the
  hoisted `var` already holds.
- **Hoisted function exports**: `exports.f = f` for a function declaration
  never reassigned is B with no other writes. It becomes `export { f }` as
  today.

### 4. Unrecovered names

A name that matches no class keeps its accesses unchanged. Today the only
per-name failure is a direct call through a value that may be replaced or
may observe its receiver. A module-level gate failure leaves every name
unchanged. Either way, the recovered ESM can still contain `exports.X`, which
throws when it runs, so the gap must be visible:

- Add a warning kind (for example `commonjs_export_unrecovered`) that lists
  the export names whose accesses remain in an ESM output. Single-file and
  unpack runs both return driver warnings. Like `cross_module_class_call`, it
  is computed by the driver from the emitted AST and is not error-class, so
  it does not change the exit status. Rules have no warning channel today. A
  driver check on the output needs none, and it also catches residuals that
  other rules leave. A new warning kind is CLI-visible output, so the commit
  updates `docs/cli.md`, `skills/wakaru/SKILL.md`, and the docs-site CLI page
  together.
- The step-1 debug report gives the reason per name (which class failed and
  which condition rejected it), so a warning can be explained without a
  rebuild.
- The output validator keeps counting the same accesses as
  `esm_commonjs_residual` for corpus runs.

### Relation to existing paths

| Existing path | After this design |
|---|---|
| Snapshot rule for `exports.X = L` with written `L` | Becomes A with value `L`: `export var X = L` is the same snapshot. The decision stands; it is no longer the default for every identifier value. Still applies to the names the statement path keeps (see [Statement-path pre-pass audit](#statement-path-pre-pass-audit)). |
| Stable named read recovery | Replaced by A and B, which rewrite every access. |
| Conditional named export recovery | Replaced by A. Nested and compound writes are ordinary A writes. |
| A-class prototype that rewrites leftover accesses after the stable pass | Superseded by step 2. It measured that A alone moves the matrix from 27 to 108, and was never merged. |
| Webpack and `defineProperty` getter pre-passes | C inputs. Top-level webpack `require.d` calls are lowered to getter definitions; the storage rewrite owns getters of a local binding (see [One getter path](#one-getter-path)). The getter-loop IIFE is lowered the same way (see [Getter-loop IIFE](#getter-loop-iife)). |
| `UnAssignmentMerging` repeatable-value chain split | Stops splitting a chain that writes both an `exports` property and a local identifier. The pipeline order stays: the `UnAssignmentMerging` → `UnEsm` edge is confirmed in [rule-dependency-inventory.md](../rule-dependency-inventory.md), and moving `UnEsm` first would also hand it every chain that `UnAssignmentMerging` already splits safely. A chain left whole is handled by the class of its export name: B drops the mirror target (`L = v`), A rewrites the target (`X = L = v`, still one valid chain), C does not occur because getter names have no writes. |
| `has_unhandled_named_export_chain` rollback | Accepts a chain whose export targets are all names the storage rewrite owns (step 2). Still keeps the boundary for chains whose names no model owns, such as after a module gate failure. |

Default exports follow the same model with the name `default`. A becomes
`var _default; export { _default as default }`. B becomes
`export { L as default }`. `export default <expression>` remains for a single
top-level write with no other access, matching today.

## Out of scope

Each of these needs separate work. The matrix tracks them.

- **swc and esbuild helper recognition.** Done in step 5 for single-file
  decompilation. esbuild single-file output is still split as a
  scope-hoisted bundle under `--unpack`.
- **`__exportStar` and other `export *` helpers.** Separate work on CommonJS
  `export *` recovery (`un_esm/export_star.rs`); its matrix row is covered
  in [`export * as ns`](#export--as-ns).
- **sucrase `_createNamedExportFrom`.** Done in step 5.
- **Single-file import interop.** Without facts about the provider,
  `require("./dep")` became a default import even when the provider has no
  default export. See [Single-file import interop](#single-file-import-interop).

## Assumptions to record

- **Mirror coverage (B).** If every write of `L` is mirrored in the same or
  next statement, the property equals `L` everywhere it can be observed.
  Between a write of `L` and a next-statement mirror, nothing else can
  observe the property, unless the write expression itself calls code that
  reads `exports`. Compiler output does not do that, but it is not a
  guarantee.
- **Hoisted `var` equivalence (A).** This holds under the existing gates. A
  remaining difference is `"X" in exports` or `Object.keys(exports)` before the
  first write, which the static-access gate already excludes.
- `commonjs_exports_data_properties` continues to apply.

## Implementation steps

Each step is a separate commit. Each commit includes unit tests in
`crates/core/tests/un_esm_rule.rs` and a matrix run compared with the
previous step.

1. **Done.** Add the per-name inventory and classification behind the
   existing paths, with a debug report of the A/B/C decision and the
   rejecting condition per name (`wakaru debug cjs-exports`, and `--explain`
   in the matrix). Compare its decisions with the matrix cases before
   changing output. Add the `commonjs_export_unrecovered` warning in the same
   step, so the baseline gap is visible before any output changes.
2. **Done.** Implement A and remove the conditional recovery it replaces.
   Stable read recovery stays until step 3; see
   [Step 2 results](#step-2-results).
3. **Done.** Stop `UnAssignmentMerging` from splitting chains that write
   both an `exports` property and a local, then implement B; see
   [Step 3 results](#step-3-results).
4. **Done.** Route the existing getter pre-passes through C. First narrowed
   to one statement-path bug (see
   [Steps 4 and 5 results](#steps-4-and-5-results)); done once the webpack
   profiles showed what the separate paths missed (see
   [One getter path](#one-getter-path)).
5. **Done.** swc, esbuild, and sucrase recognizers. They feed C.

Run the private fixture suite and the full core suite at every step. Steps 2
and 3 change snapshots by design. Each changed snapshot needs a reason in the
commit.

## Step 1 results

Measured on the matrix with the report-only analysis; output is unchanged
(27 / 291 behavior preserved).

**Warning.** `commonjs_export_unrecovered` fires on 186 of 264 wrong rows and
on none of the 27 ok rows. The other 78 wrong rows have no leftover access to
report: swc and the two Babel `alias-export-mutated` rows stay whole-module
CommonJS (valid CommonJS that the ESM driver cannot load), and esbuild rows
are ESM with a wrong export surface but no `exports` access.

**Decisions per name**, summed over all cases:

| Producer | property | mirror | getter | module gate failed |
|---|---:|---:|---:|---:|
| TypeScript 5.9 (es2020 / es5) | 35 | 50 | 6 | 1 |
| TypeScript 4.3 | 35 | 48 | 6 | 1 |
| TypeScript 3.9 | 36 | 43 | 6 | 1 |
| Babel | 11 | 72 | 6 | 0 |
| Babel loose | 11 | 66 | 0 | 0 |
| rollup | 21 | 45 | 2 | 1 |
| sucrase | 24 | 48 | 0 | 2 |
| swc (es2020 / es5) | 0 | 0 | 0 | 31 / 32 |

esbuild single-file output has no `exports` access (`module.exports =
__toCommonJS(...)`) and reports no names.

Reviewed against the producer shapes above:

- TypeScript: `export let/var` and non-identifier `export const` are A,
  functions, classes, and aliases are B, re-exports are C. Matches.
- Babel: 9 of its 11 A names are wrong for the reason this proposal
  predicts. `UnAssignmentMerging` split a mirror chain with a repeatable
  value (`exports.mode = mode = "on"` becomes `mode = "on"; exports.mode =
  "on";`, including after `UnCurlyBraces` turns an `if` branch into a block),
  so the property write no longer copies the binding. Step 3 fixes the
  input. The other two are a snapshot `export default state` (A is correct)
  and a name that is only declared (`maybe`).
- sucrase: names written through the property after the seeding copy are A,
  as described above.
- rollup: end-of-module copies are B through the final-copy condition.
- Module gate failures are all out of scope here: swc `_export`, sucrase
  `_createNamedExportFrom`, and `export *` (TypeScript `__exportStar`,
  rollup's `Object.keys(dep).forEach` loop, sucrase `_createStarExport`).
- Babel loose has no getter names because its re-export rows are excluded:
  its CommonJS already diverges from the ESM source.

Three conditions were refined while comparing, all recorded in the B
section: mirrors may sit behind other mirror statements, the final-copy
alternative, and the module-level requirement for `L`.

## Step 2 results

Matrix: 150 / 291 behavior preserved (from 27), with no row that was correct
on `main` now wrong. By producer: TypeScript 22 per profile, rollup 21,
sucrase 15, Babel 13 per profile, swc and esbuild 0. The warning fires on 57
of 141 wrong rows and on no ok row. The remaining TypeScript, rollup, and
sucrase failures are B names (aliases, reassigned functions, sucrase copies
read inside functions, a string-named alias), `export *`, and single-file
import interop. Babel also gains rows: names whose mirror chain
`UnAssignmentMerging` split are A now, which is correct but less readable
than the B result step 3 will give.

How A was placed and where it differs from the design above:

- **Position.** The rewrite runs after the CommonJS pre-passes (webpack
  getters, export-star loops, require hoisting) and before statement
  classification, because those pre-passes match `exports.x` shapes that the
  rewrite would remove. The module-gate decision (keep CommonJS when the gate
  fails and a property is written inside top-level control flow, or when
  `exports` is reassigned or aliased; see
  [Reassigned or aliased `exports`](#reassigned-or-aliased-exports)) is still
  taken first, before anything changes. `has_unhandled_named_export_chain`
  accepts a chain whose every export target is an A name the rewrite owns.
- **Statement-path names.** A name whose only access is one whole top-level
  write after leading sentinels stays on the statement path, which exports
  the value directly (`export default value`, `export const f = function f()
  {}`); A would need a fresh local whenever the name is taken. Names written
  more than once, in a chain or in control flow, read, or written in
  functions go through A, and so does a name with only a sentinel
  (`exports.x = void 0` becomes `export let x;`). An earlier version also
  left names with several whole top-level writes on the statement path,
  which exported only the last value.
- **Calls.** A direct call through an A name is rewritten under
  `call_receiver_independence`, the assumption conditional recovery already
  used, instead of the stricter "never reassigned and receiver-insensitive"
  condition above. Only a written value that is a function reading `this`
  keeps the name unrecovered.
- **Stable read recovery stays.** It still replaces reads of a stable copy
  (`exports.f = f; ... exports.f()`), which are B names until step 3
  implements B.
- **TypeScript enum and namespace initializers.**
  `L = exports.x || (exports.x = {})` counts as chain evidence for B, so A
  leaves it to `UnEnum`, which folds the exported enum later. Step 3 must
  keep that shape intact when it rewrites B reads.
- **Seed alias.** The sucrase seed (`let n = 0; exports.n = n;`, the local
  never used again) becomes `export var n = 0`.
- **Excluded names.** `exports.exports` stays on the statement path, which
  keeps its boundary for that key. A name that is only ever read becomes a
  local `var` without an export: CommonJS never created the property.
- **Getters inside functions** fail the module gate. Such a getter can be
  installed any number of times, and webpack factory IIFEs that contain one
  are unwrapped by a later recovery that needs their `exports` accesses
  intact.
- **Sentinels.** Leading `exports.x = void 0` statements are removed by the
  existing prefix scan, which now also looks past `var` declarations whose
  initializers run no code (TypeScript's `this && this.__awaiter ||
  function` helpers). A sentinel after a call stays: the call may write the
  property first.

## Step 3 results

Matrix: 193 / 291 behavior preserved (from 150), with no row that was
correct after step 2 now wrong. By producer: TypeScript 24 to 26 per
profile, Babel 25 per profile, rollup 21, sucrase 21, swc and esbuild 0. The
warning fires on 15 of 98 wrong rows and on no ok row. Outside swc and
esbuild, the remaining failures are all out of scope: `export *`
(TypeScript, rollup, sucrase), single-file import interop, and re-exports
through getters (Babel, sucrase), which step 4 covers. No pipeline snapshot
changed.

Babel output for a mirrored counter is now:

```js
export let count = 0;
function inc() {
  count = count + 1;
  return count;
}
```

How B was placed and where it differs from the design above:

- **Position.** B runs right after A, in the same place. A rewrites first
  because its declaration placement uses module-body indices that B's
  statement removal would shift.
- **Statement-path names.** A mirror name whose only access is one whole
  top-level `exports.x = local;` statement stays on the statement path, which
  already exports the local. This keeps hoisted function exports
  (`exports.f = f`) unchanged.
- **Fallback to A.** B replaces reads with the local, so it needs the local
  to be visible and initialized at every read. If any other binding in the
  module has the local's name (a parameter, an inner declaration), or a read
  outside functions comes before a lexical local's declaration, the name
  goes through A instead. A is valid for every name that passes the module
  gate; only readability differs.
- **Removed statements.** A copy statement that the rewrite reduces to
  something with no effect (`count;`, `void 0;`, `a, b;`) is removed. Only
  statements the rewrite changed are pruned.
- **Export placement.** The `export { local as x }` specifier follows the
  local's declaration, so a later pass can merge it into `export let`.
- **TypeScript enums.** Both enum and namespace argument shapes,
  `L = exports.x || (exports.x = {})` and `L || (exports.x = L = {})`, stay
  for `UnEnum`, which folds them into `const L = {...}` with an export. Only
  while they are still the argument of the enum IIFE: Terser can inline the
  IIFE (`(L = exports.x || (exports.x = {})).A = "A"`), `UnEnum` does not
  fold that, and the name is an ordinary mirror instead.
- **`UnAssignmentMerging`.** A chain with an `exports` property target and a
  resolved local target stays whole, including inside functions. A chain
  that also writes another export name, whose names no model owns (for
  example after a gate failure), now keeps the module CommonJS through
  `has_unhandled_named_export_chain`; before, the split let the statement
  path convert it partially.
- **Named stable read recovery is removed.** Mirror and property names no
  longer reached it; only enum names and names rejected for a
  receiver-sensitive call still could, and the core suite and the fixtures
  pass without it. The `module.exports` default-read part stays.
- **Assumption.** The mirror condition is recorded as
  `commonjs_export_mirror_coverage` in
  [rewrite-assumptions.md](../rewrite-assumptions.md).

**`export *` with mirror names.** Babel emits that chain for every write of
an aliased export (`export { x as y }` makes `x = 3` into
`exports.y = exports.x = x = 3`), so it is producer output, not hand-written
code. When the same module has `export * from`, Babel's copy loop indexes
`exports[key]`, which failed the module gate, and the whole module stayed
CommonJS. The analysis now skips every statement the export-star recovery
(`un_esm/export_star.rs`) replaces with `export * from`; that rewrite runs
before the storage rewrite, so the loop is gone by then. A top-level
declarator chain (`var local = exports.local = 1`) also no longer counts as
a write nested in control flow, which had kept such modules CommonJS when
the gate failed.

The matrix `reexport-star` rows still fail, on `export * as ns`:
TypeScript's `exports.ns = __importStar(require("./dep.js"))` leaves its
`require` in the ESM output. That is namespace re-export recovery, outside
this proposal.

## Steps 4 and 5 results

Matrix: 270 / 291 behavior preserved (from 193), with no row that was correct
after step 3 now wrong. By producer: TypeScript 24 to 26 per profile, Babel 26
and 25 (loose), swc 25 and 26, esbuild 24, sucrase 22, rollup 21. The
warning fires on 6 of 21 wrong rows and on no ok row. Every remaining failure
is out of scope: `export * as ns` (all `reexport-star` rows), single-file
import interop (all `import-then-export` rows, sucrase
`imported-used-in-function`), and esbuild's `__toESM(require(...))`, which
single-file mode does not recognize (esbuild `imported-used-in-function` and
`reexport-named`). No pipeline snapshot or fixture output changed.

**Step 4 was narrowed.** Webpack `require.d` getters keep their own pre-pass,
and `Object.defineProperty(exports, ...)` getters keep the statement-path
classifier. After step 5 every getter input reaches one of the two, and on
every matrix row their output matches the C decision except one: Babel's
`reexport-named`. Babel defines the getters before
`var _dep = _interopRequireWildcard(require("./dep.js"))`, which
`UnInteropRequireWildcard` has turned into `import * as _dep` by the time
`UnEsm` runs. The re-export classifier only accepted `require()` bindings, so
the getters stayed and threw at load. It now also accepts namespace import
bindings, which are immutable, and drops the import when only the re-exports
read it. Merging the two paths into C would change no matrix row, so it was
not done then. The webpack profiles later showed rows it does change; see
[One getter path](#one-getter-path).

**Step 5: getter helpers are lowered, not classified.** `UnEsm` first
rewrites each helper call into the per-name getter definitions it performs,
`Object.defineProperty(exports, "x", { enumerable: true, get })`
(`un_esm/export_getters.rs`). Both the analysis (C) and the statement path
already handle that shape, and the result is the same CommonJS module, so a
module that later keeps its CommonJS boundary loses nothing. `debug
cjs-exports` runs the same lowering before its analysis. Helper bodies are
proven by shape:

- swc `_export(target, all)`: one `for (name in all)` defining
  `{ enumerable: true, get }` on `target`, where `get` is
  `Object.getOwnPropertyDescriptor(all, name).get` (swc 1.16, entries are
  getters) or `all[name]` (older swc and esbuild, entries are functions). An
  entry that does not match the helper's read keeps the call.
- esbuild `module.exports = __toCommonJS(ns)` after `var ns = {};
  __export(ns, {...})`: `__toCommonJS` and its `__copyProps` are proven too.
  The getters move from the fresh `module.exports` object to `exports` only
  when the module refers to neither `module` nor `exports` anywhere else, so
  nothing can tell the two objects apart. No code runs between the
  replacement and the getter definitions.
- sucrase `_createNamedExportFrom(obj, "x", "y")`: lowered when `obj` is a
  binding that is never written, because the helper captured its value.
- A repeated or `__proto__` key keeps the call: defining the same
  non-configurable property twice throws, and the object literal would set
  the prototype instead.

A getter definition for a name that is not an identifier
(`export { v as "a-b" }`) now becomes a quoted export specifier.

## `export * as ns`

After steps 4 and 5, every `reexport-star` row still failed. The case puts
`export * from "./dep.js"` and `export * as ns from "./dep.js"` in one
module, and each producer broke on the second re-export or on how the two
share a source:

| Producer | Shape | Fix |
|---|---|---|
| TypeScript | `exports.ns = __importStar(require("./dep.js"))` | `UnInteropRequireWildcard` unwrapped the call to the raw `require`, which `UnEsm` left as `export const ns = require(...)`. A top-level property write of a wildcard call now gets its own `import * as ns` binding. |
| swc | `var _dep = _interop_require_wildcard(_export_star(require("./dep.js"), exports))` | The export-star recovery only knew the call as a statement. The nested form becomes `export * from` plus `import * as _dep`, when the star helper is proven to return its source. |
| esbuild | `__reExport(ns, require("./dep.js"), module.exports)` and `var ns = __toESM(require("./dep.js"))` | Neither helper was recognized in single-file output. The getter lowering rewrites the first to `__reExport(exports, require(...))` for the export-star recovery, and the second to `import * as ns`. |
| sucrase | `var _depjs = require(...); var _depjs2 = _interopRequireWildcard(_depjs); _createStarExport(_depjs);` | The wildcard of a separate `require` binding becomes a namespace import when the binding is declared once and never written, and `_createStarExport` is a new star helper shape. |

Matrix: 280 / 291 (from 270), with no row that was correct before now wrong.
Besides 8 `reexport-star` rows, `__toESM` fixed esbuild's
`imported-used-in-function` and `reexport-named`. The remaining 11 wrong
rows: `import-then-export` on every producer (single-file import interop),
sucrase `imported-used-in-function` (the same interop: a default import of a
module with no default), and rollup `reexport-star`.

**rollup** came last because its namespace helper always sets `default` to
the whole module:

```js
function _interopNamespaceDefault(e) { var n = Object.create(null); /* getters for e's keys except default */ n.default = e; return Object.freeze(n); }
var dep_js__namespace = _interopNamespaceDefault(dep_js);
```

Babel, TypeScript, swc, sucrase, and esbuild set `default` to `e.default`
when the module is marked `__esModule`, which is what an ESM namespace of a
recovered dependency gives. Run against rollup 4.63 samples, rollup's own
CommonJS differs from its ESM source in `ns.default` whether or not the
provider has a default export. Keeping the helper would reproduce that
difference; reading it as a wildcard interop reproduces the source. The
second was chosen, as the named assumption
[`namespace_interop_source_semantics`](../rewrite-assumptions.md#namespace_interop_source_semantics).
The helper is proven by body shape, including the `constBindings`,
`freeze: false`, `symbols`, non-live, and terser forms. Once the namespace
import no longer reads `dep_js`, the `export *` loop is its only use and is
recovered too. The `interop: "compat"` helper, which first returns a provider
with a `default` key unchanged, is read the same way.

Matrix: 281 / 291, with rollup `reexport-star` the only changed row.

## Single-file import interop

`import { live } from "./dep.js"` compiles to a plain `require` and
`_dep.live` reads, which `UnEsm` turned into `import _dep from "./dep.js"`.
The recovered `dep.js` has no default export, so the module failed to link.
A default import is the only form that links against every CommonJS
provider, but it fails against an ESM one; unpack mode settles this from
provider facts, single-file mode had nothing.

The importing module carries the evidence: compilers read a default import
through `.default` or an interop-default helper and a named import through
neither. `RelativeNamespaceImport` emits `import * as _dep` for a relative
`require` binding with no default-import sign in a module marked
`__esModule` (or lowered from esbuild's `__toCommonJS`), as the named
assumption
[`relative_require_esm_provider`](../rewrite-assumptions.md#relative_require_esm_provider).
The signs are collected at the start of the helper stage, before the
interop unwrapping rewrites a default import's `.default` reads into the
named import's shape.

Matrix: 290 / 291 (from 281), with no row that was correct before now
wrong: `import-then-export` for TypeScript, Babel, swc, and esbuild, and
sucrase `imported-used-in-function`. rollup `import-then-export` still
fails: rollup marks `__esModule` only when the module has a default export,
so this module shows no evidence.

## Lowered `import()`

Babel, TypeScript, swc, sucrase, and rollup (`dynamicImportInCjs: false`)
compile `import("./dep.js")` to
`Promise.resolve().then(() => WILDCARD(require("./dep.js")))`, or pass a
non-literal specifier through the promise or a wrapper function. In a
converted module, `UnEsm` unwrapped the interop and left
`Promise.resolve().then(() => require("./dep.js"))`, which throws
`ReferenceError` in ESM. The wildcard unwrap that `UnEsm` runs now turns the
whole shape back into `import(...)`, under the named assumption
[`lowered_dynamic_import_source_semantics`](../rewrite-assumptions.md#lowered_dynamic_import_source_semantics).

The `dynamic-import` case covers a namespace, a `.then` read, a default
export, and a non-literal specifier. Matrix: 297 / 298 (from 290 / 291): the
case's 7 scored rows are correct, and nothing else moved. Babel, esbuild, and
rollup keep a native `import()` with the matrix options, so their rows are
`p≠`; the Babel 7.29 and rollup lowered forms have unit tests instead.

In unpack mode, swc output built with `externalHelpers` and bundled with
`@swc/helpers` calls a helper from another module:
`import { _ } from "./helper.js"` and
`Promise.resolve().then(() => _(require(x)))`. `UnEsm` runs in Phase 1,
before that helper is known, so a Phase 2 pass restores the shape from the
provider's helper export fact. A webpack 5 bundle of that output has an
end-to-end unit test.

## One getter path

The webpack profiles showed what the separate getter paths missed. Webpack
`require.d` calls went through their own pre-pass, which turned each getter
into `exports.x = value` marked live:

- the array form of webpack 5.108 and later,
  `require.d(exports, ["c", 0, c, "x", () => x])`, and rspack's
  `require.d(exports, getters, values)` were not recognized, and the call
  stayed in the ESM output;
- a getter returning a member of an imported module (`x: () => dep.y`)
  became a snapshot `export const x = dep.y` instead of
  `export { y as x } from`.

Every top-level `require.d` call is now lowered to per-name definitions
before the analysis, in `un_esm/export_getters.rs`, like the swc, esbuild,
and sucrase helpers. A getter becomes `Object.defineProperty(exports, "x",
{ enumerable: true, get })`. A value slot becomes `exports.c = c` at the
call's position: the runtime reads it when the call runs. A key that two
calls define is not lowered, because the runtime skips a key `exports`
already owns while a second definition would throw. When the module stays
CommonJS, `UnEsm` puts the calls back as written. `require.d` calls inside an
unused wrapper IIFE are lowered once the IIFE body is exposed. The
getter-loop IIFE form was lowered later (see [Getter-loop IIFE](#getter-loop-iife)).

The storage rewrite now also owns C names whose getter returns a binding the
module computes itself (a function, a class, or a variable whose initializer
calls no `require`). It drops the definition, rewrites reads to the binding,
and places `export { local as x }` after the binding's declaration, as for B.
A later pass then merges it into `export let`/`export const` where the names
match. A getter of an import, a `require` result, or a member of one stays
on the statement path, which emits `export { y as x } from`. A module that
calls bundler runtime helpers still keeps its converted form under decision
8; that check now reads the module before the lowering.

Two neighboring fixes came with it. Webpack re-exports now reach
`export { d as x } from "./m"`, and in unpack mode a later rename of an
unrelated local `d` also renamed that specifier, because `BindingRenamer`
treated the source name of an `export … from` as a local reference; it now
skips those specifiers. And a specifier placed after `var o = {}` split the
`require.d(o, ...)` run that `UnWebpackDefineGetters` folds into one object;
that rule now looks past local export specifiers, which run no code.

Matrix: 351 / 385 (from 331), with no row that was correct before now wrong.
webpack 5.111 rises from 17 to 27 rows in both profiles. The remaining
webpack failures are `reexport-star` and `dynamic-import` (see below) and
every webpack 5.107 row (the whole-namespace default import). Pipeline
snapshots changed only in placement: the specifier of a getter export now
follows its declaration instead of the getter map, which merges `export
const version` in webpack 5 output and spreads minified webpack 4 aliases
(`export { i as x }`) next to their declarations.

## Escaped namespaces and unresolved ids

Step 2 left two kinds of webpack getter in the ESM output, where the first
`exports` access throws. Both were snapshots before it.

- **A getter of a namespace the module also passes on whole**
  (`x: () => o.y` next to `use(o)`). The statement path kept the getter: an
  escaped namespace might be written through. webpack and rspack output
  shows that write cannot happen through the escape. A CommonJS provider's
  namespace escapes as `require.t(o, 2)`, a new object with getters only,
  and an ESM provider's exports have getters only too. Only a CommonJS
  `module.exports` passed on through a default import can be written, and
  a snapshot misses that write as well. The getter is the compiled form of
  a live `export { y as x } from`, so a binding that the module never
  rebinds, writes a property of, or deletes from now gets that re-export
  whatever else reads it.
- **A getter of a member of `require(<id>)` whose id the unpacker could not
  resolve** has no source to re-export from. It becomes `exports.x = r.y`
  right after the `require` declaration, a snapshot export of the value once
  the module has loaded. webpack defines its getters before the
  declarations, so the write moves down past other export definitions and
  `require` statements, which a getter definition cannot observe. A getter
  with any other statement in between stays as written.

The escaped namespace itself was the other half. `UnEsm` imports
`var o = require("./m")` as a default import, and with provider facts
`provider_namespace_repair` turned it into `import * as o` only for member
reads and a few `Object` helpers. A namespace passed on as a value
(`use(o)`, `require.t(o, 2)`, `export { o as ns }`) kept a default import
that cannot link when the provider has no default export. The repair now
accepts those uses in unpack mode, and `UnWebpackInterop` replaces
`require.t(ns, 2)` of a namespace import, with its cache, by the namespace.
A self-requiring module keeps the old check: its fallback is CommonJS, not
an import that cannot link.

Matrix: 377 / 385 (from 351), with no row that was correct before now wrong.
webpack 5.107 rises from 0 to 24 rows and webpack 5.111 from 27 to 28 in
both profiles.

## Getter-loop IIFE

webpack's getter loop can also appear inlined as an IIFE:

```js
((target, getters) => {
  for (const key in getters)
    Object.defineProperty(target, key, { enumerable: true, get: getters[key] });
})(exports, { x: () => L });
```

`rewrite_webpack_export_getters` used to lower each getter to
`exports.x = L` and mark it live for the statement path. The storage
rewrite runs before that path and did not see the mark, so a name the module
also read (`use(exports.x)`) became A: `export var x = L` at the IIFE's
position, a snapshot, and a TDZ error when `L` is a `let` or `const`
declared later. A getter of a member of a `require` binding became a
snapshot as well, instead of `export { y as x } from`.

The IIFE is now lowered with the top-level `require.d` calls, to the same
getter definitions, so its names take the same paths: C for a local binding,
a re-export for an imported member, and the unresolved-id snapshot. The live
mark is gone. The default-object compatibility block after the IIFE is
removed with it, as before; a module that stays CommonJS gets both back.
The one shape that loses recovery is a getter of a member of a local object
(see [Remaining gaps](#remaining-gaps)).

## Providers with a default export

A whole-namespace use (`var ns = require(id); use(ns)`) of a provider with a
default export still became `import ns from`, which silently binds the
default value instead of the namespace. `UnEsm` turns both `exports.default =
v` and `module.exports = v` into a default export, and only the second makes
`v` the whole required value. A Phase 1 fact now records, before `UnEsm`,
that a module has no `module` reference, so requiring it returns its
`exports` object; `provider_namespace_repair` treats such a provider's
default as one namespace property. The fact does not need the `__esModule`
marker: a hand-written CommonJS module that sets `exports.default` returns
the same object.

The consumer needs a fact too. webpack's `require.n` getter (`() => x &&
x.__esModule ? x.default : x`) and the Babel and TypeScript interop helpers
read the default export, and the helper stage and `UnWebpackInterop` replace
them with the plain binding before the repair runs. A second fact records,
before those rules, which sources the consumer requires only as plain
`var x = require(src)` bindings that no interop wrapper touches. The repair
treats a default as a namespace property only for those sources.

Matrix: 380 / 385 (from 377), with no row that was correct before now wrong.
webpack 5.107 rises from 24 to 27 rows.

## Statement-path pre-pass audit

The storage rewrite took over most names, so the older pre-passes that
`UnEsm::convert` runs before it were checked for dead code, the same way
named stable read recovery was removed in step 3. Each pass was disabled in
turn, and the core suite, the matrix, and the private fixtures were compared
with the unchanged build. None is dead, so none was removed. No matrix row
moved with any pass disabled: compiler and webpack output reaches these
passes only with shapes the storage rewrite already owns or that the matrix
does not contain.

| Pass | What it still handles | Without it |
|---|---|---|
| `split_compound_exports`, `module.exports` form | `var v = module.exports = e` | `module.exports` stays in the ESM output |
| `split_compound_exports`, named form | `var s = exports.x = e` when `s` or the property is written again, or when the module gate fails | valid but less readable A output (`export var x; var s = x = e;`); under a direct `eval` the module stays CommonJS |
| `split_chained_local_module_exports_assignments` | `local = module.exports = e` | the module stays CommonJS |
| `split_called_module_exports_assignments` | `(module.exports = f)(args)` | the module stays CommonJS |
| `normalize_named_export_chains` | chains with a function, `require`, or provider-call value, and `module.exports = exports.default = v` | named chains fall to A (`export var a; export var b; a = b = function () {};`); the default chain keeps the module CommonJS |
| `has_unhandled_named_export_chain` | chains whose names no model owns, for example under a direct `eval`; its local-tail exemption lets TypeScript enum initializers (`L \|\| (exports.x = L = {})`) through to `UnEnum` | `export const a = exports.b = v`, which leaves `exports.b` in ESM; a module with a TypeScript enum export stays CommonJS |
| Snapshot rule in the statement classifier | a one-write name copying a written local, such as `var v = 1; exports.v = v; function f() { v = 2; }` | a live `export { v }` that follows the later write |
| `rewrite_webpack_export_getters` | `require.d` calls inside an unused wrapper IIFE | the getters stay |

The `module.exports` forms are default exports, which the storage rewrite
does not model. Three of these were live only on shapes no test covered: the
named form of `split_compound_exports`, the live mark, and the postamble
removal after an IIFE with a `default` getter. Each now has a unit test that
fails without it. The live mark and the postamble removal have since moved
out of this pass (see [Getter-loop IIFE](#getter-loop-iife)).

## Remaining gaps

- **The statement path is still an entry point for named exports.** The
  storage rewrite owns A and B names, but `UnEsm`'s statement classification
  (`classify_item`) still converts:
  - a property name whose only access is one whole top-level write, and a
    mirror name whose only access is one `exports.x = local;` copy (see
    [Step 2 results](#step-2-results) and [Step 3 results](#step-3-results));
  - a getter that returns an import, a `require` result, or a member of one,
    which becomes a re-export (see [One getter path](#one-getter-path));
  - TypeScript enum initializers that `UnEnum` folds, `exports.exports`, and
    names the module already exports as ESM;
  - every name in a module with a self-`require` or a module gate failure
    that does not force CommonJS (for example a direct `eval`), and every
    `module.exports` assignment.

  The pre-passes that prepare those statements are all still needed; see
  [Statement-path pre-pass audit](#statement-path-pre-pass-audit).
- **A getter of a member of a local object** (`x: () => cfg.main`) has no
  live ES module form. The name is unrecovered, so a module without bundler
  runtime helpers stays CommonJS under decision 8, and one with them keeps
  the getter definition in its ESM output. The getter-loop IIFE used to turn
  it into a snapshot (see [Getter-loop IIFE](#getter-loop-iife)).
- **rollup `import-then-export`, single-file only.** Without a marker the
  module has no module-level evidence. The two signals left are weak: a
  re-export getter from the same source (present only when the module also
  re-exports from it), and rollup naming the binding after the source path
  (`dep_js` for `./dep.js`), which hand-written code can match. Neither is
  used. Decompiling both files together (`wakaru --unpack=auto mod.js
  dep.js`) already recovers `import { live } from "./dep.js"` from provider
  facts, and its behavior matches the ESM source; rollup output with many
  CommonJS files (`preserveModules`) is meant to be decompiled that way.
- **esbuild single-file CommonJS under `--unpack`.** Still split as a
  scope-hoisted bundle (see [Out of scope](#out-of-scope)).
- **webpack profiles.** The matrix bundles each case with webpack 5.107
  (object-form `require.d`) and 5.111 and unpacks the bundle. The array form
  and re-export getters are fixed (see [One getter path](#one-getter-path)).
  The failing rows are `dynamic-import` in every profile, because webpack's
  own lowered `import()` (`Promise.resolve().then(require.bind(require,
  id))`, and `require.e` for a context module) is not restored, and
  `string-export-name` in webpack 5.107, whose `require.d` stays in the
  output for a reason not yet investigated.
- **Getter inputs C does not take yet.** swc's `_export` helper inlined by a
  minifier into `for (k in all) Object.defineProperty(exports, k, { get:
  all[k] })` is the largest real-world residual: the module stays CommonJS
  with its exports unrecovered. A named getter function (`get: function
  get() {}`) and a value descriptor (`{ value: v }`, which may be a mangled
  `__esModule`) are not taken either.
- **Re-exports from a provider that stays CommonJS.** `export { y as x }
  from "./m"` links in Node only when cjs-module-lexer finds `y` in `./m`;
  it does not for `module.exports = value`. Getter re-exports emit it
  whatever the provider recovers to, escaped or not. Node binds that name
  once, after `./m` has run, which is what a snapshot gives too, and a
  bundler keeps it live.
- **Unresolved-id snapshots** (see
  [Escaped namespaces and unresolved ids](#escaped-namespaces-and-unresolved-ids))
  lose later writes to the property, and a module whose getter has other
  code before the `require` declaration keeps the getter. The `require(<id>)`
  itself stays in the ESM output, with no warning.
- **Interop unwrapping before the module boundary is decided.**
  Unwrapping `_interopRequireDefault(require(x)).default` to a plain
  `require(x)` binding read whole is right only once `UnEsm` turns that
  `require` into a default import. `UnInteropRequireDefault` now unwraps
  only helper runtime requires, and `UnEsm` unwraps the rest, including the
  wildcard interop's `import * as`, when it commits to converting the
  module; a module it keeps CommonJS keeps the calls, and so does a
  `require` inside a function or class body, which stays a `require`.
  The exception is a lowered `import()` (see [Lowered `import()`](#lowered-import)).

## Reassigned or aliased `exports`

A failed module gate used to keep the module CommonJS only when an `exports`
property was written inside top-level control flow. Otherwise the statement
path still converted every top-level `exports.X = v`. That is wrong once the
`exports` binding stops naming `module.exports`:

```js
exports.a = 1; exports = { b: 2 }; exports.c = 3;  // `c` lands on the local object
var t = exports; t.a = 1; exports.b = 2;           // export `a` is lost
exports = module.exports = Parser; exports.Parser = Parser; // Node idiom
```

The last line is a hand-written Node idiom that libraries keep in their
published source. The matrix compiles ESM, so it has no row for it.

The whole module now stays CommonJS when the `exports` binding is reassigned
anywhere (assignment, pattern target, in a function), or used as a value that
can alias it: a declarator or assignment value, `return`, an object or array
element, and so on. Two uses fail the gate without forcing that boundary
up front, because the statement path recovers known helpers that make them:

- a direct call argument (`__exportStar(require("./dep"), exports)`,
  `register(exports)`);
- the right operand of `in`, which CommonJS export-star loops test before
  each copy.

If such a use is still there after conversion, no helper took it (an
unknown call, or `__exportStar(require(12345), exports)` with a module id the
unpacker could not resolve), and the ES module would throw on it. `UnEsm`
then restores the CommonJS module, unless that module calls bundler runtime
helpers such as `require.d`: no CommonJS loader provides them, so restoring
gains nothing, and the converted form keeps the recovered exports and the
warning. Before checking, `UnEsm` removes a default-object compatibility
block that the converted getters left dead, which unpack used to do only on
its second `UnEsm` run. A module with a default export keeps that block,
which reads the whole `exports` object
(`Object.assign(exports.default, exports)`); without bundler helpers it now
stays CommonJS instead of converting into ESM that threw at load.

A top-level `this` is the same object: CommonJS runs the module body with
`this` set to `module.exports`, and an ES module body has `this` undefined.
`this.value = 1` would throw after conversion, and UMD global detection
(`typeof self == "object" ? self : this`) would silently pick `undefined`.
So a top-level `this` (outside function and class bodies; arrows, a class
heritage, and computed class keys see it) keeps the module CommonJS too,
even when the module has no `exports` access. TypeScript's helper guard
`(this && this.__name) || impl` is exempt: it picks `impl` in both module
systems unless the module accesses that property through `exports`.

## Decisions

Recorded 2026-10-03.

1. **Fallback granularity:** an unrecovered name keeps its accesses and is
   reported (see [Unrecovered names](#4-unrecovered-names)); the rest of the
   module is still recovered. The warning is what lets corpus runs and users
   see the remaining gap.
2. **B strictness:** same statement or next statement, nothing further.
   Hand-written CommonJS shapes are judged case by case on data. Refined by
   decision 5 for producer shapes.
3. **`var` in output:** acceptable. A emits `var` and leaves narrowing to
   `VarDeclToLetConst`; a residual `var` where that analysis cannot prove
   safety is fine.

Recorded 2026-10-04, after step 1.

4. **Warning visibility:** `commonjs_export_unrecovered` is reported by
   default. `--diagnostics` means "re-parse the output to find more
   problems", not "show warnings", and this check needs no re-parse.
5. **B conditions follow real artifacts:** the mirror run (copies behind
   other mirror statements) and the final-copy condition are accepted,
   because Babel, TypeScript, and rollup emit those shapes.
6. **Reassigned or aliased `exports` keeps the module CommonJS**, with the
   call-argument and `in` exceptions above. Taken before step 3 because the
   webpack factories that keep this idiom cannot be unpacked faithfully until
   `UnEsm` stops converting such modules in part.
7. **A top-level `this` counts as an aliased `exports`** and keeps the
   module CommonJS, with the TypeScript helper guard exempt. Taken after an
   outside review of this line found modules converted with a `this` that
   then throws or reads `undefined`.
8. **A whole-`exports` use that survives conversion restores the CommonJS
   module.** Decision 6 lets call arguments and `in` operands through for
   the helpers the statement path recognizes; when none recognized them,
   the ESM output would throw on the leftover use. Not for modules that
   call bundler runtime helpers, which cannot run as CommonJS either.
   Per-name leftovers (`exports.x`) still keep decision 1.
