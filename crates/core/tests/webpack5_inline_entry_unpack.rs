//! Webpack 5 inlines the entry module into the bootstrap tail when it doesn't
//! need an IIFE wrapper: `var __webpack_exports__ = {};` followed by the entry
//! statements. These bundles previously produced no entry.js.

use wakaru_core::{unpack, DecompileOptions};

fn expect_unpack(source: &str, filename: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: filename.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

fn entry_of(pairs: &[(String, String)]) -> &str {
    &pairs
        .iter()
        .find(|(name, _)| name == "entry.js")
        .unwrap_or_else(|| {
            panic!(
                "entry.js should exist, got {:?}",
                pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
            )
        })
        .1
}

#[test]
fn webpack5_inline_startup_becomes_entry_module() {
    let source = r#"
(() => {
    var __webpack_modules__ = ({
        1: ((__unused_webpack_module, exports) => {
            exports.greet = function (name) { return "hi " + name; };
        }),
        2: ((__unused_webpack_module, exports, __webpack_require__) => {
            var dep = __webpack_require__(1);
            exports.shout = function () { return dep.greet("x"); };
        })
    });
    var __webpack_module_cache__ = {};
    function __webpack_require__(moduleId) {
        var cachedModule = __webpack_module_cache__[moduleId];
        if (cachedModule !== undefined) {
            return cachedModule.exports;
        }
        var module = __webpack_module_cache__[moduleId] = { exports: {} };
        __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
        return module.exports;
    }
    var __webpack_exports__ = {};
    var lib = __webpack_require__(2);
    console.log(lib.shout());
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-2.js"),
        "entry should require ./module-2.js, got:\n{entry}"
    );
    assert!(
        !entry.contains("__webpack_module_cache__") && !entry.contains("cachedModule"),
        "entry must not contain runtime code, got:\n{entry}"
    );
}

#[test]
fn webpack5_inline_startup_works_with_array_modules() {
    let source = r#"
(() => {
    var __webpack_modules__ = ([
        ,
        ((__unused_webpack_module, exports) => {
            exports.first = function () { return 1; };
        }),
        ((__unused_webpack_module, exports, __webpack_require__) => {
            var dep = __webpack_require__(1);
            exports.second = function () { return dep.first() + 1; };
        })
    ]);
    var __webpack_module_cache__ = {};
    function __webpack_require__(moduleId) {
        var cachedModule = __webpack_module_cache__[moduleId];
        if (cachedModule !== undefined) {
            return cachedModule.exports;
        }
        var module = __webpack_module_cache__[moduleId] = { exports: {} };
        __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
        return module.exports;
    }
    var __webpack_exports__ = {};
    var lib = __webpack_require__(2);
    console.log(lib.second());
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-2.js"),
        "entry should require ./module-2.js, got:\n{entry}"
    );
}

#[test]
fn webpack5_inline_startup_with_mangled_names_and_merged_decls() {
    // Minified shape: mangled require binding, exports declarator merged with
    // the first entry declarator (`var o = {}, e = r(7);`).
    let source = r#"
(() => {
    var t = {
        7: (t, e) => {
            e.mix = function (a, b) { return a + b; }
        },
        9: (t, e, r) => {
            var n = r(7);
            e.run = function () { return n.mix(2, 3); }
        }
    };
    var n = {};
    function r(e) {
        var o = n[e];
        if (o !== undefined) return o.exports;
        var i = n[e] = { exports: {} };
        return t[e](i, i.exports, r), i.exports;
    }
    var o = {}, e = r(9);
    console.log(e.run());
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-9.js"),
        "mangled require calls should be rewritten, got:\n{entry}"
    );
    assert!(
        !entry.contains("exports:"),
        "entry must not contain the runtime module cache, got:\n{entry}"
    );
}

#[test]
fn webpack5_inline_startup_rewrites_exports_binding() {
    // A real exports anchor is populated through webpack's export helpers
    // (`require.r` / `require.d`), which is what marks it as the exports object.
    let source = r#"
(() => {
    var __webpack_modules__ = ({
        3: ((__unused_webpack_module, exports) => {
            exports.value = 5;
        })
    });
    var __webpack_module_cache__ = {};
    function __webpack_require__(moduleId) {
        var cachedModule = __webpack_module_cache__[moduleId];
        if (cachedModule !== undefined) {
            return cachedModule.exports;
        }
        var module = __webpack_module_cache__[moduleId] = { exports: {} };
        __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
        return module.exports;
    }
    var __webpack_exports__ = {};
    __webpack_require__.r(__webpack_exports__);
    __webpack_require__.d(__webpack_exports__, { doubled: () => doubled });
    var dep = __webpack_require__(3);
    var doubled = dep.value * 2;
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        !entry.contains("__webpack_exports__"),
        "the exports binding should be normalized, got:\n{entry}"
    );
    assert!(
        entry.contains("./module-3.js"),
        "the import should be recovered, got:\n{entry}"
    );
}

#[test]
fn webpack5_inline_startup_without_exports_anchor() {
    // Minifiers drop the unused `var __webpack_exports__ = {}` anchor, and pack
    // the runtime member assignments into a single comma sequence. The entry is
    // whatever follows the last runtime definition and calls the require binding.
    let source = r#"
(() => {
    var e = [
        ,
        (e, t) => { t.mix = function (a, b) { return a + b; }; },
        (e, t, r) => { var n = r(1); t.run = function () { return n.mix(2, 3); }; }
    ];
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e, r.d = (e, t) => {}, (() => {})();
    var o = r(2);
    console.log(o.run());
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-2.js"),
        "no-anchor entry should require ./module-2.js, got:\n{entry}"
    );
    assert!(
        !entry.contains("exports:") && !entry.contains("r.m ="),
        "entry must not include runtime code, got:\n{entry}"
    );
}

#[test]
fn webpack5_inline_entry_local_is_not_mistaken_for_exports_anchor() {
    // Minifiers drop the real `__webpack_exports__` anchor. The entry here
    // opens with a real import followed by an ordinary empty-object local
    // (`const collector = {}`). The extractor must NOT treat that local as the
    // anchor — doing so discards the import and rewrites the local to `exports`.
    let source = r#"
(() => {
    var e = { 1: (m, x) => { x.A = 5; } };
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    const dep = r(1);
    const collector = {};
    collector.value = dep.A;
    console.log(collector);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-1.js"),
        "the leading import must be preserved, got:\n{entry}"
    );
    assert!(
        !entry.contains("export const value") && !entry.contains("console.log(exports)"),
        "the entry local must not be rewritten to exports, got:\n{entry}"
    );
}

#[test]
fn webpack5_nested_export_helper_does_not_prove_outer_local_is_anchor() {
    // A nested `function helper(a) { r.d(a, ...) }` uses a *parameter* named
    // `a`, which must not be taken as evidence that an unrelated outer
    // `const a = {}` is webpack's exports object.
    let source = r#"
(() => {
    var e = { 1: (m, x) => { x.A = 5; } };
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    const a = {};
    const dep = r(1);
    a.value = dep.A;
    function helper(a) { r.d(a, { x: () => 1 }); }
    console.log(a, helper);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-1.js"),
        "the import must be preserved, got:\n{entry}"
    );
    assert!(
        !entry.contains("export const value") && !entry.contains("console.log(exports"),
        "the outer local must not be rewritten to exports, got:\n{entry}"
    );
}

#[test]
fn webpack5_block_scoped_shadow_does_not_prove_outer_local_is_anchor() {
    // A nested block redeclares `a` with `const` and passes *that* binding to
    // `r.d`. Lexical shadowing means this is evidence about a different
    // variable — the unrelated outer `const a = {}` must not be rewritten to
    // exports.
    let source = r#"
(() => {
    var e = { 1: (m, x) => { x.A = 5; } };
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    const a = {};
    const dep = r(1);
    a.value = dep.A;
    {
        const a = { local: true };
        r.d(a, { x: () => 1 });
    }
    console.log(a);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-1.js"),
        "the import must be preserved, got:\n{entry}"
    );
    assert!(
        !entry.contains("export const value") && !entry.contains("console.log(exports"),
        "the outer local must not be rewritten to exports, got:\n{entry}"
    );
}

#[test]
fn webpack5_catch_param_shadow_does_not_prove_outer_local_is_anchor() {
    // A catch clause parameter named `a` passed to `r.d` shadows the outer
    // binding just like a block-scoped `const` — it must not count as anchor
    // evidence for the outer `const a = {}`.
    let source = r#"
(() => {
    var e = { 1: (m, x) => { x.A = 5; } };
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    const a = {};
    const dep = r(1);
    a.value = dep.A;
    try {
        throw { shadow: true };
    } catch (a) {
        r.d(a, { x: () => 1 });
    }
    console.log(a);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-1.js"),
        "the import must be preserved, got:\n{entry}"
    );
    assert!(
        !entry.contains("export const value") && !entry.contains("console.log(exports"),
        "the outer local must not be rewritten to exports, got:\n{entry}"
    );
}

#[test]
fn webpack5_named_class_expression_shadow_does_not_prove_anchor() {
    // A named class expression introduces its own binding, visible inside the
    // class body and static blocks. `class a { static { r.d(a, ...) } }` is
    // evidence about the class binding, not the unrelated outer
    // `const a = {}`.
    let source = r#"
(() => {
    var e = { 1: (m, x) => { x.A = 5; } };
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    const a = {};
    const dep = r(1);
    a.value = dep.A;
    const C = class a {
        static { r.d(a, { x: () => 1 }); }
    };
    console.log(a, C);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-1.js"),
        "the import must be preserved, got:\n{entry}"
    );
    assert!(
        !entry.contains("export const value") && !entry.contains("console.log(exports"),
        "the outer local must not be rewritten to exports, got:\n{entry}"
    );
}

#[test]
fn webpack5_closure_helper_call_does_not_prove_anchor() {
    // A nested function capturing the outer `const a = {}` calls `r.d(a, ...)`
    // but may never run. Webpack emits export helpers in the startup scope
    // itself, so closure evidence must not prove the anchor — even though the
    // captured binding really is the outer one.
    let source = r#"
(() => {
    var e = { 1: (m, x) => { x.A = 5; } };
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    const a = {};
    function helper() {
        r.d(a, { x: () => 1 });
    }
    const dep = r(1);
    a.value = dep.A;
    console.log(a, helper);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        entry.contains("./module-1.js"),
        "the import must be preserved, got:\n{entry}"
    );
    assert!(
        !entry.contains("export const value") && !entry.contains("console.log(exports"),
        "the captured local must not be rewritten to exports, got:\n{entry}"
    );
}

#[test]
fn webpack5_no_startup_after_runtime_is_not_an_entry() {
    // If nothing after the runtime calls the require binding, no entry module
    // should be synthesized (modules still extract).
    let source = r#"
(() => {
    var e = [
        ,
        (e, t) => { t.value = 1; }
    ];
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    assert!(
        pairs.iter().any(|(name, _)| name == "module-1.js"),
        "modules should still extract, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(
        !pairs.iter().any(|(name, _)| name == "entry.js"),
        "no entry.js should be synthesized without a startup call, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn arrow_require_with_array_table_recovers_entry() {
    // Arrow-bound require: detection plans the require function once and
    // entry extraction reuses the plan, so any accepted shape also recovers
    // its inline startup.
    let source = r#"
(() => {
    var __webpack_modules__ = [, (module, exports) => { exports.value = 7; }];
    var __webpack_module_cache__ = {};
    var __webpack_require__ = (moduleId) => {
        var cached = __webpack_module_cache__[moduleId];
        if (cached !== undefined) return cached.exports;
        var module = __webpack_module_cache__[moduleId] = { exports: {} };
        __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
        return module.exports;
    };
    var __webpack_exports__ = {};
    var lib = __webpack_require__(1);
    console.log(lib.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js"),
        "arrow-require startup must be recovered, got:\n{entry}"
    );
}

#[test]
fn fn_expr_require_with_object_table_recovers_entry() {
    // Object containers use the same lifecycle proof as arrays, and the
    // require-function planner must understand a var-bound function
    // expression.
    let source = r#"
(() => {
    var __webpack_modules__ = ({
        1: (module, exports) => { exports.value = 9; }
    });
    var __webpack_module_cache__ = {};
    var __webpack_require__ = function (moduleId) {
        var cached = __webpack_module_cache__[moduleId];
        if (cached !== undefined) return cached.exports;
        var module = __webpack_module_cache__[moduleId] = { exports: {} };
        __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
        return module.exports;
    };
    var __webpack_exports__ = {};
    var lib = __webpack_require__(1);
    console.log(lib.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js"),
        "fn-expr require startup must be recovered via the loose locate, got:\n{entry}"
    );
}

#[test]
fn ordinary_function_map_object_is_not_webpack5() {
    // Analytics-style scripts carry small object-literal method tables inside
    // a large IIFE. A function-valued object alone is not a webpack module
    // table without webpack's cache/invoke/return require lifecycle.
    let source = r#"
(function () {
    var handlers = {
        track: function (a, b, c) {
            return dispatcher.track(a, b, c);
        }
    };
    function Registry() {
        this.entries = {};
    }
    var dispatcher = new Registry();
    window.api = handlers;
})();
"#;

    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "ordinary function map must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}

#[test]
fn directly_invoked_require_marks_called_module_as_entry() {
    // Some webpack/minifier combinations inline the entire require runtime as
    // the final IIFE and invoke it with the entry id. That function body is
    // runtime machinery, not a synthetic entry module.
    let source = r#"
(() => {
    var modules = {
        1: (module, exports) => {
            exports.value = 42;
        }
    };
    var cache = {};
    !function require(id) {
        var cached = cache[id];
        if (cached !== undefined) return cached.exports;
        var module = cache[id] = { exports: {} };
        modules[id].call(module.exports, module, module.exports, require);
        return module.exports;
    }(1);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(!output.has_errors(), "{:?}", output.warnings);
    let pairs = &output.modules;
    assert!(
        pairs.iter().any(|(name, _)| name == "module-1.js"),
        "the genuine webpack module must be extracted, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        !pairs.iter().any(|(name, _)| name == "entry.js"),
        "the require runtime body must not become entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        output
            .provenance
            .iter()
            .any(|module| module.filename == "module-1.js" && module.is_entry),
        "the directly supplied module id must be marked as entry: {:?}",
        output.provenance
    );
}

#[test]
fn eager_only_startup_recovers_entry() {
    // webpack 5's eager `import()` startup has no direct require call — only
    // `Promise.resolve().then(r.bind(r, 1))`. The startup gate must count a
    // member call on the require binding as loader use, or the entry (and all
    // its side effects) is silently dropped while the modules still unpack.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = 42; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    Promise.resolve().then(r.bind(r, 1)).then((v) => console.log(v));
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js"),
        "eager-only startup must be recovered with the bound id rewritten, got:\n{entry}"
    );
}

#[test]
fn eager_bound_require_id_is_rewritten() {
    // A mixed startup (direct require + eager import) recovered its entry via
    // the direct call, but the bound loader kept its numeric id:
    // `require.bind(require, 2)` invokes the host require with a bare module
    // id once the webpack runtime is gone. The bound argument gets the same
    // filename rewrite as direct calls.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = 41; }, (e) => { e.exports = 42; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    var d = r(1);
    console.log(d);
    Promise.resolve().then(r.bind(r, 2)).then((v) => console.log(v));
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js") && entry.contains("./module-2.js"),
        "both the direct and the bound module ids must be rewritten, got:\n{entry}"
    );
    assert!(
        !entry.contains("bind(require, 2)"),
        "bound require id must not survive as a numeric id, got:\n{entry}"
    );
}

#[test]
fn entry_scope_shadow_does_not_move_runtime_boundary() {
    // The runtime boundary is the last `require.<member> = ...` assignment.
    // A nested function in the entry shadowing the require binding
    // (`function decorate(r) { r.flag = true; }`) must not count as a runtime
    // definition — that would slice the startup after it and silently drop
    // the real loads before it.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { mark: (o) => { o.flag = true; } }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    var dep = r(1);
    function decorate(r) { r.flag = true; }
    decorate(dep);
    console.log(dep.flag);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js") && entry.contains("decorate"),
        "the whole startup (load + shadowing helper) must be recovered, got:\n{entry}"
    );
}

#[test]
fn entry_closure_capturing_require_does_not_move_runtime_boundary() {
    // The raw require binding is available to webpack entry source. A dormant
    // entry closure can therefore capture and mutate the real binding before
    // the first module load. Its body is not executed while the bootstrap
    // boundary is established and must not be classified as runtime setup.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    function decorate() { r.instrumented = true; }
    var dep = r(1);
    decorate();
    console.log(dep.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js")
            && entry.contains("decorate")
            && entry.contains("require.instrumented = true"),
        "the capturing closure and module load must both stay in startup, got:\n{entry}"
    );
}

#[test]
fn entry_require_mutation_before_load_stays_in_startup() {
    // `__webpack_require__` is available to entry source. A source-authored
    // property write can therefore precede the first module load and must not
    // be grouped with webpack's known runtime definitions.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    r.instrumented = true;
    var dep = r(1);
    console.log(dep.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("require.instrumented = true") && entry.contains("./module-1.js"),
        "pre-load require mutation and module load must both stay in startup, got:\n{entry}"
    );
}

#[test]
fn named_exports_anchor_precedes_known_require_property_override() {
    // In readable webpack output the named exports object is an explicit
    // producer boundary. Entry source after it may even override a property
    // name that webpack itself uses; the anchor must win over shape guessing.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    var __webpack_exports__ = {};
    r.p = "/entry-owned/";
    var dep = r(1);
    console.log(dep.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("require.p = \"/entry-owned/\"") && entry.contains("./module-1.js"),
        "the named anchor must keep a known-property override in startup, got:\n{entry}"
    );
}

#[test]
fn repeated_runtime_property_assignment_starts_entry_override() {
    // `__webpack_public_path__ = ...` makes webpack emit its own `require.p`
    // initialization followed by the source-authored override. The repeated
    // static path is the producer-visible transition into entry code.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    r.p = "";
    r.p = "/entry-owned/";
    var dep = r(1);
    console.log(dep.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("require.p = \"/entry-owned/\"")
            && !entry.contains("require.p = \"\"")
            && entry.contains("./module-1.js"),
        "the source override, but not runtime initialization, must stay in startup, got:\n{entry}"
    );
}

#[test]
fn dormant_getter_require_mutation_does_not_move_runtime_boundary() {
    // Getter/setter bodies do not execute when their object literal is
    // created. A captured require mutation inside an accessor must not make
    // the containing declaration look like runtime setup.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    var hooks = {
        get value() {
            r.instrumented = true;
            return "hook";
        }
    };
    var dep = r(1);
    console.log(dep.value, hooks.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("hooks =") && entry.contains("require.instrumented = true"),
        "the accessor-bearing declaration must remain in startup, got:\n{entry}"
    );
    assert!(
        entry.contains("./module-1.js") && entry.contains("hooks.value"),
        "the recovered entry must retain both the load and defined getter binding, got:\n{entry}"
    );
}

#[test]
fn immediately_invoked_runtime_assignment_stays_before_startup() {
    // Webpack wraps some runtime setup in IIFEs. Skipping dormant closures
    // must not hide assignments that are actually executed by such a wrapper.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    (() => {
        r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key);
    })();
    var dep = r(1);
    console.log(dep.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js") && !entry.contains("hasOwnProperty"),
        "executed runtime wrapper must stay outside the recovered entry, got:\n{entry}"
    );
}

#[test]
fn startup_merged_into_runtime_sequence_is_recovered() {
    // Terser merges consecutive expression statements, so the last runtime
    // definitions and the startup can share one comma sequence:
    // `r.o = ..., r.d = ..., loadEntry()`. Statement-granular boundary
    // slicing marked the whole statement as runtime and silently dropped the
    // startup tail.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 7 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key), r.d = (exp, def) => { exp[def] = 1; }, console.log(r(1).value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js") && !entry.contains("r.d"),
        "the sequence tail after the last runtime assignment must become the entry, got:\n{entry}"
    );
}

#[test]
fn library_anchor_consumed_by_wrapper_keeps_its_declaration() {
    // In a minified CommonJS library the export-helper anchor is also the
    // library value: `var t = {}; r.r(t); r.d(t, ...); module.exports = t;`.
    // Renaming it to a free `exports` and dropping the declaration leaves
    // `module.exports = exports` with no `exports` binding — a runtime
    // ReferenceError after ESM recovery. Such an anchor must stay untouched.
    let source = r#"
(() => {
    var e = ({
        1: (e, o) => { o.greet = () => "hi"; }
    });
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.d = (exp, def) => { for (var k in def) exp[k] = def[k]; };
    var t = {};
    r.d(t, { greet: () => r(1).greet });
    module.exports = t;
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("= {}"),
        "the anchor declaration must be kept when the wrapper consumes it, got:\n{entry}"
    );
    assert!(
        entry.contains("export default t"),
        "the wrapper tail must survive (as the recovered default export), got:\n{entry}"
    );
    assert!(
        !entry.contains("export default exports") && !entry.contains("module.exports = exports"),
        "the anchor must not be renamed to a free `exports`, got:\n{entry}"
    );
}

#[test]
fn inlined_esmodule_marker_is_dropped_from_entry() {
    // When the minifier inlines the unused exports anchor into the marker
    // call (`var t = {}; r.r(t)` becomes `r.r({})`), the call is a semantic
    // no-op and must not survive in the recovered entry — it rides the same
    // sequence as the last runtime definition.
    let source = r#"
(() => {
    var e = [, (e) => { e.exports = { value: 3 }; }];
    var n = {};
    function r(o) {
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = { exports: {} };
        e[o](c, c.exports, r);
        return c.exports;
    }
    r.o = (obj, key) => Object.prototype.hasOwnProperty.call(obj, key), r.r({});
    var v = r(1);
    console.log(v.value);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);
    assert!(
        entry.contains("./module-1.js") && !entry.contains("require.r"),
        "the inlined esModule marker must be dropped, got:\n{entry}"
    );
}
