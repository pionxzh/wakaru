# TypeScript default inheritance

`source.ts` has no explicit constructor. TypeScript 5.9.3 emits the guarded
`.apply(this, arguments) || this` constructor. `generate.sh` reproduces all
three helper delivery modes, each raw and with Terser 5.51.2 compression /
compression plus mangling, using the matrix's `module: true` and
`compress: { defaults: true, unused: false }` settings. These lift the inline
helper factory local into a sequence. The generated JS is consumed by rule/pipeline tests;
those tests do not need npm.

Regenerate with `bash generate.sh`. To test actual runtime behavior, install
tslib 2.8.1 in a temporary directory and run:

```sh
NODE_PATH=<temporary-directory>/node_modules WAKARU=<absolute-binary-path> \
  node --experimental-vm-modules runtime.cjs
```

The runtime oracle executes all nine original modules, their minimal and
standard recoveries, and the native source. It verifies ordinary-function
agreement and intentional differences for null / native-class parents,
overridden `.apply` / `.call`, replacement of the parent's prototype, and
mutation of the child's prototype chain. Minimal must match the lowered input;
standard must match the native source in these cases. This pins the
`native_class_inheritance` policy; it does not claim universal equivalence.

The `static-factory/` child directory adds `Child.make(value)` returning
`new Child(value)` to the same nine compiler/minifier profiles. Its
`runtime.cjs` checks ordinary, native, null, and overridden-apply parents
against the same minimal/lowered and standard/native policy. Two additional
hand-authored cases reassign the outer class variable: the factory must still
construct the original inner constructor, including when its emitted name
was different. Run this oracle with the same environment and VM flag as above.

The `fields/` child directory compiles a subclass with field initializers and
no constructor, without `downlevelIteration` (with it, TypeScript emits an
explicit `constructor(...args)` instead of the default frame). Its
`runtime.cjs` checks ordinary, object-returning, native, null, and
overridden-apply parents, and calls the arrow-function field detached so the
recovered `this` alias is exercised. Run it the same way.
