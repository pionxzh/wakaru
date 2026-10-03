# Turbopack Production Chunk Unpacking

Status: **PROPOSED.** The format is understood well enough to design
against. Next.js 16 builds production apps with Turbopack by default, so new
Next.js deployments ship this format unless they opt out. Dev builds are out of
scope, matching the production-build scope in
[unpacking.md](../unpacking.md).

Ground rules: follow [AGENTS.md](../../AGENTS.md), including a focused unit
test for every change. Use synthetic module ids, chunk names, and strings in
tests and commits. Update [unpacking.md](../unpacking.md) and the CLI docs in
the same commit that adds behavior.

## Current behavior

Wakaru has no Turbopack detector.

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

### Version differences seen so far

Between 15.5 and 16 only the `ctx.s` encoding changed (getter pairs became
`name, 0, value` triples in client chunks). Older 15.x releases and dev
builds were not examined. Dev builds use path-like string ids such as
`[project]/app/page.js [app-client] (ecmascript)`.

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
  unproven factory parameters.

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

## Out of scope

- Dev builds and HMR update chunks.
- Recovering boundaries that Turbopack inlined away.
- CSS chunks and the server action / client reference manifests.

## Open questions

- Older Turbopack production output (Next 15.0–15.4, `--turbo` era) may use
  an object-keyed payload. Check before claiming version coverage.
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
