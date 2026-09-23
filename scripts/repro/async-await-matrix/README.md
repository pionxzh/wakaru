# Async/Await Matrix

This matrix generates async function, async arrow, async IIFE, double-await,
try/catch/finally, loop control flow (with and without internal continue),
`for await` (plain, early `return`, `break` inside `try/finally`, destructured
element, nested sync `for…of`, inside `try/catch`, in an async arrow),
destructuring/default, object-rest, nested async callback, and generator
delegation snippets through Babel, TypeScript, SWC, and esbuild, then runs
wakaru over each generated shape.

## Comparison model

Non-mangled shapes are checked with substring needles (`expected` /
`expectedAny`). Mangled shapes can't be — every local binding is renamed — so
they are compared **structurally**: `wakaru debug normalize --rename` alpha-
renames all local bindings to canonical names and reprints, so an alpha-
equivalent recovery normalizes to byte-identical source. A mangled shape passes
when its normalized output equals the normalized `source` or any entry in the
snippet's `acceptForms` (genuinely distinct idiomatic recoveries, e.g. a C-style
loop recovered as `for…of`). See `../lib/compare.mjs`.

This replaced an earlier regex name-stripping normalizer that was lossy and
**false-passed** unrecovered output: Terser-compressed regenerator state
machines and Babel lazy-init helper artifacts were reported as recovered. Those
now correctly show as `no`. Remaining `no` rows fall into four honest buckets:

- **state-machine** — wakaru leaves a Terser-compressed regenerator runtime intact.
- **degraded** — a helper artifact leaks (`__rest` inlined, `const x = undefined`,
  `push.apply(...)` not recovered).
- **control-flow** — every `for await` row through a regenerator machine
  (Babel `regenerator` mode and preset-env) stays a machine. The decoder
  rebuilds the loop and both try regions. The protocol's close guard
  (`if (abrupt && it.return != null)`) sits in the try block nested inside
  the `finally`, and it jumps forward to that block's `finally` label. The
  regenerator decoder joins forward jumps only at the end of the machine,
  so this jump stays an opcode and the decode fails closed. A `break` in the
  loop adds a second block: it exits through an abrupt-completion opcode
  inside the try region (`_context.a(3, N)`), which the decoder does not
  structure either. One Terser shape of the swc `_ts_generator` machine
  nests a yield opcode two ternaries deep in a single `return`, which also
  stays a machine. The `__generator` machines (tsc-es5, swc-es5) join
  forward jumps mid-machine and rebuild loops inside try regions, so their
  `for await` rows recover, including the Terser variants.
- **nested loop** — a sync `for…of` inside the recovered `for await` body
  keeps its indexed form: the machine hoists the index and array temporaries
  to the function scope and assigns them in place, a shape the for-of
  recovery does not match.
- **hoisted destructuring** — after regenerator recovery of the Babel 7.8 and
  7.13 Terser rows, the destructuring stays in assignment form with
  `_slicedToArray(temp = defaulted, n)` on an inline-assigned temp, and the
  `input == null ? await load() : input` pick stays an `if`/`else` temp
  assignment instead of folding to `??`. A destructured `for await` element
  through a `__generator` machine (tsc-es5, swc-es5) binds the value
  temporary and keeps the hoisted member reads (`id = _d.id`) in the body.

The matrix's `error` count is separate from those Wakaru recovery failures.
It counts producer or harness transform failures, for which Wakaru is not
invoked; one failed producer transform also marks its two downstream Terser
variants as `source not in batch`, so the count grows by three per failed
source transform. The Babel profiles carry the plugins their snippets need
(see below), so a non-zero error count means a producer regression, not a
known limitation.

Some hoisted `let x; … x = await …` splits are folded back to `let x = await …`
by the `MergeDeclarationInit` rule, while others intentionally remain split
when merging across awaits would mask unrecovered state-machine artifacts. The
`acceptForms` above capture both clean forms.

Each Babel/TypeScript/SWC/esbuild output is checked in three Terser variants:

- raw compiler output
- Terser compression without name mangling
- Terser compression with name mangling

The matrix also includes standalone source-through-Terser rows for both Terser
variants, because some recoverable shapes only appear after compiler or source
output is minified.

Each `for await` row also rejects `asyncIterator` in the recovered output, so
a recovered loop that leaves the adapter helper behind (`_asyncIterator`,
`__forAwait`, `__asyncValues` all mention it) is reported as `no`. Where Terser
inlines the single-use element (`size += _step.value.size`) the recovered loop
binds the protocol's step name; the `async-for-await-arrow` row accepts that
form through `expectedAny`.

The `class-async-method` snippet also includes a dedicated Babel preset-env IE11
profile. Its Terser compression+mangle variant reproduces Babel's lazy async
class-method trampoline after minification, where the method descriptor value
becomes a comma-sequence assignment plus `.apply(this, arguments)` wrapper.

Babel is run in two modes:

- `async-generator`: `@babel/plugin-transform-async-to-generator`, leaving
  native generator syntax inside `_asyncToGenerator(...)`.
- `regenerator`: async-to-generator plus `@babel/plugin-transform-regenerator`,
  producing `regeneratorRuntime.wrap(...)` state-machine output, with
  `@babel/plugin-transform-destructuring` ahead of them because regenerator's
  declaration hoisting has no case for patterns with defaults or rest.

Both modes also run the async-generator-functions plugin
(`@babel/plugin-proposal-async-generator-functions` for the 7.8 and 7.13
profiles, `@babel/plugin-transform-async-generator-functions` from 7.28) ahead
of async-to-generator, in preset-env order: it lowers `for await` to the
`_asyncIterator` protocol while the enclosing function is still `async`.
Without it, async-to-generator leaves `for await` inside a plain generator,
which is not valid JavaScript and which Terser rejects.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

Run from the repo root:

```bash
# Full matrix
node scripts/repro/async-await-matrix/matrix.mjs --level standard

# Full matrix with failure details (lowered + recovered code blocks)
node scripts/repro/async-await-matrix/matrix.mjs --level standard --details

# Single snippet (exact or substring match)
node scripts/repro/async-await-matrix/matrix.mjs --level standard --snippet async-iife

# Dump full lowered input + wakaru output for one snippet × tool
node scripts/repro/async-await-matrix/matrix.mjs --level standard --dump async-simple-loop tsc-es5-terser-compress

# Structured output for triage: every row carries full lowered + recovered code
# and (for failures) the missing/leaked needles. Pipe to jq.
node scripts/repro/async-await-matrix/matrix.mjs --level standard --json | jq '.rows[] | select(.status=="no")'

# Cluster failing shapes by the structure of their recovered output (alpha-
# renamed), so repeats (e.g. identical unrecovered state machines) collapse and
# genuinely distinct failures stand out.
node scripts/repro/async-await-matrix/matrix.mjs --level standard --cluster
```

These flags are shared by every matrix (they live in `../lib/runner.mjs`). The
structural comparison and `--cluster` keys are produced by `wakaru debug
normalize --rename`; see `../lib/compare.mjs`.

The script installs transformer and minifier packages under
`target/repro-tools/`, so those downloads are cached outside the source tree.
