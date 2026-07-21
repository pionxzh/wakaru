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
