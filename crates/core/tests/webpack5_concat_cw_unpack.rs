//! webpack 5's `__webpack_require__.cw` CommonJS wrapper: each wrapped
//! CommonJS module splits into its own file and every accessor call becomes
//! `require()` of it.

use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::DecompileOptions;

fn raw_modules(source: &str) -> Vec<(String, String)> {
    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

fn module<'a>(modules: &'a [(String, String)], filename: &str) -> &'a str {
    modules
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "missing {filename}, got {:?}",
                modules.iter().map(|(name, _)| name).collect::<Vec<_>>()
            )
        })
}

fn filenames(modules: &[(String, String)]) -> Vec<&str> {
    let mut names: Vec<&str> = modules.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    names
}

/// Asserts the input stayed one file that still holds the wrapper.
fn assert_not_split(source: &str) {
    let modules = raw_modules(source);
    assert_eq!(
        modules.len(),
        1,
        "the bundle must stay one file, got {:?}",
        filenames(&modules)
    );
    assert!(
        modules[0].1.contains(".call("),
        "the runtime must stay, got:\n{}",
        modules[0].1
    );
}

const RUNTIME: &str = "(body) => {
    var mod;
    return () => {
      if (body) {
        var fn = body;
        body = 0;
        mod = { exports: {} };
        fn.call(mod.exports, mod, mod.exports);
      }
      return mod.exports;
    };
  }";

/// The unminified bootstrap around `wrapped`, which declares the accessors
/// with `__webpack_require__.cw`, and `entry`.
fn unminified(wrapped: &str, entry: &str) -> String {
    format!(
        "(() => {{
  \"use strict\";
  const __webpack_require__ = {{}};
  __webpack_require__.cw = {RUNTIME};
{wrapped}
{entry}
}})();
"
    )
}

const PROVIDER: &str = "
// MODULE: ./lib/provider.js
var provider_namespaceFn = /*#__PURE__*/__webpack_require__.cw(function(module, exports) {
exports.U9 = 0;
exports.$p = bump;
function bump() { exports.U9 += 1; }
exports.Ay = class Widget {
  constructor(name) { this.name = name; }
  label() { return \"widget:\" + this.name; }
};
});
";

const CONSUMER: &str = "
provider_namespaceFn();
function run() {
  const before = (provider_namespaceFn().U9);
  (0,provider_namespaceFn().$p)();
  return [new (provider_namespaceFn().Ay)(\"x\").label(), before];
}
globalThis.sink = Object.freeze({ get run() { return run; } });
";

#[test]
fn unminified_wrapper_splits_into_its_module_comment_path() {
    let modules = raw_modules(&unminified(PROVIDER, CONSUMER));
    assert_eq!(filenames(&modules), ["entry.js", "lib/provider.js"]);

    let provider = module(&modules, "lib/provider.js");
    assert!(
        provider.contains("exports.U9 = 0") && provider.contains("exports.U9 += 1"),
        "the body becomes the module, got:\n{provider}"
    );
    assert!(
        provider.trim_start().starts_with("\"use strict\""),
        "the body ran in the strict bootstrap and stays strict, got:\n{provider}"
    );

    let entry = module(&modules, "entry.js");
    assert!(
        !entry.contains(".cw")
            && !entry.contains("fn.call")
            && !entry.contains("__webpack_require__"),
        "the runtime and the unused require scope leave entry.js, got:\n{entry}"
    );
    // The import-position stub runs first, so it becomes a binding that every
    // later accessor call reads.
    assert!(
        entry.contains("var provider_namespaceFn = require(\"./lib/provider.js\");"),
        "got:\n{entry}"
    );
    assert_eq!(entry.matches("require(").count(), 1, "got:\n{entry}");
    assert!(
        entry.contains("provider_namespaceFn.U9")
            && entry.contains("new provider_namespaceFn.Ay(\"x\")")
            && !entry.contains("provider_namespaceFn()"),
        "got:\n{entry}"
    );
    let stub = entry.find("= require(").unwrap();
    assert!(stub < entry.find("function run").unwrap(), "got:\n{entry}");
}

#[test]
fn accessor_calls_stay_requires_when_the_body_replaces_its_exports_later() {
    // A later call would return the replaced exports; a binding would not.
    let modules = raw_modules(&unminified(
        "var provider_namespaceFn = __webpack_require__.cw(function(module, exports) {
  exports.ready = false;
  setTimeout(() => { module.exports = { ready: true }; });
});",
        "provider_namespaceFn();\nglobalThis.check = () => provider_namespaceFn().ready;",
    ));
    let entry = module(&modules, "entry.js");
    assert_eq!(
        entry.matches("require(\"./module-cw-0.js\")").count(),
        2,
        "got:\n{entry}"
    );
}

#[test]
fn accessor_calls_stay_requires_when_code_runs_before_the_stub() {
    // `setup` could call the accessor before the stub does.
    let modules = raw_modules(&unminified(
        PROVIDER,
        "function setup() { return provider_namespaceFn().U9; }
globalThis.early = setup();
provider_namespaceFn();
globalThis.late = provider_namespaceFn().U9;",
    ));
    let entry = module(&modules, "entry.js");
    assert!(
        !entry.contains("var provider_namespaceFn")
            && entry.matches("require(\"./lib/provider.js\")").count() == 3,
        "got:\n{entry}"
    );
}

#[test]
fn a_pass_through_load_binds_the_module_it_completes() {
    // The relay's body loads the helper on every run, so after the relay's
    // stub the helper is loaded too and the entry can bind it.
    let modules = raw_modules(&unminified(
        "var helper_namespaceFn = __webpack_require__.cw((__unused_webpack_module, exports) => {
  exports.describe = (value) => \"helper:\" + value;
});
var relay_namespaceFn = __webpack_require__.cw((module) => {
  module.exports = (helper_namespaceFn());
});",
        "relay_namespaceFn();
function run() { return (0, helper_namespaceFn().describe)(\"y\"); }
globalThis.sink = { run };",
    ));
    assert_eq!(
        filenames(&modules),
        ["entry.js", "module-cw-0.js", "module-cw-1.js"]
    );
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("var relay_namespaceFn = require(\"./module-cw-1.js\");")
            && entry.contains("var helper_namespaceFn = require(\"./module-cw-0.js\");")
            && entry.contains("(0, helper_namespaceFn.describe)"),
        "got:\n{entry}"
    );
    let relay = module(&modules, "module-cw-1.js");
    assert!(
        relay.contains("module.exports = require(\"./module-cw-0.js\")"),
        "got:\n{relay}"
    );
}

#[test]
fn minified_inline_runtime_splits() {
    // Terser inlines the runtime into its only call.
    let source = r#"(()=>{"use strict";var r=(r=>{var t;return()=>{if(r){var e=r;r=0,t={exports:{}},e.call(t.exports,t,t.exports)}return t.exports}})(function(r,t){function e(r){return"label:"+r}t.U9=void 0,t.$p=function(){t.U9+=1},t.B$=e,t.U9=0,t.Ay=class{constructor(r){this.name=r}label(){return e(this.name)}}});function t(){const t=r().U9;return(0,r().$p)(),[new(r().Ay)("x").label(),(0,r().B$)("x"),t,r().U9]}r(),globalThis.sink=Object.freeze({get run(){return t}})})();"#;
    let modules = raw_modules(source);
    assert_eq!(filenames(&modules), ["entry.js", "module-cw-0.js"]);

    let provider = module(&modules, "module-cw-0.js");
    assert!(
        provider.contains("exports.U9 = 0") && provider.contains("exports.$p"),
        "the factory parameters become module and exports, got:\n{provider}"
    );
    let entry = module(&modules, "entry.js");
    // The stub leads a comma sequence after a function declaration; the
    // sequence splits at it.
    assert!(
        entry.contains("var r = require(\"./module-cw-0.js\");\nglobalThis.sink")
            && entry.contains("const t = r.U9")
            && !entry.contains("r()")
            && !entry.contains(".call("),
        "got:\n{entry}"
    );
}

#[test]
fn shared_runtime_binding_and_module_exports_reassignment() {
    // Terser with several wrapped modules keeps the runtime in a binding.
    // The second body replaces its exports, which the accessor returns.
    let source = r#"(()=>{"use strict";const t=t=>{var r;return()=>{if(t){var e=t;t=0,r={exports:{}},e.call(r.exports,r,r.exports)}return r.exports}};var r=t(function(t,r){r.describe=function(t){return"helper:"+t}}),e=t(function(t,e){t.exports=r()});e();const{describe:n}=e();globalThis.sink={run:n}})();"#;
    let modules = raw_modules(source);
    assert_eq!(
        filenames(&modules),
        ["entry.js", "module-cw-0.js", "module-cw-1.js"]
    );
    let relay = module(&modules, "module-cw-1.js");
    assert!(
        relay.contains("module.exports = require(\"./module-cw-0.js\")"),
        "a call inside a wrapped body becomes a require too, got:\n{relay}"
    );
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("var e = require(\"./module-cw-1.js\");")
            && entry.contains("} = e;")
            && !entry.contains("const t"),
        "the runtime binding leaves with its accessors, got:\n{entry}"
    );
}

#[test]
fn runtime_in_object_literal_keeps_the_other_helpers() {
    let source = r#"(()=>{"use strict";const e={n:t=>{const n=t&&t.__esModule?()=>t.default:()=>t;return e.d(n,{a:n}),n},cw:e=>{var t;return()=>{if(e){var n=e;e=0,t={exports:{}},n.call(t.exports,t,t.exports)}return t.exports}},d:(t,n)=>{for(var r in n)e.o(n,r)&&!e.o(t,r)&&Object.defineProperty(t,r,{enumerable:!0,get:n[r]})},o:(e,t)=>Object.prototype.hasOwnProperty.call(e,t)};var t=e.cw(function(e,t){t.value=1});function n(){return n.c||(n.c=e.n(t()))}t(),globalThis.sink={run:()=>[n()(),t().value]}})();"#;
    let modules = raw_modules(source);
    assert_eq!(filenames(&modules), ["entry.js", "module-cw-0.js"]);
    let entry = module(&modules, "entry.js");
    // `e.n` reads the provider's `__esModule` marker, so the accessor calls
    // stay requires of the CommonJS exports.
    assert!(
        entry.contains("e.n(require(\"./module-cw-0.js\"))")
            && entry.contains("require(\"./module-cw-0.js\").value")
            && entry.contains("d: ")
            && entry.contains("o: ")
            && !entry.contains("cw:"),
        "only the wrapper runtime leaves the require-scope object, got:\n{entry}"
    );
}

#[test]
fn full_pipeline_recovers_the_provider_module() {
    let output = unpack(
        &unminified(PROVIDER, CONSUMER),
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let modules = output.modules;
    assert_eq!(filenames(&modules), ["entry.js", "lib/provider.js"]);
    let provider = module(&modules, "lib/provider.js");
    assert!(
        provider.contains("export"),
        "the provider becomes a module with exports, got:\n{provider}"
    );
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("./lib/provider.js") && !entry.contains("namespaceFn"),
        "got:\n{entry}"
    );
}

#[test]
fn accessor_used_as_a_value_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        PROVIDER,
        "provider_namespaceFn();\nglobalThis.keep = provider_namespaceFn;",
    ));
}

#[test]
fn accessor_called_with_arguments_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        PROVIDER,
        "globalThis.v = provider_namespaceFn(1).U9;",
    ));
}

#[test]
fn reassigned_accessor_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        PROVIDER,
        "provider_namespaceFn = () => ({});\nglobalThis.v = provider_namespaceFn().U9;",
    ));
}

#[test]
fn body_reading_a_bootstrap_binding_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        "const shared = { n: 1 };
var provider_namespaceFn = __webpack_require__.cw(function(module, exports) {
  exports.n = shared.n;
});",
        "globalThis.v = provider_namespaceFn().n;",
    ));
}

#[test]
fn body_reading_the_require_scope_object_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        "var provider_namespaceFn = __webpack_require__.cw(function(module, exports) {
  exports.global = __webpack_require__.g;
});",
        "globalThis.v = provider_namespaceFn().global;",
    ));
}

#[test]
fn arrow_body_reading_this_keeps_the_bundle_whole() {
    // An arrow's `this` is the bootstrap's, not the exports object.
    assert_not_split(&unminified(
        "var provider_namespaceFn = __webpack_require__.cw((module, exports) => {
  exports.self = this;
});",
        "globalThis.v = provider_namespaceFn().self;",
    ));
}

#[test]
fn function_body_reading_this_splits() {
    // A function body's `this` is the exports object, as in a CommonJS file.
    let modules = raw_modules(&unminified(
        "var provider_namespaceFn = __webpack_require__.cw(function(module, exports) {
  exports.self = this === exports;
});",
        "globalThis.v = provider_namespaceFn().self;",
    ));
    assert_eq!(filenames(&modules), ["entry.js", "module-cw-0.js"]);
}

#[test]
fn body_reading_arguments_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        "var provider_namespaceFn = __webpack_require__.cw(function(module, exports) {
  exports.count = arguments.length;
});",
        "globalThis.v = provider_namespaceFn().count;",
    ));
}

#[test]
fn bootstrap_this_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        PROVIDER,
        "globalThis.v = [provider_namespaceFn().U9, this];",
    ));
}

#[test]
fn bootstrap_binding_named_require_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        PROVIDER,
        "function load(require) { return require; }\nglobalThis.v = [provider_namespaceFn().U9, load];",
    ));
}

#[test]
fn replaced_runtime_member_keeps_the_bundle_whole() {
    // A later write could make the accessor something else.
    assert_not_split(&unminified(
        PROVIDER,
        "__webpack_require__.cw = (f) => f;\nglobalThis.v = provider_namespaceFn().U9;",
    ));
}

#[test]
fn escaping_require_scope_object_keeps_the_bundle_whole() {
    assert_not_split(&unminified(
        PROVIDER,
        "globalThis.scope = __webpack_require__;\nglobalThis.v = provider_namespaceFn().U9;",
    ));
}

// ========================================================================
// Wrapped modules beside a module table
// ========================================================================

/// A minified bootstrap with a module table (module 10, CommonJS, and module
/// 20, which replaces its exports later) around `startup`, which runs after
/// webpack's runtime helpers on the require function `n`.
// shape: producer webpack@5.112.2 mode production, concatenateModules, Terser
// (the table, the `n.cw` install, and the declarations reduced to a minimum)
fn table_bootstrap(startup: &str) -> String {
    format!(
        "(()=>{{\"use strict\";var e={{10(e,t){{t.tag=function(e){{return\"[\"+e+\"]\"}}}},\
20(e,t){{e.exports={{v:1}},globalThis.later=()=>{{e.exports={{v:2}}}}}}}};\
const t={{}};function n(o){{const s=t[o];if(void 0!==s)return s.exports;\
const r=t[o]={{exports:{{}}}};return e[o].call(r.exports,r,r.exports,n),r.exports}}\
n.m=e,n.cw=e=>{{var t;return()=>{{if(e){{var n=e;e=0,t={{exports:{{}}}},n.call(t.exports,t,t.exports)}}return t.exports}}}},\
n.d=(e,t)=>{{for(var o in t)n.o(t,o)&&!n.o(e,o)&&Object.defineProperty(e,o,{{enumerable:!0,get:t[o]}})}},\
n.o=(e,t)=>Object.prototype.hasOwnProperty.call(e,t),\
n.r=e=>{{Object.defineProperty(e,\"__esModule\",{{value:!0}})}};\
{startup}}})();"
    )
}

/// Asserts the table split without moving a wrapped module: the wrappers
/// stay in entry.js.
fn assert_wrappers_stay(source: &str) {
    let modules = raw_modules(source);
    assert!(
        filenames(&modules).contains(&"module-10.js")
            && !filenames(&modules)
                .iter()
                .any(|name| name.starts_with("module-cw")),
        "the table splits, the wrapped modules stay, got {:?}",
        filenames(&modules)
    );
    let entry = module(&modules, "entry.js");
    assert!(entry.contains(".cw("), "got:\n{entry}");
}

/// A wrapped ES module exporting through `n.d`, an accessor of table module
/// 10 it reads, and the entry code that requires it.
const TABLE_STARTUP: &str = "var a=()=>n(10),o=n.cw(function(e,t){n.r(t),\
n.d(t,{label:()=>s});function s(e){return(0,a().tag)(\"widget:\"+e)}});\
const i=o();globalThis.sink={run:()=>i.label(\"x\")};";

#[test]
fn table_wrapped_body_becomes_a_table_module() {
    let modules = raw_modules(&table_bootstrap(TABLE_STARTUP));
    assert_eq!(
        filenames(&modules),
        ["entry.js", "module-10.js", "module-20.js", "module-cw-0.js"]
    );
    let wrapped = module(&modules, "module-cw-0.js");
    assert!(
        wrapped.contains("require.r(exports)")
            && wrapped.contains("require.d(exports")
            && wrapped.contains("require(\"./module-10.js\").tag")
            && !wrapped.contains("a()"),
        "the body is a factory of the table and the accessor call a require, got:\n{wrapped}"
    );
    assert!(
        wrapped.trim_start().starts_with("\"use strict\""),
        "got:\n{wrapped}"
    );
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("const i = require(\"./module-cw-0.js\")")
            && !entry.contains(".cw")
            && !entry.contains("n(10)"),
        "got:\n{entry}"
    );
}

#[test]
fn table_wrapped_esm_body_with_array_define_becomes_esm() {
    // webpack 5.108+ defines a harmony export with a fixed value through the
    // array form of `require.d`.
    // shape: producer webpack@5.112.2 mode production, concatenateModules
    let source = table_bootstrap(
        "var o=n.cw(function(e,t){n.r(t),n.d(t,[\"A\",0,\"circle\",\"b\",0,()=>1])});\
const i=o();globalThis.sink={run:()=>[i.A,i.b()]};",
    );
    let output = unpack(
        &source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    let wrapped = module(&output.modules, "module-cw-0.js");
    assert!(
        wrapped.contains("export") && !wrapped.contains("require.d"),
        "the harmony exports become ESM exports, got:\n{wrapped}"
    );
    let entry = module(&output.modules, "entry.js");
    assert!(
        entry.contains("./module-cw-0.js") && !entry.contains(".cw"),
        "got:\n{entry}"
    );
}

#[test]
fn table_wrapper_reading_another_wrapper_requires_it() {
    let modules = raw_modules(&table_bootstrap(
        "var o=n.cw(function(e,t){t.label=e=>\"widget:\"+e}),\
s=n.cw(function(e,t){t.run=()=>(0,o().label)(\"x\")});\
globalThis.sink={run:()=>s().run()};",
    ));
    assert_eq!(
        filenames(&modules),
        [
            "entry.js",
            "module-10.js",
            "module-20.js",
            "module-cw-0.js",
            "module-cw-1.js"
        ]
    );
    let second = module(&modules, "module-cw-1.js");
    assert!(
        second.contains("require(\"./module-cw-0.js\").label"),
        "got:\n{second}"
    );
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("require(\"./module-cw-1.js\").run()"),
        "got:\n{entry}"
    );
}

#[test]
fn table_wrapper_called_where_it_is_built_splits() {
    // Terser inlines a one-use accessor into its call, here inside the
    // startup IIFE.
    let modules = raw_modules(&table_bootstrap(
        "(()=>{\"use strict\";n.cw(function(e,t){globalThis.polyfilled||(globalThis.polyfilled=!0)})();\
var i=n(10);globalThis.sink={run:()=>(0,i.tag)(\"x\")}})();",
    ));
    assert!(
        filenames(&modules).contains(&"module-cw-0.js"),
        "got {:?}",
        filenames(&modules)
    );
    let wrapped = module(&modules, "module-cw-0.js");
    assert!(wrapped.contains("globalThis.polyfilled"), "got:\n{wrapped}");
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("require(\"./module-cw-0.js\")") && !entry.contains(".cw"),
        "got:\n{entry}"
    );
}

#[test]
fn table_wrappers_in_a_startup_iife_before_a_library_assignment_split() {
    // `output.library` with an assignment type writes the exports after the
    // startup IIFE: `let x = {}; (() => {…})(); Lib = x;`.
    // shape: hypothetical (the startup of the table bootstrap above moved
    // into an IIFE followed by an `output.library` assignment)
    let modules = raw_modules(&table_bootstrap(
        "let x={};(()=>{\"use strict\";var a=()=>n(10),o=n.cw(function(e,t){t.label=function(e){return(0,a().tag)(e)}});\
globalThis.sink={run:()=>o().label(\"x\")}})();globalThis.Lib=x;",
    ));
    assert!(
        filenames(&modules).contains(&"module-cw-0.js"),
        "got {:?}",
        filenames(&modules)
    );
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("require(\"./module-cw-0.js\")")
            && !entry.contains(".cw")
            && entry.contains("globalThis.Lib"),
        "the wrapper leaves the IIFE and the assignment after it stays, got:\n{entry}"
    );
}

#[test]
fn table_namespace_object_a_body_reads_becomes_a_module() {
    // webpack declares a namespace object for `import * as ns` of a wrapped
    // module beside the accessors; its getters call the accessor.
    // shape: producer webpack@5.112.2 mode production, concatenateModules
    let modules = raw_modules(&table_bootstrap(
        "var o=n.cw(function(e,t){n.r(t),n.d(t,{circle:()=>s});const s=\"circle\"}),c={};\
n.r(c),n.d(c,{circle:()=>o().circle});\
var l=n.cw(function(e,t){n.d(t,{kinds:()=>s});function s(){return Object.keys(c)}});\
const i=l();globalThis.sink={run:()=>i.kinds()};",
    ));
    let namespace = module(&modules, "module-cw-namespace-0.js");
    assert!(
        namespace.contains("require.r(exports)")
            && namespace.contains("require(\"./module-cw-0.js\").circle"),
        "the namespace object is a module of its own, got:\n{namespace}"
    );
    let reader = module(&modules, "module-cw-1.js");
    assert!(
        reader.contains("Object.keys(require(\"./module-cw-namespace-0.js\"))"),
        "got:\n{reader}"
    );
}

#[test]
fn table_loaded_binding_a_body_reads_becomes_a_require() {
    let modules = raw_modules(&table_bootstrap(
        "var o=n.cw(function(e,t){t.run=()=>(0,q.tag)(\"x\")});\
var q=n(10);globalThis.sink={run:()=>o().run()};",
    ));
    let wrapped = module(&modules, "module-cw-0.js");
    assert!(
        wrapped.contains("require(\"./module-10.js\").tag") && !wrapped.contains("q."),
        "got:\n{wrapped}"
    );
}

#[test]
fn table_loaded_binding_of_a_module_that_replaces_its_exports_later_stays() {
    // Module 20 replaces `module.exports` after it loads: the binding keeps
    // the first object, a fresh load would return the second.
    assert_wrappers_stay(&table_bootstrap(
        "var o=n.cw(function(e,t){t.run=()=>q.v});\
var q=n(20);globalThis.sink={run:()=>o().run()};",
    ));
}

#[test]
fn table_accessor_used_as_a_value_keeps_the_wrappers() {
    assert_wrappers_stay(&table_bootstrap(&format!(
        "{TABLE_STARTUP}globalThis.keep=o;"
    )));
}

#[test]
fn table_body_reading_another_bootstrap_binding_keeps_the_wrappers() {
    assert_wrappers_stay(&table_bootstrap(
        "var k=[1],o=n.cw(function(e,t){t.run=()=>k[0]});globalThis.sink={run:()=>o().run()};",
    ));
}

#[test]
fn table_body_reading_arguments_keeps_the_wrappers() {
    assert_wrappers_stay(&table_bootstrap(
        "var o=n.cw(function(e,t){t.count=arguments.length});globalThis.sink={run:()=>o().count};",
    ));
}

#[test]
fn table_arrow_body_reading_this_keeps_the_wrappers() {
    assert_wrappers_stay(&table_bootstrap(
        "var o=n.cw((e,t)=>{t.self=this});globalThis.sink={run:()=>o().self};",
    ));
}

#[test]
fn table_accessor_called_before_it_is_declared_keeps_the_wrappers() {
    // `o()` runs before `s` is defined: the original throws there.
    assert_wrappers_stay(&table_bootstrap(
        "var o=n.cw(function(e,t){t.run=()=>s().v});globalThis.early=o().run;\
var s=n.cw(function(e,t){t.v=1});globalThis.sink={run:()=>o().run()};",
    ));
}

#[test]
fn table_module_reaching_entry_code_keeps_the_wrappers() {
    // Module 30 can call the entry function, which calls an accessor, from
    // wherever it loads.
    assert_wrappers_stay(
        &table_bootstrap(&format!(
            "{TABLE_STARTUP}function g(){{return o().label(\"y\")}}"
        ))
        .replacen("20(e,t){", "30(e,t){t.f=()=>g()},20(e,t){", 1),
    );
}

#[test]
fn table_replaced_runtime_member_keeps_the_wrappers() {
    assert_wrappers_stay(&table_bootstrap(&format!("{TABLE_STARTUP}n.cw=e=>e;")));
}

#[test]
fn table_written_require_function_keeps_the_wrappers() {
    assert_wrappers_stay(&table_bootstrap(&format!(
        "{TABLE_STARTUP}globalThis.reset=()=>{{n=null}};"
    )));
}

#[test]
fn table_import_stub_becomes_a_binding() {
    let modules = raw_modules(&table_bootstrap(
        "var o=n.cw(function(e,t){t.v=1});o();globalThis.sink={run:()=>o().v};",
    ));
    let entry = module(&modules, "entry.js");
    assert!(
        entry.contains("var o = require(\"./module-cw-0.js\")")
            && entry.contains("o.v")
            && !entry.contains("o()"),
        "got:\n{entry}"
    );
}
