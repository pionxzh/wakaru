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
    var dep = __webpack_require__(3);
    __webpack_exports__.doubled = dep.value * 2;
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let entry = entry_of(&pairs);

    assert!(
        !entry.contains("__webpack_exports__"),
        "the exports binding should be normalized, got:\n{entry}"
    );
}
