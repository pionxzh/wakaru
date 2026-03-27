# wakaru-rs extensions over original wakaru

This document covers cases where wakaru-rs handles more patterns than the
original TypeScript implementation, or adds new capabilities with no equivalent
in the original.

---

## `un-es6-class` — extended pattern coverage

### Member-expression super class

The original implementation requires the super class argument to be a plain
identifier.

```js
// original: only handled
var Child = (function(_super) { _inherits(t, _super); ... }(Animal));

// original: NOT handled (super is a member expression)
var Child = (function(_super) { _inherits(t, _super); ... }(module.Component));
```

wakaru-rs accepts any expression as the super class — plain identifiers,
member expressions like `React.Component` or `module.Component`, and so on.

### Inlined inheritance (webpack4 without `_inherits`)

The original only recognises inheritance when a named helper function
(`_inherits`, `_inheritsLoose`, `__extends`) is called.  webpack4 bundled
TypeScript sometimes inlines the inheritance setup directly without any helper:

```js
var Child = (function(_super) {
    // type guard (no helper name, just inline logic)
    if (typeof _super !== "function" && _super !== null) {
        throw new TypeError("...");
    }
    function t() { _super !== null && _super.apply(this, arguments); }
    t.prototype = Object.create(_super !== null && _super.prototype);
    t.prototype.constructor = t;
    _super && (Object.setPrototypeOf
        ? Object.setPrototypeOf(t, _super)
        : t.__proto__ = _super);
    t.prototype.run = function run() { return true; };
    return t;
}(Base));
```

wakaru-rs recognises all three inline forms and converts the result to:

```js
class Child extends Base {
    constructor() { _super !== null && _super.apply(this, arguments); }
    run() { return true; }
}
```

The three skipped patterns are:
- `if (typeof _super !== "function" && _super !== null) { throw ... }` — type guard
- `t.prototype = Object.create(...)` — instance chain setup
- `_super && (Object.setPrototypeOf ? ... : t.__proto__ = _super)` — static chain setup

---

## `smart-inline` — zero-param arrow wrapper inlining

The original `smart-inline` only inlines temp variables within the same block
scope at statement level.  It cannot inline across function boundaries.

wakaru-rs adds a pre-pass (`inline_module_arrow_wrappers`) that specifically
targets zero-param arrow wrappers at module level:

```js
const o = () => r;
```

These are inlined everywhere in the module — including inside nested functions
and classes — because they are pure aliases with no side effects.

**Call-site inlining:**

```js
// input
const o = () => r;
function foo() { return o(); }

// output (o() → (() => r)() → r via second UnIife pass)
function foo() { return r; }
```

**`.a` accessor inlining (webpack4 `require.n` pattern):**

webpack4's `__webpack_require__.n(module)` always defines a property named
literally `'a'` on the returned getter function:

```js
// webpack4 runtime (simplified)
__webpack_require__.n = function(module) {
    var getter = module.__esModule ? () => module.default : () => module;
    Object.defineProperty(getter, 'a', { enumerable: true, get: getter });
    return getter;
};
```

`getter.a` is therefore equivalent to `getter()`.  After rewriting `require.n`
calls, call sites that use `.a` become `o.a`.  Naively inlining `o = () => r`
would produce `(() => r).a` which is `undefined` at runtime.  wakaru-rs
detects the `X.a` accessor pattern and replaces it directly with the inner
identifier:

```js
// input
const o = () => r;
function foo() { return o.a.Children; }

// output (o.a → r, not (() => r).a)
function foo() { return r.Children; }
```

Scope safety: matching uses `(symbol, SyntaxContext)` pairs from SWC's
`resolver` pass, so inner-scope variables with the same name as an outer
arrow wrapper are never incorrectly replaced.

---

## `un-variable-merging` — TDZ-safe for-init extraction

The original checks whether each declarator in `for (var a, b, c; ...)` is
referenced in the loop's test or update expression.  If not referenced, the
declarator is extracted before the loop.

This is unsound when a declarator's initializer depends on another declarator
that must stay in the for-init.  After `VarDeclToLetConst` converts `var` to
`let`/`const`, the extracted declarator would reference a variable not yet
initialized (TDZ violation):

```js
// input
for (var n = 10, a = new Array(n), i = 0; i < n; i++) { ... }
```

The test is `i < n` and the update is `i++`, so the original correctly keeps
`n` and `i` in the for-init and extracts only `a`.  After extraction and
`VarDeclToLetConst`:

```js
// original output — BROKEN
// `n` is still declared inside the for(...) below, not yet executed
const a = new Array(n);  // ReferenceError: n is not defined (TDZ)
for (let n = 10, i = 0; i < n; i++) { ... }

// wakaru-rs output — correct: a depends on n, so both stay in the for-init
for (let n = 10, a = new Array(n), i = 0; i < n; i++) { ... }
```

wakaru-rs uses a fixpoint iteration to compute the full transitive closure of
"must-stay" declarators before extracting anything.

---

## webpack4 unpacker — `require.n` rewriting

The webpack4 unpacker in wakaru-rs adds two post-extraction rewrite passes
that have no equivalent in the original unpacker:

**`RequireIdRewriter`** — replaces numeric module IDs with the resolved module
filename so that `__webpack_require__(42)` becomes `require("./foo")`.

**`RequireNRewriter`** — rewrites `__webpack_require__.n(r)` calls.
`require.n` wraps ES-module-namespace objects in a getter function.  The
rewriter folds the call into an arrow wrapper `const X = () => r`, which is
then eliminated by `smart-inline`'s arrow wrapper pass, leaving direct
references to the module throughout the code.
