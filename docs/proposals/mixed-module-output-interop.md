# Mixed ESM/CommonJS output: which interop rules the output targets

Status: proposed, not started. The goal is recovered output that people can
run again. The recommended first step (document the current contract) is
small. The preferred long-term direction is option C; the `.mjs` layouts
rename too many files. C renames files in every unpack output tree that
keeps a CommonJS module, so it is a next-major candidate. Revisit when
users report that recovered output breaks after a rebuild, or when a major
release is cut.

## Problem

Unpack output can mix module formats. Most modules come back as ESM, but a
module whose exports cannot be recovered safely stays CommonJS. An ESM
module that imports such a sibling has to pick an import form, and the same
form means different things under two interop conventions:

- **Node's rules.** An ESM importer of CommonJS gets `module.exports` as the
  default export, whatever `__esModule` says. Named exports are the names
  cjs-module-lexer finds by scanning the source before it runs. Their values
  are copied once, when the CommonJS module finishes loading.
- **The `__esModule` convention.** Babel introduced it, and TypeScript, swc,
  webpack, and esbuild follow it through their interop helpers. A provider
  marked `__esModule` has its default at `exports.default`. Named imports
  read `module.exports` properties at access time.

Bundlers switch between the two by the importer, not the provider. An
importer that Node would load as ESM (`.mjs`, or `.js` under
`"type": "module"`) gets Node's default. Any other importer gets the
`__esModule` convention.

Running the output again goes through one of two paths. A Node-targeted
tree can run in Node directly. A browser cannot load CommonJS or JSX, so a
browser-targeted tree that keeps either runs only after a bundler builds it
again. That rebuild is where today's layout breaks.

## Measurements

**A recovered tree, rebuilt.** The provider was built by
`producer typescript@5.9.3 module=CommonJS target=ES2020 esModuleInterop`
and bundled with an ESM consumer by `producer webpack@5.111.1` with Terser.
The provider keeps a top-level `typeof this`, so wakaru keeps it CommonJS
and recovers the consumer as ESM with `import p from` and member reads. The
consumer calls a provider function that reassigns an exported `let`, then
reads it. Each layout of the same recovered files was run in Node 24.18,
and bundled and run with esbuild 0.28 and webpack 5.111.1:

| Layout | Node | esbuild | webpack |
|---|---|---|---|
| As emitted today: `.js`, no `package.json` | correct | `TypeError` | `TypeError` |
| Today's files under `"type": "module"` | link error | build error | build error |
| Option C: `"type": "module"`, CommonJS files `.cjs` | correct | correct | correct |
| Module files `.mjs`, CommonJS files `.js`, no `package.json` | correct | correct | correct |

Under `"type": "module"` alone, the CommonJS provider kept as `.js` loads as
ESM, so every runtime fails. Both working layouts rename one side, and every
specifier that names a renamed file moves with it.

**Importer against provider extension.** A minimal provider sets
`__esModule`, exports `count`, and has a `bump()` that reassigns
`exports.count`. The consumer uses `import d` and `import * as ns`, calls
`bump()`, then reads `count` (Node 24.18, esbuild 0.28.0, webpack 5.111.1;
both files hand-written, `hypothetical`):

| Consumer → provider | Node | esbuild | webpack |
|---|---|---|---|
| `consumer.js` → `provider.js` | `d` live, `ns` stale | `d` undefined | `d` undefined |
| `consumer.js` → `provider.cjs` | `d` live, `ns` stale | `d` undefined | `d` undefined |
| `consumer.mjs` → `provider.cjs` | `d` live, `ns` stale | `d` live, `ns` live | `d` live, `ns` live |
| `"type": "module"`, `consumer.js` → `provider.cjs` | `d` live, `ns` stale | `d` live, `ns` live | `d` live, `ns` live |

"Stale" means `ns.count` keeps the value from load time. A name the lexer
does not detect, such as a getter that returns a literal or a call result,
is `undefined` on the namespace. The provider's own `.cjs` extension changes
nothing for the bundlers; the importer decides.

## Current contract

The output targets Node's rules. A default import of a CommonJS sibling is
the live `module.exports`, so member reads through it are correct. The
single-file namespace rewrite (`relative_require_esm_provider` in
`rewrite-assumptions.md`) accepts the lexer risk only because there the
provider is unseen and most likely ESM. In unpack mode the provider is known
to have stayed CommonJS, so the default import is the form that needs no
guess.

The tree has no `package.json`. A module named from a bundle id or input
file keeps a recognized extension, including `.cjs` and `.mjs`
([module-id-extension-policy.md](module-id-extension-policy.md)); every
other module is `.js`. Node loads the `.js` files through syntax detection:
a file with module syntax (`import` or `export` declarations, `import.meta`,
or top-level `await`) runs as ESM, any other file runs as CommonJS. So
Node's rules hold for the tree only while two things are true:

- No `"type": "module"` covers it.
- No module keeps a `.cjs` name but comes back with ESM syntax. Unpack can
  produce that today, and Node then fails to load the module
  ([Known gaps](../unpacking.md#known-gaps), "A `.cjs` name still gets ESM
  syntax").

## What breaks today

- **A rebuild.** esbuild and webpack treat a `.js` importer with no
  `"type"` as non-Node ESM. They apply the `__esModule` convention, so the
  default import of a marked CommonJS sibling becomes `exports.default`,
  usually `undefined`.
- **A `"type": "module"` project.** If the user copies the tree into one,
  every `.js` module that stayed CommonJS is loaded as ESM and fails.
- **A `.cjs` name with ESM syntax**, as above.
- **Transpiling back to CommonJS** (Babel, `tsc` with `esModuleInterop`,
  test runners built on them) applies the `__esModule` convention. Not
  measured.

## Options

### A. Keep Node's rules and document them (recommended now)

State in `docs/unpacking.md` and the CLI reference that the output follows
Node's ESM/CommonJS rules, and that a bundler rebuilding today's tree reads
imports of CommonJS siblings by the `__esModule` convention instead. No
output changes. Adding `"type": "module"` alone does not help (first
table); a user who needs a rebuild has to rename one side by hand, which is
option C or its `.mjs` variant.

### B. Namespace import for a marked CommonJS provider (rejected)

Rewrite `import d` to `import * as d` when the provider stayed CommonJS but
carries `__esModule` and a named surface with no default. This makes a
rebuild with the `__esModule` convention work, and breaks Node:

- A namespace holds only lexer-detected names.
- Its values are a load-time snapshot. In the recovered tree measured
  above, the consumer reads the reassigned `let` through the namespace and
  gets the old value.

It trades the contract's runtime for a different one, so it is rejected
under the current contract.

### C. Emit `"type": "module"` and name every non-module file `.cjs`

Write `{"type": "module"}` at the output root. Name each file by its syntax:

- A file with module syntax (`import` or `export` declarations,
  `import.meta`, or top-level `await`) keeps `.js`. A dynamic `import()` is
  valid CommonJS and does not count.
- Every other file gets the CommonJS form of its extension. That includes
  scripts with no CommonJS references.

The CommonJS form replaces the extension, it does not append one: `a.js`
becomes `a.cjs`, and `src/index.ts` becomes `src/index.cts`. Node strips
types by default and loads a `.ts` file by the package type, so under
`"type": "module"` a CommonJS module named `.ts` loads as ESM and fails
(`exports is not defined`, Node 24.18); `.cts` loads as CommonJS. A name
that already states a format (`.cjs`, `.cts`, `.mjs`, `.mts`) is covered by
the rule below.

`.jsx` and `.tsx` have no CommonJS form, and keeping the name is not safe
either. Node does not run JSX, but bundlers read the package type for these
files too: under `"type": "module"`, esbuild 0.28 loads a CommonJS `.jsx`
file as ESM and fails the build (`No matching export … for import
"default"`), while webpack 5.111.1 still treats it as CommonJS (hand-written
minimal files, `hypothetical`; `.tsx` not measured). C needs a rule for a
CommonJS module that holds JSX before it can ship, for example keeping JSX
out of CommonJS files.

Replacing an extension can collide with a sibling: a bundle with both
`./a.js` (CommonJS) and `./a.cjs`. module-id-extension-policy.md rejected
extension replacement for this reason. Here the rename is necessary, so it
goes through the existing filename dedup, which assigns a stable suffix and
rewrites the specifiers that name the file.

This copies the split Node's syntax detection makes today, so Node runs
every file as it does now. Only the bundlers' view changes. The second
table shows the mechanism: with the importer in Node-mode ESM, Node,
esbuild, and webpack agree that the default import of a CommonJS sibling is
the live `module.exports`. The first table shows the whole recovered tree
running correctly under this layout in all three.

The `.mjs` variant in the first table (module files `.mjs`, CommonJS files
`.js`, no `package.json`) also passes. The working layouts differ in what
they depend on:

| Layout | Files renamed | Depends on |
|---|---|---|
| C: `"type": "module"`, CommonJS files `.cjs` | CommonJS files | The emitted `package.json` staying with the tree. Without it, Node still runs the `.js` files as ESM by syntax detection, but bundlers fall back to the `__esModule` convention. |
| `.mjs` variant | Module files, usually most of the tree | The host project having no `"type": "module"`. Under one, the CommonJS `.js` files load as ESM and fail, which is today's failure mode. |
| `.mjs` and `.cjs` on both sides | Every file | Nothing. Each file states its own format. |

Scripts get `.cjs` on purpose. Turning a script into ESM changes more than
strict mode and top-level `this`. Top-level `var` and function declarations
move from the CommonJS module scope to the ESM one, and a UMD wrapper's
`typeof module` test takes a different branch.

Rules and costs:

- **Module syntax decides first.** A module with module syntax keeps `.js`
  even if it still calls `require` (a require kept after a
  hoisting barrier, or a lazy require inside a function). Such a module is
  broken under either name. It is an existing Known gap, and renaming must
  not try to decide it.
- **A name that states the format is kept as the format.** A single-file
  decompile already keeps a `.cjs`/`.cts` input CommonJS. Unpack should
  follow the same rule. That is the `.cjs` Known gap's own fix, including
  its harder part: deciding per module, from what the bundle says, whether
  it is CommonJS. The mirror case, a `.mjs` module that would
  stay CommonJS, has no producer evidence (`hypothetical`). Record it and do
  not handle it yet.
- **Every reference moves with a rename**: synthesized import specifiers,
  `require("./x.js")` calls the recovered code keeps, and source-map `file`
  fields.
- Every unpack output tree that keeps a CommonJS module changes, and
  fixture references move. The CLI reference, the agent skill, and the docs
  site change together.
- `--raw` output needs no special case: the syntax rule already names most
  raw files `.cjs`. The open question is whether that churn is acceptable.
- Single-file decompile writes no tree and is unaffected.
- The transpile-to-CommonJS case is still unmeasured under this layout.

### D. Option C behind a flag first

Ship C as an opt-in output option. Add a rebuild step to the execution
checks: bundle each recovered tree with esbuild and webpack, run it, and
compare with the source run, as in the first table. Decide from those runs
whether C becomes the default in a major release.

## Related

- The cleaner fix is fewer modules that stay CommonJS. When a provider is
  recovered as ESM, every importer agrees and this question goes away.
- Output validation resolves imports between the emitted files. If C
  lands, it must resolve the renamed `.cjs` files.
