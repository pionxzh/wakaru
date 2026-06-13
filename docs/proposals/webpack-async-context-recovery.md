# Webpack Async Context Recovery

Status: deferred

## Summary

Webpack context modules can combine `require.e(chunkId)` and `require.t(moduleId, mode)` to implement dynamic `import()` over a statically known request map. Wakaru currently preserves that shape because recovering it safely requires cross-module/chunk semantics, not just local helper cleanup.

Observed fixture shape:

```js
const map = {
  "./de-DE.json": [6952, 952],
  "./fr-FR.json": [718, 746]
};

function load(request) {
  if (!require.o(map, request)) {
    return Promise.resolve().then(() => {
      const error = Error(`Cannot find module '${request}'`);
      error.code = "MODULE_NOT_FOUND";
      throw error;
    });
  }

  const entry = map[request];
  const moduleId = entry[0];
  return require.e(entry[1]).then(() => require.t(moduleId, 19));
}

load.keys = () => Object.keys(map);
load.id = 68494;
export default load;
```

## Why It Is Deferred

Unlike `require.d`, `require.n`, `require.r`, and static property reads from `require.t(value, 2)`, this pattern is not a local expression rewrite:

- `require.e(chunkId)` is the async chunk-load boundary.
- `require.t(moduleId, 19)` includes `mode & 1`, so the helper loads `require(moduleId)` after the chunk resolves.
- The request string is resolved through a context map, usually `request -> [moduleId, chunkId]`.
- The generated context function exposes runtime API properties such as `.keys` and `.id`.
- Failed lookup behavior is observable: it returns a rejected promise with `MODULE_NOT_FOUND`.

Collapsing this to `import(...)` without enough metadata can lose chunk/module relationships, error behavior, or the context module API.

## Likely Recovery Direction

Handle this as a dedicated webpack context-module recovery pass, not as part of `UnWebpackInterop`.

Potential staged work:

1. Recognize the context-module shape:
   - top-level request map
   - `hasOwnProperty` guard via `require.o(map, request)`
   - lookup into `[moduleId, chunkId]`
   - `require.e(chunkId).then(() => require.t(moduleId, mode))`
   - `.keys` and `.id` assignments on the context function
2. Normalize local helper residue without changing semantics:
   - `require.o(map, request)` -> `Object.prototype.hasOwnProperty.call(map, request)`
   - name temporary bindings from map entry roles when safe
3. Only recover source-like `import()` if module ids and chunk ids can be mapped back to stable request paths.

## Non-Goals For Now

- Do not rewrite `require.e(...).then(...)` into `import(...)` solely from expression shape.
- Do not simplify `require.t(..., 19)` to a module binding; `mode & 1` performs a module load.
- Do not remove `.keys` or `.id` from context functions; callers may rely on them.
