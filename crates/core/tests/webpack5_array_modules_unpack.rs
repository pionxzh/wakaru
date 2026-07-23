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
fn webpack5_array_table_with_only_zero_param_factories() {
    // Webpack omits unused (module, exports, require) parameters, so a valid
    // array module table can consist entirely of zero-parameter factories
    // (e.g. a side-effect-only dependency). Detection must rely on the
    // require-function/table relationship, not on factory arity.
    let source = r#"
(() => {
    var e = [
        ,
        () => { console.log("side effect dep"); }
    ];
    var t = {};
    function r(o) {
        var n = t[o];
        if (n !== undefined) return n.exports;
        var c = t[o] = { exports: {} };
        return e[o](c, c.exports, r), c.exports;
    }
    r.m = e;
    r(1);
})();
"#;

    let pairs = expect_unpack(source, "bundle.js");
    assert!(
        pairs.iter().any(|(name, _)| name == "module-1.js"),
        "zero-parameter array table should still extract, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    let mod_1 = pairs
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .expect("module-1.js should exist");
    assert!(
        mod_1.1.contains("side effect dep"),
        "factory body should be recovered, got:\n{}",
        mod_1.1
    );
}

#[test]
fn indexed_callback_dispatcher_is_not_mistaken_for_webpack5() {
    // An ordinary dispatch table calls `handlers[i](event)` — a computed member
    // call with an argument, but not a webpack require function. It must not be
    // destructively unpacked.
    let source = r#"
(() => {
    var handlers = [
        (event) => { console.log(event); },
        (event) => { return event + 1; }
    ];
    function dispatch(i, event) {
        return handlers[i](event);
    }
    console.log(dispatch(0, "click"));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "indexed dispatcher must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn dispatcher_passing_module_exports_pair_is_not_webpack5() {
    // A dispatcher may naturally pass `(module, module.exports)`, matching the
    // webpack argument pair. Detection must instead require webpack's module
    // lifecycle (a locally-created `{ exports: {} }` object passed to the
    // table and its `.exports` returned), which a dispatcher — whose call
    // result is returned directly — does not have.
    let source = r#"
(() => {
    var handlers = [
        (module, exports) => { console.log(module); },
        (module, exports) => { return module + 1; }
    ];
    function dispatch(i, module) {
        return handlers[i](module, module.exports);
    }
    console.log(dispatch(0, {}));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "dispatcher passing (module, module.exports) must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn mutating_dispatcher_returning_context_exports_is_not_webpack5() {
    // A dispatcher that mutates a caller-supplied context and returns
    // `context.exports` pairs a table invocation with a returned `.exports`
    // member — the two loose facts alone. It must not match: webpack's require
    // function *creates* the module object locally (`var m = cache[id] =
    // { exports: {} }`), while this context is a parameter.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "h0"; },
        (ctx) => { ctx.exports = ctx.exports + "!"; }
    ];
    function dispatch(i, context) {
        handlers[i](context);
        return context.exports;
    }
    console.log(dispatch(0, { exports: "" }));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "mutating dispatcher returning context.exports must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn shadowed_block_local_does_not_complete_dispatcher_lifecycle() {
    // Lifecycle facts must belong to the same *binding*, not the same
    // spelling: here an inner block-local `context` supplies the
    // object-creation fact while the outer parameter `context` supplies the
    // invocation and returned-exports facts. Merging them by name would
    // classify this ordinary dispatcher as webpack.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "h0"; },
        (ctx) => { ctx.exports = ctx.exports + "!"; }
    ];
    function dispatch(i, context) {
        { const context = { exports: {} }; console.log(context); }
        handlers[i](context);
        return context.exports;
    }
    console.log(dispatch(0, { exports: "" }));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "shadowed block-local must not complete the module lifecycle, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn dispatcher_reassigning_its_own_parameter_is_not_webpack5() {
    // The creation fact must come from a binding declared in the function
    // body. Assigning a fresh `{ exports: {} }` into a caller-supplied
    // parameter is context mutation, not webpack's module lifecycle.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "h0"; },
        (ctx) => { ctx.exports = ctx.exports + "!"; }
    ];
    function dispatch(i, context) {
        context = { exports: {} };
        handlers[i](context);
        return context.exports;
    }
    console.log(dispatch(0, null));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "parameter reassignment must not carry the creation fact, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn dispatcher_param_shadowing_table_name_is_not_webpack5() {
    // The dispatcher's parameter is spelled like the outer handler table, so a
    // probe that dropped parameters would resolve `handlers[i](...)` to the
    // outer table and complete the lifecycle. The parameter shadows the table
    // — this is an ordinary dispatcher and must pass through.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "ok"; }
    ];
    function dispatch(handlers, i) {
        var context = { exports: {} };
        handlers[i](context);
        return context.exports;
    }
    console.log(dispatch(handlers, 0));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "param shadowing the table name must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn dispatcher_var_redeclaring_param_is_not_webpack5() {
    // `var context` in the body shares the parameter's binding — JavaScript
    // hoists both to the same function-scope variable. A caller-supplied
    // parameter must not gain the creation fact through a `var` redeclaration.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "h0"; }
    ];
    function dispatch(i, context) {
        var context = { exports: {} };
        handlers[i](context);
        return context.exports;
    }
    console.log(dispatch(0, null));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "var redeclaration of a param must not carry the creation fact, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn named_fn_expr_self_binding_is_not_the_table() {
    // Inside `function handlers(i) { ... }` the name `handlers` is the
    // function expression's own self-binding, shadowing the unrelated outer
    // array. `handlers[i](...)` here indexes the *function object* (whose
    // element is installed after the fact), not the outer table.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "outer"; }
    ];
    var dispatch = function handlers(i) {
        var context = { exports: {} };
        handlers[i](context);
        return context.exports;
    };
    dispatch[0] = (ctx) => { ctx.exports = "inner"; };
    console.log(dispatch(0));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "fn-expr self-binding must not be mistaken for the table, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn class_expr_name_shadowing_table_is_not_webpack5() {
    // A class expression's name is visible inside its methods and shadows the
    // outer array there — same identity rule as the fn-expr self-binding, one
    // construct over.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "outer"; }
    ];
    var D = class handlers {
        static dispatch(i) {
            var context = { exports: {} };
            handlers[i](context);
            return context.exports;
        }
    };
    handlers[0] = (ctx) => { ctx.exports = "static"; };
    console.log(D.dispatch(0));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "class-expr name must not be mistaken for the table, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn closure_over_enclosing_param_is_not_the_table() {
    // The inner function invokes `handlers`, but that resolves to the
    // *enclosing* function's parameter — a caller-supplied table, not the
    // region-level modules container. Only the region-level binding's
    // identity counts as the table.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "outer"; }
    ];
    function make(handlers) {
        return function (i) {
            var context = { exports: {} };
            handlers[i](context);
            return context.exports;
        };
    }
    console.log(make([(ctx) => { ctx.exports = "inner"; }])(0));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "closure over an enclosing param must not be mistaken for the table, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn dispatcher_writing_enclosing_context_is_not_webpack5() {
    // `context` is declared in the enclosing scope, not in `dispatch`.
    // Webpack's module object is always the require function's own variable —
    // an assignment into shared enclosing state must not carry the creation
    // fact, even though the binding resolves inside the region.
    let source = r#"
(() => {
    var handlers = [
        (ctx) => { ctx.exports = "ok"; }
    ];
    var context;
    function dispatch(i) {
        context = { exports: {} };
        handlers[i](context);
        return context.exports;
    }
    console.log(dispatch(0));
})();
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "enclosing-scope context must not carry the creation fact, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
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

#[test]
fn dispatcher_creating_local_module_object_is_not_webpack5() {
    // A dispatcher may create its own `{ exports: {} }` context, pass it to
    // the table, and return `context.exports` — satisfying every lifecycle
    // fact except webpack's module-cache write. Without requiring the cache
    // write (`m = cache[id] = {...}` indexed by the binding that also indexes
    // the table), this ordinary handler pattern was destructively unpacked
    // into module-0.js + entry.js.
    let source = r#"
var handlers = [function (ctx) { ctx.exports.ready = true; }];
function dispatch(i) {
    var context = { exports: {} };
    handlers[i](context);
    return context.exports;
}
console.log(dispatch(0).ready);
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "dispatcher creating a local module object must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn memoizing_dispatcher_without_shared_index_is_not_webpack5() {
    // Even with a cache write, the cache index must be the same binding that
    // indexes the table invocation — webpack's require memoizes and invokes
    // under one `moduleId`. A dispatcher caching under an unrelated key does
    // not have that relationship.
    let source = r#"
var handlers = [function (ctx) { ctx.exports.ready = true; }];
var slots = {};
var nextSlot = 0;
function dispatch(i) {
    var key = nextSlot++;
    var context = slots[key] = { exports: {} };
    handlers[i](context);
    return context.exports;
}
console.log(dispatch(0).ready);
"#;
    let pairs = expect_unpack(source, "app.js");
    assert!(
        !pairs
            .iter()
            .any(|(name, _)| name.starts_with("module-") || name == "entry.js"),
        "cache write under an unrelated key must not unpack as webpack5, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn huge_array_concat_offset_does_not_panic() {
    // `Array(1e100).concat([...])` saturates a float→usize cast to usize::MAX
    // and `id_offset + index` overflows. The offset must be rejected (the
    // bundle passes through) instead of panicking.
    let source = r#"
(() => {
    var __webpack_modules__ = Array(1e100).concat([
        ((module, exports, __webpack_require__) => { module.exports = 1; })
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
    __webpack_require__(1e100);
})();
"#;
    let pairs = expect_unpack(source, "bundle.js");
    assert!(
        !pairs.iter().any(|(name, _)| name.starts_with("module-")),
        "overflowing offsets must fall through, got {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}
