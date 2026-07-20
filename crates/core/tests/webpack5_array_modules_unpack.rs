//! Webpack 5 emits the modules container as a sparse array (instead of an
//! object) when module ids are dense numerics — `Template.getModulesArrayBounds`
//! — optionally wrapped in `Array(n).concat([...])` when the smallest id is
//! non-zero. See https://github.com/pionxzh/wakaru/issues/200.

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

#[test]
fn webpack5_entry_bundle_with_array_modules() {
    // Shape produced by webpack 5 with dense numeric module ids: a sparse
    // array with a hole where the inlined entry module used to live.
    let source = r#"
(() => {
    var __webpack_modules__ = ([
        ,
        ((__unused_webpack_module, exports) => {
            exports.greet = function (name) { return "hi " + name; };
        }),
        ((__unused_webpack_module, exports, __webpack_require__) => {
            var dep = __webpack_require__(1);
            exports.shout = function () { return dep.greet("x"); };
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
    console.log(lib.shout());
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"module-1.js") && filenames.contains(&"module-2.js"),
        "array-form entry bundle should split into modules, got {filenames:?}"
    );

    let mod_2 = pairs
        .iter()
        .find(|(name, _)| name == "module-2.js")
        .expect("module-2.js should exist");
    assert!(
        !mod_2.1.contains("require(1)"),
        "module-2 should not keep raw require(1), got:\n{}",
        mod_2.1
    );
    assert!(
        mod_2.1.contains("./module-1.js"),
        "module-2 should reference ./module-1.js, got:\n{}",
        mod_2.1
    );
}

#[test]
fn webpack5_entry_bundle_with_array_concat_offset() {
    // When the smallest module id is non-zero webpack emits
    // `Array(minId).concat([...])`; ids are offset by minId.
    let source = r#"
(() => {
    var __webpack_modules__ = Array(40).concat([
        ((__unused_webpack_module, exports, __webpack_require__) => {
            var dep = __webpack_require__(41);
            exports.first = function () { return dep.second(); };
        }),
        ((__unused_webpack_module, exports) => {
            exports.second = function () { return 7; };
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
    var lib = __webpack_require__(40);
    console.log(lib.first());
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"module-40.js") && filenames.contains(&"module-41.js"),
        "concat-form ids should be offset by Array(n), got {filenames:?}"
    );

    let mod_40 = pairs
        .iter()
        .find(|(name, _)| name == "module-40.js")
        .expect("module-40.js should exist");
    assert!(
        mod_40.1.contains("./module-41.js"),
        "module-40 should reference ./module-41.js, got:\n{}",
        mod_40.1
    );
}

#[test]
fn webpack5_chunk_push_with_array_modules() {
    let source = r#"
(self.webpackChunk_demo = self.webpackChunk_demo || []).push([[456], [
    ,
    ,
    ((__unused_webpack_module, exports, __webpack_require__) => {
        var dep = __webpack_require__(3);
        exports.later = function () { return dep.sum(3, 4); };
    }),
    ((__unused_webpack_module, exports) => {
        exports.sum = function (x, y) { return x + y; };
    })
]]);
"#;

    let pairs = expect_unpack(source, "chunk.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        pairs.len(),
        2,
        "holey array chunk should yield 2 modules, got {filenames:?}"
    );
    assert!(
        filenames.contains(&"module-2.js") && filenames.contains(&"module-3.js"),
        "array indices should become module ids, got {filenames:?}"
    );

    let mod_2 = pairs
        .iter()
        .find(|(name, _)| name == "module-2.js")
        .expect("module-2.js should exist");
    assert!(
        mod_2.1.contains("./module-3.js"),
        "module-2 should reference ./module-3.js, got:\n{}",
        mod_2.1
    );
}

#[test]
fn webpack5_chunk_push_with_array_concat_offset() {
    let source = r#"
(self.webpackChunk_demo = self.webpackChunk_demo || []).push([[9], Array(70).concat([
    ((__unused_webpack_module, exports, __webpack_require__) => {
        var dep = __webpack_require__(71);
        exports.first = function () { return dep.second(); };
    }),
    ((__unused_webpack_module, exports) => {
        exports.second = function () { return 5; };
    })
])]);
"#;

    let pairs = expect_unpack(source, "chunk.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"module-70.js") && filenames.contains(&"module-71.js"),
        "concat-form chunk ids should be offset by Array(n), got {filenames:?}"
    );

    let mod_70 = pairs
        .iter()
        .find(|(name, _)| name == "module-70.js")
        .expect("module-70.js should exist");
    assert!(
        mod_70.1.contains("./module-71.js"),
        "module-70 should reference ./module-71.js, got:\n{}",
        mod_70.1
    );
}

#[test]
fn webpack5_chunk_array_skips_false_placeholders() {
    // webpack renders a suppressed module source as the literal `false`
    // (`renderModule(...) || "false"` in Template.renderChunkModules).
    let source = r#"
(self.webpackChunk_demo = self.webpackChunk_demo || []).push([[3], [
    ,
    false,
    ((module, exports, __webpack_require__) => {
        exports.value = 12;
    })
]]);
"#;

    let pairs = expect_unpack(source, "chunk.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(
        filenames,
        vec!["module-2.js"],
        "false placeholder should be skipped, not extracted or fatal"
    );
}

#[test]
fn webpack5_commonjs_chunk_with_array_modules() {
    let source = r#"
exports.id = 88, exports.ids = [88], exports.modules = [
    ,
    ((__unused_webpack_module, exports, __webpack_require__) => {
        var dep = __webpack_require__(2);
        exports.top = function () { return dep.base + 1; };
    }),
    ((__unused_webpack_module, exports) => {
        exports.base = 41;
    })
];
"#;

    let pairs = expect_unpack(source, "chunk.js");
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"module-1.js") && filenames.contains(&"module-2.js"),
        "CommonJS chunk array modules should be extracted, got {filenames:?}"
    );

    let mod_1 = pairs
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .expect("module-1.js should exist");
    assert!(
        mod_1.1.contains("./module-2.js"),
        "module-1 should reference ./module-2.js, got:\n{}",
        mod_1.1
    );
}

#[test]
fn generic_callback_array_is_not_mistaken_for_webpack5() {
    // A plain array of zero-parameter callbacks in an IIFE must not trigger
    // webpack5 detection: real module factories receive (module, exports,
    // require) parameters somewhere in the table.
    let source = r#"
(() => {
    var callbacks = [
        () => { console.log(1); },
        () => { console.log(2); }
    ];
    callbacks.forEach(function (cb) { cb(); });
})();
"#;

    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "generic callback array must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}
