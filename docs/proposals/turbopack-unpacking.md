# Turbopack Production Chunk Unpacking

Status: **IMPLEMENTED for Next.js 15.3 through 16.3** (`unpacker/turbopack.rs`;
current behavior lives in [unpacking.md](../unpacking.md#turbopack)). This
document keeps the format research and the remaining work; the open items
are listed under [Remaining work](#remaining-work). Dev builds are out of scope, matching the
production-build scope in [unpacking.md](../unpacking.md).

Ground rules: follow [AGENTS.md](../../AGENTS.md), including a focused unit
test for every change. Use synthetic module ids, chunk names, and strings in
tests and commits. Update [unpacking.md](../unpacking.md) and the CLI docs in
the same commit that adds behavior.

## Behavior before the detector

Wakaru had no Turbopack detector.

- A single client chunk is not detected as a bundle. It goes through the
  single-file pipeline: JSX is restored, but every module stays inside one
  `push([...])` call and `ctx.i` / `ctx.s` runtime calls remain.
- A directory of chunks keeps only detected files. Turbopack chunks are
  skipped, so the result can contain a few modules split from an unrelated
  file (for example a polyfill chunk going through scope-hoisted splitting).
  The skip shows only in the stderr scan summary, not in `--json`. That is
  more misleading than an explicit "unsupported" result.

## Observed format

Observed on minimal Next.js 15.5 (`next build --turbopack`) and Next.js 16
(`next build`, Turbopack by default) production builds. Ids below are
synthetic.

### Containers

Client chunk:

```js
(globalThis.TURBOPACK || (globalThis.TURBOPACK = [])).push([
  "object" == typeof document ? document.currentScript : void 0,
  101, ctx => { "use strict"; /* ESM module */ },
  202, 203, (ctx, module, exports) => { /* CJS module, two ids */ },
]);
```

Server chunk: the same payload without the script element, as
`module.exports = [101, factory, ...]`.

The global name is configurable. Remote chunk loading uses a computed member
with a project-specific suffix and the same payload:

```js
(globalThis["TURBOPACK_remote_chunk_loading_global_example-app"] ||
  (globalThis["TURBOPACK_remote_chunk_loading_global_example-app"] = []))
  .push([document.currentScript, 101, factory]);
```

Next.js on webpack also contains `TURBOPACK_*` strings (build-manifest and
HMR message constants). A bare `TURBOPACK` substring is not a container
signal.

Payload rules, taken from the runtime's chunk registration:

- Element 0 of a client payload is the script reference (`currentScript`, a
  string path, or absent in a worker).
- After it, one or more ids precede each factory. All of them map to that
  factory (aliases). If any id in a run is already registered, the whole run
  maps to that earlier factory instead.
- A client payload of length 2 is a runtime registration:
  `[script, { otherChunks: [...paths], runtimeModuleIds: [...] }]`. It holds
  no factories.

The runtime lives in its own file: `turbopack-<hash>.js` on the client,
`[turbopack]_runtime.js` on the server. Server entry files load chunks with
`R.c("server/chunks/...")`.

### Factory protocol

The runtime calls `factory(ctx, module, exports)`. `ctx` is an instance whose
prototype holds short method names that the Turbopack runtime defines
itself. Minifiers do not produce them, so they are stable within a version.

| Method | Meaning |
|---|---|
| `ctx.i(id)` | ESM import; returns the namespace, with CJS interop |
| `ctx.r(id)` | CJS require; returns `module.exports` |
| `ctx.s(list, id?)` | define ESM exports on the namespace, then seal it |
| `ctx.v(value, id?)` | `module.exports = value` |
| `ctx.n(ns, id?)` | `module.exports = namespaceObject = ns` |
| `ctx.j(obj, id?)` | dynamic `export *` through a proxy |
| `ctx.A(id)` | async loader: `ctx.r(id)(ctx.i)` |
| `ctx.l(path)` / `ctx.L(path)` | load a chunk |
| `ctx.R(id)` | `require(id).default ?? require(id)` |
| `ctx.t` | host `require` |
| `ctx.f(map)` | `require.context`-style lookup |
| `ctx.g` | `globalThis` |
| `ctx.c`, `ctx.M` | module cache, factory map |
| `ctx.U`, `ctx.P`, `ctx.F`, `ctx.b`, `ctx.h`, `ctx.X` | URL and path helpers |
| `ctx.z` | throws on dynamic `require` |

`ctx.s` list encoding, read in pairs or triples:

- `name, getter` → getter export (Next 15.5 everywhere; Next 16 server).
- `name, getter, setter` → getter/setter export (the setter is the next
  element when it is a function).
- `name, 0, value` → value export (Next 16 client).
- Any other numeric tag throws in the runtime.

The optional second argument of `ctx.s`/`ctx.v`/`ctx.n`/`ctx.j` targets
another module id instead of the current one.

### Lazy imports

`import("./lazy")` compiles to `ctx.A(301)`, where 301 is a loader module:

```js
301, ctx => {
  ctx.v(load => Promise.all(["static/chunks/lazy-beta.js"].map(p => ctx.l(p)))
    .then(() => load(302)));
},
```

302 is the real target module, registered in the listed chunk.

### Version history

Traced in the vercel/next.js source (Turbopack moved into that monorepo
under `turbopack/` in August 2024; earlier code lives in vercel/turbo). The
chunk wrapper is written by `turbopack-browser/src/ecmascript/content.rs` and
`turbopack-nodejs/src/ecmascript/node/content.rs`; the runtime is
`turbopack-ecmascript-runtime/js/src/`; the letter table is `make_shortcut!`
in `runtime_functions.rs`.

| Next.js | `next build` with Turbopack | Container | `ctx` | `ctx.s` encoding | Ids |
|---|---|---|---|---|---|
| 13.x–15.1 | none (hidden flag, env-gated or non-functional) | object keyed by id | destructured long names | `{name: getter}` | strings |
| 15.2 | hidden flag, alpha | object keyed by id, chunk path first | `__turbopack_context__.x` object | `{name: getter}` | numbers |
| 15.3–15.4 | `--turbopack`, experimental | object keyed by id, `currentScript` first | `ctx.x` object | `{name: getter}` (+ target id in 15.4) | numbers |
| 15.5 | `--turbopack` | flat `[script, id, f, ...]` | prototype letters | `name, getter[, setter]` | numbers |
| 16.0–16.1 | default | flat | prototype letters | value `name, 0, value`; accessor `name, getter[, setter]` | numbers |
| 16.2–16.3 | default | flat, configurable global | letters, `w`/`b` reassigned in 16.3 | same as 16.0 | numbers |
| 16.4 canary | default | flat, plus strict-mode factory groups | adds `S` (re-export) | **inverted:** value `name, value`; accessor `name, 0, getter[, setter]` | numbers |

Points that affect a parser:

- Production output exists only from 15.2. Earlier versions produced
  Turbopack output only in `next dev`, so they fall under the dev-build
  exclusion.
- Letter meanings are not stable across versions: 16.3 reuses `w` (was wasm,
  now runtime root) and `b` (was worker blob, then create-worker, now chunk
  base path). The core module protocol letters (`i r s v n j A a t g`) kept
  their meaning from 15.5 to 16.3.
- 16.4 canary inverts the `ctx.s` tag: `[name, 0, x]` is a value in 16.0–16.3
  and an accessor in canary. The two readings differ only when `x` is a
  function, which a value export of a function produces. A parser must not
  guess: use the runtime file when present (its `esm` binding loop shows the
  polarity), or other evidence in the same chunk (an untagged non-function
  value proves the canary protocol; an untagged function proves 16.0–16.3),
  and otherwise reject the ambiguous binding.
- 16.4 canary strict-mode groups put a nested array first
  (`push([script, (() => { "use strict"; return [id, f, ...]; })(), id, f])`)
  or wrap the whole push in a strict IIFE. `ctx.S([...])` encodes re-export
  groups: a module id or namespace value, `exportName, importedName` pairs,
  and a `0` separator; a group holding one string is a comma-joined pair list.
- Extra ids per factory: 15.4 writes `[factory, [id2, ...]]` under the first
  key; 15.5+ writes `id, id2, factory`.
- Async modules: before 15.5 the factory calls a destructured
  `__turbopack_async_module__`; from 15.5 it returns `ctx.a(...)`.
- 16.3 can emit `function () {}` factories instead of arrows.
- Dev builds use path-like string ids such as
  `[project]/app/page.js [app-client] (ecmascript)`.

### Support scope

Support is defined by container shape, not by version number:

1. **First target: the 15.5+ flat container** with the 16.0–16.3 tag
   encoding and the 15.5 getter-pair encoding. This covers Next 15.5 through
   the latest stable 16.x.
2. **Canary shapes** (strict groups, `ctx.S`, inverted tags) follow when they
   reach a stable release. Until then the polarity check above rejects
   ambiguous bindings instead of misreading them.
3. **15.2–15.4 object-keyed containers** were added with the first
   implementation because they needed only a second container parser, the
   object `ctx.s` encoding, and the context preamble. 15.2 was verified only
   through the shared shapes, not against a real build.
4. **Before 15.2:** out of scope. No production output exists.

### Cross-module inlining

Turbopack can inline across module boundaries. In the observed build, a
module that exported only a string constant had no factory left: the literal
was inlined into its importer. Those boundaries cannot be recovered.

## Proposed design

### Phase 1: detection and container extraction

- Detect the client wrapper `(globalThis.TURBOPACK || (globalThis.TURBOPACK
  = [])).push([...])`, its computed `globalThis["TURBOPACK_…"]` variant, and
  the server wrapper `module.exports = [...]` structurally. Both sides of the
  `||` must name the same global. Match `globalThis` with `unresolved_mark`.
- Validate the payload: optional script element, then runs of numeric ids
  each followed by one function. Anything else rejects the container.
- Recognize the runtime registration payload (length 2, object literal) and
  the runtime file, so neither becomes a module.
- Ids are global across chunks, as in webpack multi-chunk input. Multi-input
  runs merge all containers into one id table.

### Phase 2: factory normalization

Reuse the factory-normalization contract in [unpacking.md](../unpacking.md):
canonical parameter names, collision and shadowing checks, hygienic renames,
and a whole-container fallback on failure.

- CJS factories `(ctx, module, exports)`: map `ctx.r(id)` to `require(id)`
  and keep `module` / `exports`. This matches the webpack model.
- ESM factories: map `ctx.i(id)` to a namespace import and `ctx.s([...])` to
  export declarations. This is closer to ESM than webpack's `.d` / `.r`
  getter objects. Getter exports become live bindings; `name, 0, value`
  exports become constant snapshots.
- `ctx.v` / `ctx.n` become `module.exports` assignments.
- `ctx.j` (dynamic `export *` proxy) and the targeted second argument of the
  export methods stay runtime calls in the first version.
- Unknown `ctx` members keep the module as failed/opaque, as webpack does for
  unproven factory parameters. Chunk loading (`ctx.l`, `ctx.L`), path and
  file URL resolution (`ctx.P`, `ctx.F`), the host `require` (`ctx.t`), and
  the require stub (`ctx.z`) were later exempted: they stay residual runtime
  calls with a non-error diagnostic, because they affect no module graph or
  export (see
  [unpacking.md](../unpacking.md#turbopack)). `ctx.f` (`require.context`)
  later became a call to a local copy of the runtime implementation.

### Phase 3: lazy imports and chunk facts

- Recognize the exact loader-module shape and rewrite `ctx.A(loader)` to
  `import()` of the target module when the loader is in the input set.
- Report loader chunk paths through `debug enumerate-chunks`. The runtime
  registration's `otherChunks` gives the startup chunk list. Today a fetcher
  that relies on `enumerate-chunks` reaches no Turbopack lazy chunk, so its
  inputs hold only the chunks the page loaded at startup. Phase 3 is what
  makes multi-input Turbopack runs complete.

### Fail-closed method table

The method table is runtime-defined and changed encoding once already. When
the runtime file is part of the input, read its prototype assignments
(`proto.s = …`, `proto.i = …`) and confirm the expected semantics before
normalizing. Without the runtime, accept only the known shapes above. An
unknown `ctx.s` tag, or a method used with an unexpected arity, rejects the
module rather than guessing.

## Remaining work

Each item keeps the affected factory opaque (or the output less readable)
today; none of them guesses.

- **Free `module`/`exports` in a translated factory.** The webpack
  normalizer renames the translated parameters to those names, so a factory
  that also reads one of them free stays opaque with
  `webpack_factory_recovery_failed`. Turbopack binds these names itself in
  CommonJS modules and folds their `typeof` checks, so a free one comes from
  an ESM module (`module.hot`, `module` as a value) or from Turbopack's own
  lowering of a UMD `define(...)` inside an ESM module, which leaves
  discarded `exports, module` reads. They are never merged: they name
  different objects (an ESM `typeof exports` probe would flip from
  `"undefined"` to `"object"`), and the parameter cannot keep another
  spelling because `UnEsm` matches `require.d(exports, …)` by the unresolved
  `exports` name. Discarded reads could be dropped. A free `require` call
  (from `turbopackIgnore`, including Next's app-page template
  `require("path").join(/* turbopackIgnore: true */ process.cwd(), …)`)
  merges with the require parameter; see
  [unpacking.md](../unpacking.md#factory-normalization-and-failure-boundaries).
- **Top-level `this` in a CommonJS factory.** Turbopack compiles a free
  top-level `this` in a CommonJS module to `ctx.e` (from 15.4.0). Only the
  TypeScript helper guard `ctx.e && ctx.e.__name` becomes `this` again;
  every other read stays `exports`. Translating all of them to `this` would
  restore the source text (UMD roots, `var root = … ? self : this`), but
  `UnEsm` converts a module with a top-level `this` to ESM and keeps `this`,
  which is `undefined` there. Do it once `UnEsm` treats a top-level `this`
  as the exports object.
- **`ctx.b`.** Create-worker in 16.2, chunk base path in 16.3; the same
  letter cannot become a residual without version evidence. Revisit when
  the chunk or the runtime file can prove the version.
- **`ctx.C`, `ctx.U`, `ctx.R`, `ctx.j`, async modules (`ctx.a`).** Not
  translated. `ctx.C` appears in one module of the local Next 16 server
  build; the others were not observed in production chunks.
- **`ctx.f` version evidence.** The local `moduleContext` copy uses the 16.1+
  request parsing unless the chunk is a 15.2–15.4 object container, because
  15.5–16.0 flat chunks look identical to 16.1 ones. When the runtime file
  is in the input set, its `f` implementation shows the behavior directly
  (16.1+ calls a request parser before the lookup); reading it needs
  runtime facts passed across inputs. Do this when a 15.5–16.0 build that
  uses `ctx.f` turns up.
- **Ambiguous ids across inputs.** A merged group can list an id as a
  member in one chunk while another chunk defines the same id as its own
  factory. The multi-input rewrite leaves such ids as numeric `require(N)`
  calls in importers. One option: prefer the definition from the importer's
  own input when it has one (not measured).
- **Cosmetic:** a local exported by both a merged group's primary and one of
  its members is renamed to the member's `name_<id>` alias.
- The 16.4 canary shapes and chunk enumeration (Phase 3).

## Out of scope

- Dev builds and HMR update chunks.
- Recovering boundaries that Turbopack inlined away.
- CSS chunks and the server action / client reference manifests.

## Open questions

- Whether to accept the 16.4 canary shapes before they reach a stable tag.
- Server chunks ship with sibling source maps that embed `sourcesContent` in
  the observed builds. That makes them a good fit for automatic sibling-map
  handling, but server artifacts are rarely what a user has.

## Verification plan

- Unit fixtures: synthetic client, computed-global, and server containers
  covering alias ids,
  each `ctx.s` encoding, `ctx.v`, `ctx.n`, a CJS factory, the loader module,
  and the runtime registration payload. Use synthetic ids and names.
- Rejection tests: unknown `ctx.s` tag, unknown `ctx` member, malformed
  payload, payload with a non-function after an id run.
- Generated builds: add Next 15.5 and Next 16 production builds to the
  reproduction matrix under `scripts/repro/`, with markers for a client
  component, a lazy import, and a CJS dependency.
