use std::fs;

use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, BundleFormat, DecompileOptions};

#[test]
fn browserify_accepts_a_nonliteral_cache_argument() {
    let source = r#"
var sharedCache = {};
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = "entry";
  }, {}]
}, sharedCache, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify variable-cache bundle should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert!(output.modules.iter().any(|(name, _)| name == "entry.js"));
}

#[test]
fn browserify_accepts_a_nonliteral_dependency_map() {
    let source = r#"
var entryDependencies = { "./value": 2 };
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("./value");
  }, entryDependencies],
  2: [function(require, module) {
    module.exports = "value";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify dynamic dependency-map bundle should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    // The map's declaration is code around the prelude call, so entry.js
    // holds it and the entry module takes its id.
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry-1.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");
    assert!(
        entry.contains(r#"require("./value")"#),
        "an unproven dynamic map must preserve the original request:\n{entry}"
    );
}

#[test]
fn browserify_accepts_numeric_dependency_request_keys() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("2048");
  }, { 2048: 2 }],
  2: [function(require, module) {
    module.exports = "value";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify numeric-request bundle should unpack");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");
    assert!(entry.contains("require(\"./module-2.js\")"), "{entry}");
}

#[test]
fn browserify_uses_unambiguous_dependency_requests_as_filenames() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("./lib/utility");
  }, { "./lib/utility": 2 }],
  2: [function(require, module) {
    module.exports = "utility";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify request-path bundle should unpack");
    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["entry.js", "lib/utility.js"]);
    let entry = &output.modules[0].1;
    assert!(
        entry.contains(r#"require("./lib/utility.js")"#),
        "dependency rewrite must consume the readable filename:\n{entry}"
    );
}

#[test]
fn browserify_preserves_case_insensitive_javascript_extensions() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = require("./UTILITY.JS");
  }, { "./UTILITY.JS": 2 }],
  2: [function(require, module) {
    module.exports = "utility";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify uppercase-extension bundle should unpack");
    assert!(
        output.modules.iter().any(|(name, _)| name == "UTILITY.JS"),
        "an existing JavaScript extension must not be duplicated"
    );
    let entry = &output.modules[0].1;
    assert!(
        entry.contains(r#"require("./UTILITY.JS")"#),
        "dependency rewrite must preserve the hinted extension:\n{entry}"
    );
}

#[test]
fn browserify_falls_back_when_requests_disagree_on_a_module_name() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = [require("./first"), require("./second")];
  }, { "./first": 2, "./second": 2 }],
  2: [function(require, module) {
    module.exports = "shared";
  }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify alias bundle should unpack");
    assert!(
        output.modules.iter().any(|(name, _)| name == "module-2.js"),
        "ambiguous aliases must retain the stable numeric fallback"
    );
    let entry = &output.modules[0].1;
    assert_eq!(entry.matches(r#"require("./module-2.js")"#).count(), 2);
}

#[test]
fn browserify_reserves_entries_and_deduplicates_hint_paths_case_insensitively() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(require, module) {
    module.exports = [require("./entry"), require("./Utility"), require("./utility")];
  }, { "./entry": 2, "./Utility": 3, "./utility": 4 }],
  2: [function(require, module) { module.exports = "named like entry"; }, {}],
  3: [function(require, module) { module.exports = "upper"; }, {}],
  4: [function(require, module) { module.exports = "lower"; }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify colliding-name bundle should unpack");
    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["entry.js", "entry-2.js", "Utility.js", "utility-2.js"]
    );
}

#[test]
fn browserify_keeps_code_around_the_prelude_call_in_entry() {
    // Statements before and after the prelude call run before and after the
    // bundle; entry.js keeps them around the entries' requires, in the
    // prelude's entry order. shape: hypothetical (a script concatenated
    // with a Browserify bundle).
    let source = r#"
function setup() { globalThis.before = 1; }
var config = setup();
(function() { return function() {}; })()({
  1: [function(require, module) { globalThis.first = 1; }, {}],
  2: [function(require, module) { globalThis.second = 2; }, {}]
}, {}, [2, 1]);
console.log("after", config);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify bundle with surrounding code should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should hold the surrounding code");
    let positions = [
        "var config = setup()",
        r#"require("./entry-2.js")"#,
        r#"require("./entry-1.js")"#,
        r#"console.log("after", config)"#,
    ]
    .map(|needle| {
        entry
            .find(needle)
            .unwrap_or_else(|| panic!("entry.js must contain `{needle}`:\n{entry}"))
    });
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "source and entry order must be kept:\n{entry}"
    );
    assert!(
        output
            .provenance
            .iter()
            .all(|module| module.is_entry == (module.filename == "entry.js")),
        "entry.js runs the entries, so only it is the entry: {:?}",
        output.provenance
    );
}

#[test]
fn browserify_directive_around_the_prelude_call_writes_no_extra_entry() {
    // shape: hypothetical (a directive prologue before the prelude call).
    let source = r#"
"use strict";
(function() { return function() {}; })()({
  1: [function(require, module) { module.exports = "entry"; }, {}]
}, {}, [1]);
"#;

    let output =
        unpack_raw(source, &DecompileOptions::default()).expect("Browserify bundle should unpack");
    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["entry.js"]);
}

#[test]
fn browserify_module_reading_a_declaration_around_the_prelude_call_stays_unsplit() {
    // Module 1 calls `decode`, a global the code before the prelude call
    // declares; split, it would be module-scoped in entry.js. A module's own
    // `decode` parameter in module 2 is not that global. shape: hypothetical
    // (a string-array decoder as javascript-obfuscator declares one).
    let source = r#"
var strings = ["value"];
function decode(i) { return strings[i]; }
(function() { return function() {}; })()({
  1: [function(require, module) { module.exports = decode(0); }, {}],
  2: [function(require, module, exports, decode) { module.exports = decode; }, {}]
}, {}, [1]);
"#;

    let output = unpack(source, DecompileOptions::default()).expect("unpack should succeed");
    assert!(
        !output.detected_formats.contains(&BundleFormat::Browserify),
        "a module reading a declaration around the prelude call must keep the input whole, got {:?}",
        output
            .modules
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn browserify_module_with_a_parameter_named_like_a_declaration_around_splits() {
    // shape: hypothetical.
    let source = r#"
function decode(i) { return "banner" + i; }
(function() { return function() {}; })()({
  1: [function(require, module, exports, decode) { module.exports = decode; }, {}]
}, {}, [1]);
"#;

    let output = unpack(source, DecompileOptions::default()).expect("unpack should succeed");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert!(output.modules.iter().any(|(name, _)| name == "entry.js"));
}

#[test]
fn browserify_code_around_declaring_require_stays_unsplit() {
    // entry.js calls the module's `require`; a top-level `require` declared
    // around the prelude call would capture it. shape: hypothetical.
    let source = r#"
var require = function () {};
(function() { return function() {}; })()({
  1: [function(require, module) { module.exports = require("./value"); }, { "./value": 2 }],
  2: [function(require, module) { module.exports = "value"; }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default()).expect("unpack should succeed");
    assert!(
        !output.detected_formats.contains(&BundleFormat::Browserify),
        "a declared `require` around the prelude call must keep the input whole, got {:?}",
        output
            .modules
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn browserify_deconflicts_factory_param_rename_capture() {
    let source = r#"
(function() { return function() {}; })()({
  1: [function(r, m, e) {
    function invoke(require) {
      return [require, r("./dependency")];
    }
    m.exports = invoke;
  }, { "./dependency": 2 }],
  2: [function(r, m, e) { m.exports = "dependency"; }, {}]
}, {}, [1]);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("Browserify capture should be repaired and unpacked");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");
    assert!(
        entry.contains("function invoke(_require)")
            && entry.contains(r#"require("./dependency.js")"#),
        "the nested binding must be renamed before the runtime loader:\n{entry}"
    );
}

/// browser-pack 6.1's prelude after Terser inlined its wrapper: the prelude
/// function is the callee. producer `browserify@17.0.1` + `terser@5.51.2`
/// (default options).
const TERSER_INLINED_PRELUDE: &str = r#"!function r(e,n,i){function o(f,u){if(!n[f]){if(!e[f]){var c="function"==typeof require&&require;if(!u&&c)return c(f,!0);if(t)return t(f,!0);var s=new Error("Cannot find module '"+f+"'");throw s.code="MODULE_NOT_FOUND",s}var a=n[f]={exports:{}};e[f][0].call(a.exports,function(r){return o(e[f][1][r]||r)},a,a.exports,r,e,n,i)}return n[f].exports}for(var t="function"==typeof require&&require,f=0;f<i.length;f++)o(i[f]);return o}"#;

const INLINED_PRELUDE_TABLE: &str = r#"({1:[function(r,e,n){globalThis.sink=r("./app.js")},{"./app.js":2}],2:[function(r,e,n){var i=r("./provider.js");n.describe=i.describe,n.name="entry-name"},{"./provider.js":3}],3:[function(r,e,n){n.describe=function(r){return"provider:"+r}},{}]},{},[1]);"#;

fn module_names(output: &[(String, String)]) -> Vec<&str> {
    output.iter().map(|(name, _)| name.as_str()).collect()
}

#[test]
fn browserify_detects_a_terser_inlined_prelude() {
    let source = format!("{TERSER_INLINED_PRELUDE}{INLINED_PRELUDE_TABLE}");

    let output = unpack_raw(&source, &DecompileOptions::default())
        .expect("an inlined Browserify prelude should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert_eq!(
        module_names(&output.modules),
        ["entry.js", "app.js", "provider.js"]
    );
    let app = &output.modules[1].1;
    assert!(
        app.contains(r#"require("./provider.js")"#) && app.contains("exports.describe"),
        "the factory's parameters take their runtime names:\n{app}"
    );
}

#[test]
fn browserify_inlined_prelude_recovers_esm_through_the_pipeline() {
    let source = format!("{TERSER_INLINED_PRELUDE}{INLINED_PRELUDE_TABLE}");

    let output = unpack(&source, DecompileOptions::default()).expect("unpack should succeed");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    let app = output
        .modules
        .iter()
        .find(|(name, _)| name == "app.js")
        .map(|(_, code)| code)
        .expect("app.js should exist");
    assert!(
        app.contains(r#"from "./provider.js""#) && app.contains("export const name"),
        "the module's CommonJS should become ESM:\n{app}"
    );
}

#[test]
fn browserify_inlined_prelude_keeps_code_around_it_in_entry() {
    // shape: hypothetical (a script concatenated before the minified bundle).
    let source = format!(
        "var banner = \"before\";\n{TERSER_INLINED_PRELUDE}{INLINED_PRELUDE_TABLE}\nconsole.log(banner);"
    );

    let output = unpack_raw(&source, &DecompileOptions::default())
        .expect("an inlined Browserify prelude should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should hold the surrounding code");
    let positions = [
        r#"var banner = "before""#,
        r#"require("./entry-1.js")"#,
        "console.log(banner)",
    ]
    .map(|needle| {
        entry
            .find(needle)
            .unwrap_or_else(|| panic!("entry.js must contain `{needle}`:\n{entry}"))
    });
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "source order must be kept:\n{entry}"
    );
}

#[test]
fn browserify_detects_an_inlined_prelude_merged_into_a_sequence() {
    // shape: hypothetical (a minifier merging a banner statement with the
    // prelude call; the sequence split exposes the call).
    let prelude = TERSER_INLINED_PRELUDE.trim_start_matches('!');
    let source = format!("globalThis.banner=1,{prelude}{INLINED_PRELUDE_TABLE}");

    let output = unpack_raw(&source, &DecompileOptions::default())
        .expect("an inlined Browserify prelude should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry.js should hold the banner");
    assert!(
        entry.contains("globalThis.banner = 1") && entry.contains(r#"require("./entry-1.js")"#),
        "{entry}"
    );
}

#[test]
fn browserify_detects_the_direct_call_prelude_before_browser_pack_6_0_4() {
    // browser-pack 0.10 to 6.0.3 write the prelude as a function expression
    // that the table call invokes directly. Prelude text: browser-pack@6.0.0
    // `_prelude.js`.
    let source = r#"(function e(t,n,r){function s(o,u){if(!n[o]){if(!t[o]){var a=typeof require=="function"&&require;if(!u&&a)return a(o,!0);if(i)return i(o,!0);var f=new Error("Cannot find module '"+o+"'");throw f.code="MODULE_NOT_FOUND",f}var l=n[o]={exports:{}};t[o][0].call(l.exports,function(e){var n=t[o][1][e];return s(n?n:e)},l,l.exports,e,t,n,r)}return n[o].exports}var i=typeof require=="function"&&require;for(var o=0;o<r.length;o++)s(r[o]);return s})({1:[function(require,module,exports){
module.exports = require("./value");
},{"./value":2}],2:[function(require,module,exports){
module.exports = "value";
},{}]},{},[1]);
"#;

    let output =
        unpack_raw(source, &DecompileOptions::default()).expect("the bundle should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert_eq!(module_names(&output.modules), ["entry.js", "value.js"]);
}

#[test]
fn browserify_detects_the_prelude_that_calls_the_factory_without_call() {
    // browser-pack 0.6 and 0.7 call `modules[name][0](…)` without `.call`.
    // Prelude text: browser-pack@0.7.0 `prelude.js` minified by its pinned
    // uglify-js@1.3.4, as its `index.js` does before emitting it.
    let source = r#"(function(e,t,n){function i(n,s){if(!t[n]){if(!e[n]){var o=typeof require=="function"&&require;if(!s&&o)return o(n,!0);if(r)return r(n,!0);throw new Error("Cannot find module '"+n+"'")}var u=t[n]={exports:{}};e[n][0](function(t){var r=e[n][1][t];return i(r?r:t)},u,u.exports)}return t[n].exports}var r=typeof require=="function"&&require;for(var s=0;s<n.length;s++)i(n[s]);return i})({1:[function(require,module,exports){
module.exports = require("./value");
},{"./value":2}],2:[function(require,module,exports){
module.exports = "value";
},{}]},{},[1]);
"#;

    let output =
        unpack_raw(source, &DecompileOptions::default()).expect("the bundle should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Browserify]);
    assert_eq!(module_names(&output.modules), ["entry.js", "value.js"]);
}

/// Unpacks `source` and asserts that it stays one file.
fn assert_not_browserify(source: &str, why: &str) {
    let output = unpack_raw(source, &DecompileOptions::default()).expect("unpack should succeed");
    assert!(
        !output.detected_formats.contains(&BundleFormat::Browserify),
        "{why}, got {:?}",
        module_names(&output.modules)
    );
}

#[test]
fn browserify_rejects_a_direct_call_whose_body_is_not_the_prelude() {
    // shape: hypothetical.
    let source = format!(
        "!function r(e,n,i){{for(var f=0;f<i.length;f++)globalThis.loaded=e[i[f]];return n}}{INLINED_PRELUDE_TABLE}"
    );
    assert_not_browserify(
        &source,
        "a three-parameter function that does not run the table must keep the input whole",
    );
}

#[test]
fn browserify_rejects_a_prelude_require_without_a_missing_module_test() {
    // shape: hypothetical (the require runs `modules[name]` unchecked).
    let source = format!(
        "!function r(e,n,i){{function o(f){{if(!n[f]){{var a=n[f]={{exports:{{}}}};e[f][0].call(a.exports,o,a,a.exports)}}return n[f].exports}}for(var f=0;f<i.length;f++)o(i[f]);return o}}{INLINED_PRELUDE_TABLE}"
    );
    assert_not_browserify(
        &source,
        "a require without browser-pack's missing-module branch must keep the input whole",
    );
}

#[test]
fn browserify_rejects_an_inlined_prelude_with_a_nonliteral_table() {
    // shape: hypothetical.
    let source = format!(
        "var table = {{1:[function(r,e,n){{e.exports=1}},{{}}]}};\n{TERSER_INLINED_PRELUDE}(table,{{}},[1]);"
    );
    assert_not_browserify(
        &source,
        "a table that is not a literal must keep the input whole",
    );
}

#[test]
fn browserify_rejects_an_inlined_prelude_called_without_an_entry_list() {
    // shape: hypothetical.
    let source =
        format!("{TERSER_INLINED_PRELUDE}({{1:[function(r,e,n){{e.exports=1}},{{}}]}},{{}});");
    assert_not_browserify(
        &source,
        "a call without an entry list must keep the input whole",
    );
}

#[test]
fn webpack5_deconflicts_factory_param_rename_capture() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    1: ((m, e, r) => {
      function invoke(require) {
        return [require, r(2)];
      }
      m.exports = invoke;
    }),
    2: ((m) => { m.exports = "dependency"; })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(id) {
    var m = __webpack_module_cache__[id] = { exports: {} };
    __webpack_modules__[id](m, m.exports, __webpack_require__);
    return m.exports;
  }
  __webpack_require__(1);
})();
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("webpack5 capture should be repaired and unpacked");
    let module = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .map(|(_, code)| code)
        .expect("webpack5 module should exist");
    assert!(
        module.contains("function invoke(_require)")
            && module.contains(r#"require("./module-2.js")"#),
        "the nested binding must be renamed before the runtime loader:\n{module}"
    );
}

#[test]
fn webpack5_unpack_extracts_multiple_modules() {
    let source_path = "../../testcases/webpack5/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read webpack5 testcase");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    assert!(
        pairs.len() > 1,
        "expected webpack5 unpack to split modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "expected webpack5 unpack to include entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}

#[test]
fn webpack5_require_n_default_interop_is_recovered() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./src/cjs.js": ((module) => {
      module.exports = function greet(name) {
        return "Hello, " + name;
      };
    }),
    "./src/index.js": ((__unused_webpack_module, __webpack_exports__, __webpack_require__) => {
      __webpack_require__.r(__webpack_exports__);
      var _cjs__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__("./src/cjs.js");
      var _cjs__WEBPACK_IMPORTED_MODULE_0___default = __webpack_require__.n(_cjs__WEBPACK_IMPORTED_MODULE_0__);
      console.log(_cjs__WEBPACK_IMPORTED_MODULE_0___default()("Ada"));
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/index.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-require-n.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.js")
        .map(|(_, code)| code)
        .expect("expected index module");

    assert!(
        index.contains("import ") && index.contains(r#""./cjs.js""#),
        "expected recovered import in webpack5 require.n module:\n{index}"
    );
    assert!(
        !index.contains("require.n") && !index.contains("__esModule"),
        "webpack5 require.n helper should not survive:\n{index}"
    );
}

/// A provider that stays CommonJS keeps its `__esModule` marker, also one
/// defined only on one branch: a consumer's interop check reads it.
#[test]
fn webpack5_bundled_tslib_interop_helpers_become_imports() {
    // shape: producer tsc@5.9.3 module=CommonJS esModuleInterop importHelpers,
    // bundled with tslib@2.8.1 by webpack@5.111.1 mode=production (Terser);
    // sink renamed to `globalThis.lib`. Webpack resolves tslib's `module`
    // condition, so the helpers come from `tslib.es6.mjs`, and Terser joins
    // each provider's `__esModule` flag into a sequence.
    let source = r#"(()=>{var e={393(e,t,r){"use strict";t.run=function(){return["consumer-side",new o.default("w").label(),(0,u.default)("v"),u.default.version,i.twice(3),i.default,Object.keys(i).sort().join(",")]};const n=r(635),o=n.__importDefault(r(805)),u=n.__importDefault(r(30)),i=n.__importStar(r(701))},97(e,t,r){const n=r(393);t.main=function(){return n.run()}},30(e){function t(e){return"<"+String(e)+">"}t.version="format-1",e.exports=t},701(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),t.twice=function(e){return 2*e},t.default="util-default"},805(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),t.default=class{constructor(e){this.name=e}label(){return"widget:"+this.name}}},635(e,t,r){"use strict";r.d(t,{__importDefault:()=>a,__importStar:()=>i});var n=Object.create?function(e,t,r,n){void 0===n&&(n=r);var o=Object.getOwnPropertyDescriptor(t,r);o&&!("get"in o?!t.__esModule:o.writable||o.configurable)||(o={enumerable:!0,get:function(){return t[r]}}),Object.defineProperty(e,n,o)}:function(e,t,r,n){void 0===n&&(n=r),e[n]=t[r]},o=Object.create?function(e,t){Object.defineProperty(e,"default",{enumerable:!0,value:t})}:function(e,t){e.default=t},u=function(e){return u=Object.getOwnPropertyNames||function(e){var t=[];for(var r in e)Object.prototype.hasOwnProperty.call(e,r)&&(t[t.length]=r);return t},u(e)};function i(e){if(e&&e.__esModule)return e;var t={};if(null!=e)for(var r=u(e),i=0;i<r.length;i++)"default"!==r[i]&&n(t,e,r[i]);return o(t,e),t}function a(e){return e&&e.__esModule?e:{default:e}}}};const t={};function r(n){const o=t[n];if(void 0!==o)return o.exports;const u=t[n]={exports:{}};return e[n](u,u.exports,r),u.exports}r.d=(e,t)=>{for(var n in t)r.o(t,n)&&!r.o(e,n)&&Object.defineProperty(e,n,{enumerable:!0,get:t[n]})},r.o=(e,t)=>Object.prototype.hasOwnProperty.call(e,t),globalThis.lib=r(97)})();"#;

    // A provider marked `__esModule` keeps its default export for
    // `__importDefault` and its namespace for `__importStar`; the unmarked
    // `module.exports = format` is the whole value either way.
    assert_interop_consumer_imports(
        source,
        &[
            r#"import o from "./module-805.js";"#,
            r#"import u from "./module-30.js";"#,
            r#"import * as i from "./module-701.js";"#,
            "new o(\"w\")",
            "u.version",
            "i.default",
        ],
        &["__importDefault", "__importStar", "o.default", "u.default"],
    );
}

#[test]
fn webpack5_bundled_babel_runtime_interop_helper_becomes_imports() {
    // shape: producer @babel/core@7.28.5 with plugin-transform-runtime@7.28.5
    // and plugin-transform-modules-commonjs@7.28.6, bundled with
    // @babel/runtime@7.28.6 by webpack@5.111.1 mode=production (Terser); sink
    // renamed to `globalThis.lib`. Terser leaves the runtime's
    // `interopRequireDefault` an anonymous `module.exports = function (e) {…}`;
    // transform-runtime inlines `interopRequireWildcard`.
    let source = r#"(()=>{var e={393(e,t,r){"use strict";var n=r(994);t.run=function(){return["consumer-side",new o.default("w").label(),(0,u.default)("v"),u.default.version,i.twice(3),i.default,Object.keys(i).sort().join(",")]};var o=n(r(805)),u=n(r(30)),i=function(e){if("function"==typeof WeakMap){var t=new WeakMap;new WeakMap}return function(e){if(e&&e.__esModule)return e;var r,n,o={__proto__:null,default:e};if(null===e||"object"!=typeof e&&"function"!=typeof e)return o;if(r=t){if(r.has(e))return r.get(e);r.set(e,o)}for(const t in e)"default"!==t&&{}.hasOwnProperty.call(e,t)&&((n=(r=Object.defineProperty)&&Object.getOwnPropertyDescriptor(e,t))&&(n.get||n.set)?r(o,t,n):o[t]=e[t]);return o}(e)}(r(701))},97(e,t,r){const n=r(393);t.main=function(){return n.run()}},30(e){function t(e){return"<"+String(e)+">"}t.version="format-1",e.exports=t},701(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),t.default=void 0,t.twice=function(e){return 2*e},t.default="util-default"},805(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),t.default=void 0,t.default=class{constructor(e){this.name=e}label(){return"widget:"+this.name}}},994(e){e.exports=function(e){return e&&e.__esModule?e:{default:e}},e.exports.__esModule=!0,e.exports.default=e.exports}};const t={};globalThis.lib=function r(n){const o=t[n];if(void 0!==o)return o.exports;const u=t[n]={exports:{}};return e[n](u,u.exports,r),u.exports}(97)})();"#;
    assert_interop_consumer_imports(
        source,
        &[
            r#"import o from "./module-805.js";"#,
            r#"import u from "./module-30.js";"#,
            r#"import * as i from "./module-701.js";"#,
            "new o(\"w\")",
            "u.version",
        ],
        &["o.default", "u.default"],
    );
}

#[test]
fn webpack5_bundled_babel_runtime_interop_of_a_commonjs_provider_becomes_a_default_import() {
    // The babel runtime bundle above, with a `module.exports = format`
    // provider that requires a module inside a function, so it stays
    // CommonJS. The interop's `.default` is its `module.exports`, which is the
    // default import of a CommonJS module.
    let source = r#"(()=>{var e={393(e,t,r){"use strict";var n=r(994);t.run=function(){return["consumer-side",new o.default("w").label(),(0,u.default)("v"),u.default.version,i.twice(3),i.default,Object.keys(i).sort().join(",")]};var o=n(r(805)),u=n(r(30)),i=function(e){if("function"==typeof WeakMap){var t=new WeakMap;new WeakMap}return function(e){if(e&&e.__esModule)return e;var r,n,o={__proto__:null,default:e};if(null===e||"object"!=typeof e&&"function"!=typeof e)return o;if(r=t){if(r.has(e))return r.get(e);r.set(e,o)}for(const t in e)"default"!==t&&{}.hasOwnProperty.call(e,t)&&((n=(r=Object.defineProperty)&&Object.getOwnPropertyDescriptor(e,t))&&(n.get||n.set)?r(o,t,n):o[t]=e[t]);return o}(e)}(r(701))},97(e,t,r){const n=r(393);t.main=function(){return n.run()}},30(e,t,r){function n(e){return"<"+String(e)+">"}n.version="format-1",n.lazy=function(){return r(805)},e.exports=n},701(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),t.default=void 0,t.twice=function(e){return 2*e},t.default="util-default"},805(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),t.default=void 0,t.default=class{constructor(e){this.name=e}label(){return"widget:"+this.name}}},994(e){e.exports=function(e){return e&&e.__esModule?e:{default:e}},e.exports.__esModule=!0,e.exports.default=e.exports}};const t={};globalThis.lib=function r(n){const o=t[n];if(void 0!==o)return o.exports;const u=t[n]={exports:{}};return e[n](u,u.exports,r),u.exports}(97)})();"#;
    let output = unpack(source, DecompileOptions::default()).expect("bundle should unpack");
    // Node runs the syntaxless provider as CommonJS, and so does the
    // validator.
    let findings = validate_output_modules(&output.modules);
    assert!(findings.is_empty(), "{findings:#?}");
    let module = |filename: &str| {
        output
            .modules
            .iter()
            .find(|(name, _)| name == filename)
            .map(|(_, code)| code.clone())
            .expect("module should exist")
    };
    let provider = module("module-30.js");
    assert!(provider.contains("module.exports = n"), "{provider}");
    assert!(!provider.contains("export "), "{provider}");
    let consumer = module("module-393.js");
    for present in [
        r#"import u from "./module-30.js";"#,
        r#"u("v")"#,
        "u.version",
    ] {
        assert!(
            consumer.contains(present),
            "missing `{present}`:\n{consumer}"
        );
    }
    for absent in ["require(", "u.default"] {
        assert!(!consumer.contains(absent), "kept `{absent}`:\n{consumer}");
    }
}

#[test]
fn webpack5_bundled_swc_external_interop_helpers_become_imports() {
    // shape: producer @swc/core@1.16.2 module=commonjs externalHelpers, bundled
    // with @swc/helpers@0.5.18 by webpack@5.111.1 mode=production (Terser);
    // sink renamed to `globalThis.lib`. Each helper module exports `_`.
    let source = r#"(()=>{var e={393(e,t,r){"use strict";Object.defineProperty(t,"run",{enumerable:!0,get:function(){return f}});const n=r(677),o=r(544),u=n._(r(805)),i=n._(r(30)),c=o._(r(701));function f(){return["consumer-side",new u.default("w").label(),(0,i.default)("v"),i.default.version,c.twice(3),c.default,Object.keys(c).sort().join(",")]}},97(e,t,r){const n=r(393);t.main=function(){return n.run()}},30(e){function t(e){return"<"+String(e)+">"}t.version="format-1",e.exports=t},701(e,t){"use strict";function r(e){return 2*e}Object.defineProperty(t,"__esModule",{value:!0}),function(e,t){for(var r in t)Object.defineProperty(e,r,{enumerable:!0,get:Object.getOwnPropertyDescriptor(t,r).get})}(t,{get default(){return n},get twice(){return r}});const n="util-default"},805(e,t){"use strict";Object.defineProperty(t,"__esModule",{value:!0}),Object.defineProperty(t,"default",{enumerable:!0,get:function(){return r}});class r{label(){return"widget:"+this.name}constructor(e){this.name=e}}},677(e,t,r){"use strict";function n(e){return e&&e.__esModule?e:{default:e}}r.d(t,{_:()=>n})},544(e,t,r){"use strict";function n(e){if("function"!=typeof WeakMap)return null;var t=new WeakMap,r=new WeakMap;return(n=function(e){return e?r:t})(e)}function o(e,t){if(!t&&e&&e.__esModule)return e;if(null===e||"object"!=typeof e&&"function"!=typeof e)return{default:e};var r=n(t);if(r&&r.has(e))return r.get(e);var o={__proto__:null},u=Object.defineProperty&&Object.getOwnPropertyDescriptor;for(var i in e)if("default"!==i&&Object.prototype.hasOwnProperty.call(e,i)){var c=u?Object.getOwnPropertyDescriptor(e,i):null;c&&(c.get||c.set)?Object.defineProperty(o,i,c):o[i]=e[i]}return o.default=e,r&&r.set(e,o),o}r.d(t,{_:()=>o})}};const t={};function r(n){const o=t[n];if(void 0!==o)return o.exports;const u=t[n]={exports:{}};return e[n](u,u.exports,r),u.exports}r.d=(e,t)=>{for(var n in t)r.o(t,n)&&!r.o(e,n)&&Object.defineProperty(e,n,{enumerable:!0,get:t[n]})},r.o=(e,t)=>Object.prototype.hasOwnProperty.call(e,t),globalThis.lib=r(97)})();"#;
    assert_interop_consumer_imports(
        source,
        &[
            r#"import u from "./module-805.js";"#,
            r#"import i from "./module-30.js";"#,
            r#"import * as c from "./module-701.js";"#,
            "new u(\"w\")",
            "i.version",
        ],
        &["._(", "u.default", "i.default"],
    );
}

/// Unpacks `source` and checks the consumer `module-393.js`: no require() is
/// left, and it contains each of `present` and none of `absent`.
fn assert_interop_consumer_imports(source: &str, present: &[&str], absent: &[&str]) {
    let output =
        unpack(source, DecompileOptions::default()).expect("webpack5 bundle should unpack");
    let findings = validate_output_modules(&output.modules);
    assert!(
        findings.is_empty(),
        "unexpected graph findings: {findings:#?}"
    );
    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-393.js")
        .map(|(_, code)| code)
        .expect("consumer module should exist");
    for absent in std::iter::once(&"require(").chain(absent) {
        assert!(
            !consumer.contains(absent),
            "consumer must not keep `{absent}`:\n{consumer}"
        );
    }
    for present in present {
        assert!(
            consumer.contains(present),
            "consumer must contain `{present}`:\n{consumer}"
        );
    }
}

#[test]
fn webpack_commonjs_provider_keeps_a_conditional_esmodule_marker() {
    let source = r#"
(() => {
  var modules = {
    100: function(module, exports) {
      var api = { a: 1 };
      exports.default = api;
      exports.api = api;
      "undefined" == typeof window || window !== exports ? Object.defineProperty(exports, "__esModule", { value: !0 }) : delete exports.default;
    },
    200: function(module, exports, load) {
      var p = load(100);
      console.log(p.__esModule, p.default === p.api, p.api.a);
    }
  };
  var cache = {};
  function load(id) {
    var cached = cache[id];
    if (cached !== undefined) return cached.exports;
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(200);
})();
"#;

    let output =
        unpack(source, DecompileOptions::default()).expect("webpack unpack should succeed");
    let provider = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-100.js")
        .map(|(_, code)| code)
        .expect("expected provider module");
    assert!(
        provider.contains("exports.default = api"),
        "the provider stays CommonJS:\n{provider}"
    );
    assert!(
        provider.contains(r#"Object.defineProperty(exports, "__esModule""#),
        "the provider keeps its marker:\n{provider}"
    );
}

#[test]
fn webpack5_default_object_composition_shell_stays_commonjs() {
    // `module.exports = {}` filled by ordered
    // `Object.assign(module.exports, require(id) || {})` copies
    // (wild-observed). The sources become ES modules; the shell keeps its
    // requires, which read each provider's default export.
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    0: ((module, exports, __webpack_require__) => {
      module.exports = {};
      Object.assign(module.exports, __webpack_require__(1) || {});
      Object.assign(module.exports, __webpack_require__(2) || {});
    }),
    1: ((module) => {
      module.exports = { alpha: "alpha-token", shared: "first-token" };
    }),
    2: ((module) => {
      module.exports = { beta: "beta-token", shared: "second-token" };
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-composition.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the composition shell should unpack");
        assert!(
            !output.has_errors(),
            "unexpected warnings: {:?}",
            output.warnings
        );
        let code = |filename: &str| {
            output
                .modules
                .iter()
                .find(|(name, _)| name == filename)
                .map(|(_, code)| code.as_str())
                .unwrap_or_else(|| panic!("expected {filename}"))
        };
        let shell = code("module-0.js");
        assert!(
            shell.contains("module.exports = {};")
                && shell.contains(
                    r#"Object.assign(module.exports, require("./module-1.js").default || {});"#
                )
                && shell.contains(
                    r#"Object.assign(module.exports, require("./module-2.js").default || {});"#
                ),
            "{shell}"
        );
        assert!(
            !shell.contains("import ") && !shell.contains("export "),
            "{shell}"
        );
        for provider in ["module-1.js", "module-2.js"] {
            assert!(
                code(provider).contains("export default {"),
                "{}",
                code(provider)
            );
        }
        assert_eq!(validate_output_modules(&output.modules), vec![]);
    }
}

#[test]
fn webpack4_string_module_ids_use_relative_output_imports() {
    let source = r#"
!function(__webpack_modules__) {
  function __webpack_require__(moduleId) {
    var module = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  return __webpack_require__("./src/index.js");
}({
  "./src/value.js": function(module) {
    module.exports = "ok";
  },
  "./src/index.js": function(module, exports, __webpack_require__) {
    var value = __webpack_require__("./src/value.js");
    module.exports = value;
  }
});
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-string-ids.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.js")
        .map(|(_, code)| code)
        .expect("expected index module");

    assert!(
        index.contains("import ") && index.contains(r#""./value.js""#),
        "expected import relative to src/index.js:\n{index}"
    );
    assert!(
        !index.contains(r#""./src/value.js""#),
        "import must not be relative to the bundle root:\n{index}"
    );
}

#[test]
fn webpack4_non_javascript_module_ids_emit_javascript_filenames() {
    let source = r#"
!function(__webpack_modules__) {
  function __webpack_require__(moduleId) {
    var module = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  return __webpack_require__("./src/index.ts");
}({
  "./src/style/index.less": function(module) {
    module.exports = "compiled style";
  },
  "./src/index.ts": function(module, exports, __webpack_require__) {
    module.exports = __webpack_require__("./src/style/index.less");
  }
});
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-non-js-id.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 non-JavaScript module id should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.ts")
        .map(|(_, code)| code)
        .expect("the JavaScript-like TypeScript path should be retained");
    assert!(
        index.contains(r#""./style/index.less.js""#),
        "the consumer must use the derived JavaScript filename:\n{index}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, _)| name == "src/style/index.less.js"),
        "expected loader-produced JavaScript to append .js"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_global_var_injection_exposes_named_exports() {
    let source = r#"
!function(modules) {
  function require(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, require);
    return module.exports;
  }
  return require("./src/entry.js");
}({
  "./node_modules/webpack/buildin/global.js": function(module) {
    module.exports = globalThis;
  },
  "./src/global-user.js": function(module, exports, __webpack_require__) {
    (function(injectedGlobal) {
      exports.getGlobal = function() {
        return injectedGlobal;
      };
    }).call(this, __webpack_require__("./node_modules/webpack/buildin/global.js"));
  },
  "./src/entry.js": function(module, exports, __webpack_require__) {
    module.exports = __webpack_require__("./src/global-user.js").getGlobal();
  }
});
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-global-injection.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 global injection should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let provider = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/global-user.js")
        .map(|(_, code)| code)
        .expect("expected injected-global provider");
    assert!(
        provider.contains("export") && provider.contains("getGlobal"),
        "the wrapper must expose its CommonJS assignment to UnEsm:\n{provider}"
    );
    assert!(
        !provider.contains(".call(this") && !provider.contains("require("),
        "the webpack wrapper and nested require must be recovered:\n{provider}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_injected_global_fallback_exposes_default_export() {
    let source = r#"
!function(modules) {
  function require(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, require);
    return module.exports;
  }
  return require(0);
}([
  function(module, exports, require) {
    module.exports = require(1);
  },
  function(module, exports, require) {
    (function(injectedGlobal) {
      function selectGlobal(candidate) {
        return candidate && candidate.Math === Math && candidate;
      }
      module.exports = selectGlobal("object" === typeof globalThis && globalThis)
        || selectGlobal("object" === typeof self && self)
        || selectGlobal("object" === typeof window && window)
        || selectGlobal("object" === typeof injectedGlobal && injectedGlobal)
        || Function("return this")();
    }).call(this, require(2));
  },
  function(module) {
    globalThis.injectionLoads = (globalThis.injectionLoads || 0) + 1;
    module.exports = globalThis;
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-global-fallback.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 global fallback should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let provider = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .map(|(_, code)| code)
        .expect("expected global detector module");
    assert!(
        provider.contains("export default"),
        "the wrapper must expose module.exports to UnEsm:\n{provider}"
    );
    assert!(
        provider.contains(r#"import "./module-2.js""#),
        "the unidentified provider must remain as an eager side-effect import:\n{provider}"
    );
    assert!(
        !provider.contains("injectedGlobal"),
        "the obsolete injected fallback binding must be removed:\n{provider}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, code)| name == "module-2.js" && code.contains("injectionLoads")),
        "the side-effectful injected provider must be emitted"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_reused_loader_parameter_becomes_a_local_after_module_loads() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(0);
}([
  function(module, exports, load) {
    var dependency = load(1);
    load = /compiled/;
    module.exports = [dependency, load.test("compiled")];
  },
  function(module) {
    module.exports = "dependency";
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-reused-loader.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 reused loader parameter should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);

    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("/compiled/"))
        .map(|(_, code)| code)
        .expect("expected webpack4 module that reuses the loader");
    assert!(
        entry.contains("import "),
        "expected recovered import:\n{entry}"
    );
    assert!(
        !entry.contains("require"),
        "the reused factory parameter must not become a free require binding:\n{entry}"
    );
    assert!(
        entry.contains("const _load = /compiled/;") && entry.contains("_load.test"),
        "the post-load value must be declared and read through the local:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_reused_exports_parameter_preserves_the_runtime_export_lifetime() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(0);
}([
  function(module, publicValue, load) {
    publicValue.ready = true;
    publicValue = load(1);
    consume(publicValue.value);
  },
  function(module) {
    module.exports = { value: "dependency" };
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-reused-exports.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 exports parameter reuse should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("consume"))
        .map(|(_, code)| code)
        .expect("expected recovered exports-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(
        !entry.contains("exports ="),
        "the parameter's second lifetime must be a declared local:\n{entry}"
    );
    assert!(entry.contains("import _publicValue from"), "{entry}");
    assert!(entry.contains("consume(_publicValue.value)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_localized_runtime_parameter_avoids_a_free_global_reference() {
    // Same alias chain as the Ajv test, but the module also references a
    // global spelled like the local the unpacker would invent (`_publicValue`).
    // Introducing `let _publicValue` would capture that reference after
    // printing, so the local must take the next free spelling.
    let source = r#"
(() => {
  var modules = ({
    0: ((context, publicValue) => {
      Object.defineProperty(publicValue, "__esModule", { value: true });
      publicValue.SchemaEnv = void 0;
      class Ajv {}
      _publicValue.track("ajv");
      (publicValue.Ajv = Ajv,
       context.exports = publicValue = Ajv,
       context.exports.Ajv = Ajv,
       Object.defineProperty(publicValue, "__esModule", { value: true }),
       publicValue.default = Ajv);
      globalThis.observed = publicValue.Ajv === Ajv;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-ajv-exports-alias-free-global.js".to_string(),
            ..Default::default()
        },
    )
    .expect("the alias chain should still unpack");

    let module = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered module");
    assert!(
        module.contains("_publicValue.track(\"ajv\")"),
        "the free global reference must survive untouched:\n{module}"
    );
    assert!(
        module.contains("let _publicValue_2;") && module.contains("_publicValue_2 = Ajv"),
        "the invented local must not take the spelling of a free global:\n{module}"
    );
    assert!(
        !module.contains("let _publicValue;"),
        "no local may shadow the free `_publicValue` reference:\n{module}"
    );
}

#[test]
fn webpack5_reused_exports_parameter_in_commonjs_alias_chain_is_localized() {
    // Ajv 8.20.0 authors this compatibility bridge before TypeScript and
    // webpack lower/minify it: `module.exports = exports = Ajv`. The exports
    // factory parameter has a CommonJS-object lifetime before the chain and a
    // callable-local lifetime after it.
    let source = r#"
(() => {
  var modules = ({
    0: ((context, publicValue) => {
      Object.defineProperty(publicValue, "__esModule", { value: true });
      publicValue.SchemaEnv = void 0;
      class Ajv {}
      (publicValue.Ajv = Ajv,
       context.exports = publicValue = Ajv,
       context.exports.Ajv = Ajv,
       Object.defineProperty(publicValue, "__esModule", { value: true }),
       publicValue.default = Ajv);
      globalThis.observed = publicValue.Ajv === Ajv;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-ajv-exports-alias.js".to_string(),
            ..Default::default()
        },
    )
    .expect("the Ajv-style exports alias chain should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let module = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered Ajv-style module");
    assert!(module.contains("class Ajv"), "{module}");
    assert!(
        module.contains("let _publicValue;")
            && module.contains("_publicValue = Ajv")
            && !module.contains("\npublicValue")
            && !module.contains("exports ="),
        "the factory parameter's callable lifetime must be a local binding:\n{module}"
    );
    assert!(
        module.contains(".Ajv ="),
        "the callable property must survive:\n{module}"
    );
    assert!(module.contains("export default Ajv"), "{module}");
    assert!(module.contains("export { Ajv"), "{module}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_unproven_commonjs_alias_default_keeps_the_exports_writes() {
    // An alias bridge whose default the lifetime split cannot prove keeps
    // the bridge on the module's own `exports` binding, so ESM recovery
    // cannot erase a property observable through it.
    for (initializer, tail) in [
        ("(effect(), Engine)", ""),
        ("Engine", "Engine = replacement;"),
        ("Engine", "context.exports = replacement;"),
        ("Engine", "expose(context);"),
        ("Engine", "eval(source);"),
        ("Engine", "eval(0);"),
        (
            "Engine",
            "function read() { return context.exports; } read();",
        ),
    ] {
        let source = format!(
            r#"
(() => {{
  var modules = ({{0: ((context, publicValue) => {{
    Object.defineProperty(publicValue, "__esModule", {{value: true}});
    class Engine {{}}
    context.exports = publicValue = {initializer};
    context.exports.Engine = Engine;
    globalThis.observed = publicValue.Engine === Engine;
    {tail}
  }})}});
  var cache = {{}};
  (function load(id) {{
    var module = cache[id] = {{exports: {{}}}};
    modules[id](module, module.exports, load);
    return module.exports;
  }})(0);
}})();
"#
        );
        let output = unpack(&source, DecompileOptions::default()).expect("unpack the factory");
        assert_eq!(
            output.detected_formats,
            [BundleFormat::Webpack5],
            "{initializer}; {tail}"
        );
        let module = assert_commonjs_exports_provider(&output, "module-0.js");
        assert!(
            module.contains("exports.Engine === Engine"),
            "{initializer}; {tail}:\n{module}"
        );
    }
}

#[test]
fn webpack5_reused_exports_and_loader_parameters_split_in_evaluation_order() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, publicValue, load) => {
      publicValue.ready = true;
      const read = (
        publicValue = load(1),
        load = load(2),
        () => [publicValue.value, load.value]
      );
      module.exports = read;
    }),
    1: ((module) => {
      module.exports = { value: "dependency" };
    }),
    2: ((module) => {
      module.exports = { value: "runtime" };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-interleaved-runtime-parameters.js".to_string(),
            ..Default::default()
        },
    )
    .expect("interleaved exports and loader lifetimes should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered interleaved module");
    let dependency = entry.find("./module-1.js").expect("first dependency");
    let runtime = entry.find("./module-2.js").expect("second dependency");
    assert!(dependency < runtime, "dependency order changed:\n{entry}");
    assert!(!entry.contains("exports ="), "{entry}");
    assert!(entry.contains("import _publicValue from"), "{entry}");
    assert!(entry.contains("import _load from"), "{entry}");
    assert!(entry.contains("_publicValue.value"), "{entry}");
    assert!(entry.contains("_load.value"), "{entry}");
    assert!(entry.contains("export default read"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let mapped = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-interleaved-runtime-parameters.js".to_string(),
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("source-map materialization should preserve both localized lifetimes");
    let mapped_entry = mapped
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected mapped interleaved module");
    assert!(
        mapped_entry.contains("_publicValue.value"),
        "{mapped_entry}"
    );
    assert!(mapped_entry.contains("_load.value"), "{mapped_entry}");
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-0.js"));
    assert_eq!(validate_output_modules(&mapped.modules), vec![]);
}

#[test]
fn webpack5_reused_module_parameter_preserves_the_runtime_module_lifetime() {
    let source = r#"
(() => {
  var modules = ({
    0: ((context, exports, load) => {
      context.exports.ready = true;
      context = load(1);
      consume(context.value);
    }),
    1: ((module) => {
      module.exports = { value: "dependency" };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-reused-module.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 module parameter reuse should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered module-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(
        !entry.contains("module ="),
        "the parameter's second lifetime must be a declared local:\n{entry}"
    );
    assert!(entry.contains("import _context from"), "{entry}");
    assert!(entry.contains("consume(_context.value)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_exports_in_for_in_rhs_preserves_the_consumed_value() {
    let source = r#"
(() => {
  var modules = ({
    0: ((context, publicValue, load) => {
      var additions = load(1);
      function api(value) { return value; }
      for (var key in (((publicValue = context.exports = api).own = true), additions)) {
        publicValue[key] = additions[key];
      }
      consume(publicValue);
    }),
    1: ((module) => {
      module.exports = { extra: "value" };
    }),
    2: ((module, exports, load) => {
      var own = load(0).own;
      module.exports = own;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(2);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-for-in-exports-alias.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a consumed exports alias reset in a for-in RHS should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered for-in alias module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("export default _publicValue"), "{entry}");
    assert!(entry.contains("_publicValue.own = true"), "{entry}");
    assert!(entry.contains("_publicValue[key]"), "{entry}");
    assert!(!entry.contains("exports ="), "{entry}");
    assert!(entry.contains("consume(_publicValue)"), "{entry}");
    let consumer = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-2.js")
        .map(|(_, code)| code)
        .expect("expected consumer of the attached callable property");
    assert!(!consumer.contains("import { own"), "{consumer}");
    assert!(consumer.contains(".own"), "{consumer}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_hoisted_function_capture_keeps_runtime_parameter_reuse_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      observeLoader();
      load = load(1);
      function observeLoader() {
        consume(load(2));
      }
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "runtime";
    }),
    2: ((module) => {
      module.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-hoisted-runtime-capture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a hoisted capture should isolate only its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque hoisted-capture factory");
    assert!(opaque.contains("function observeLoader"), "{opaque}");
    assert!(opaque.contains("load(2)"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_conditional_exports_write_keeps_the_write_on_exports() {
    // shape: hypothetical (a conditional first write has no provable
    // lifetime boundary)
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(2);
}([
  function(module, publicValue) {
    if (globalThis.replaceExports) publicValue = globalThis.replacement;
    module.exports = publicValue;
  },
  function(module) {
    module.exports = "stable";
  },
  function(module, exports, load) {
    module.exports = [load(0), load(1)];
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-conditional-exports-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a conditional exports write should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(
        provider.contains("exports = globalThis.replacement;")
            && provider.contains("module.exports = exports;"),
        "{provider}"
    );
    let entry = module_code(&output, "entry.js");
    assert!(entry.contains("./module-0.js"), "{entry}");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(!entry.contains("require("), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

/// The emitted code of `filename`.
fn module_code(output: &wakaru_core::driver::test_support::UnpackOutput, filename: &str) -> String {
    output
        .modules
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| code.clone())
        .unwrap_or_else(|| panic!("missing {filename}: {:?}", output.modules))
}

/// A factory whose `exports` parameter is written is recovered, not isolated,
/// and stays a CommonJS module that keeps writing its own `exports` binding.
fn assert_commonjs_exports_provider(
    output: &wakaru_core::driver::test_support::UnpackOutput,
    filename: &str,
) -> String {
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let code = module_code(output, filename);
    assert!(
        code.lines()
            .all(|line| !line.starts_with("import ") && !line.starts_with("export ")),
        "{filename} must stay CommonJS:\n{code}"
    );
    assert!(code.contains("exports ="), "{filename}:\n{code}");
    code
}

/// A webpack 5 object-table bundle around `factories` that starts at `entry`.
fn webpack5_bundle(factories: &str, entry: usize) -> String {
    format!(
        r#"
(() => {{
  var modules = ({{
{factories}
  }});
  var cache = {{}};
  function load(id) {{
    var cached = cache[id];
    if (cached !== undefined) return cached.exports;
    var module = cache[id] = {{ exports: {{}} }};
    modules[id](module, module.exports, load);
    return module.exports;
  }}
  load({entry});
}})();
"#
    )
}

fn unpack_webpack5(
    factories: &str,
    entry: usize,
) -> wakaru_core::driver::test_support::UnpackOutput {
    let output = unpack(
        &webpack5_bundle(factories, entry),
        DecompileOptions {
            filename: "webpack5-exports-writes.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    output
}

#[test]
fn webpack5_exports_bridge_in_a_library_iife_keeps_the_exports_writes() {
    // shape: producer webpack@5.109.0 mode=production (the guarded bridge
    // inside a library IIFE, as in wp5-exports-bridge-min)
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      !function () {
        function Lib(name) { this.name = name; }
        Lib.prototype.first = function () { return this.name; };
        e.exports && (t = e.exports = Lib), t.Lib = Lib;
      }();
    }),
    1: ((e, t, n) => {
      var Lib = n(0);
      console.log(new Lib("alpha").first());
    }),
"#,
        1,
    );
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(provider.contains("module.exports = Lib"), "{provider}");
    assert!(provider.contains("exports.Lib = Lib"), "{provider}");
    assert!(!provider.contains("t.Lib"), "{provider}");
    let consumer = module_code(&output, "module-1.js");
    assert!(
        consumer.contains("from \"./module-0.js\""),
        "the consumer must link to the recovered provider:\n{consumer}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_reversed_exports_bridge_with_a_later_member_write_keeps_the_exports_writes() {
    // shape: producer webpack@4.47.0 mode=production (a top-level
    // `module.exports = exports = <call or literal>` bridge, as in
    // wp4-exports-bridge-min)
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(1);
}([
  function(e, t, n) {
    function createLib() { return { first: 1 }; }
    e.exports = t = createLib();
    t.second = 2;
  },
  function(e, t, n) {
    var lib = n(0);
    console.log(lib.first, lib.second);
  }
]);
"#;
    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-reversed-exports-bridge.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack4 unpack should succeed");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(provider.contains("createLib()"), "{provider}");
    assert!(provider.contains("exports.second = 2"), "{provider}");
    let consumer = module_code(&output, "entry.js");
    assert!(consumer.contains("from \"./module-0.js\""), "{consumer}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_exports_bridge_in_an_if_test_keeps_the_exports_writes() {
    // shape: wild-observed (the bridge as the first element of a comma `if`
    // test); producer webpack@5.109.0 mode=production emits the member-object
    // form `if ((t = e.exports = ...).alpha)` (wp5-exports-bridge-min)
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      var A = n(2);
      if (t = e.exports = A.locals || {}, t.alpha) t.beta = 2;
    }),
    1: ((e, t, n) => {
      console.log(n(0).beta);
    }),
    2: ((e) => {
      e.exports = { locals: { alpha: true } };
    }),
"#,
        1,
    );
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(provider.contains("module.exports = "), "{provider}");
    assert!(provider.contains("exports.alpha"), "{provider}");
    assert!(provider.contains("exports.beta = 2"), "{provider}");
    assert!(provider.contains("./module-2.js"), "{provider}");
    assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_conditional_exports_bridge_as_a_member_object_keeps_the_exports_writes() {
    // shape: hypothetical (the member-object bridge under a guard; the
    // unconditional top-level form is localized instead)
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      globalThis.flag && ((t = e.exports = n(2)).extra = 3);
      t.other = 4;
    }),
    1: ((e, t, n) => {
      console.log(n(0).other);
    }),
    2: ((e) => {
      e.exports = { base: true };
    }),
"#,
        1,
    );
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(provider.contains(".extra = 3"), "{provider}");
    assert!(provider.contains("exports.other = 4"), "{provider}");
    assert!(provider.contains("./module-2.js"), "{provider}");
    assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_hoisted_function_capture_of_a_written_exports_keeps_the_exports_writes() {
    // shape: hypothetical (a hoisted declaration reads the parameter before
    // the bridge runs, so no lifetime split is possible)
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      function read() { return t.first; }
      t = e.exports = { first: 1 };
      t.read = read;
    }),
    1: ((e, t, n) => {
      console.log(n(0).read());
    }),
"#,
        1,
    );
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(provider.contains("return exports.first"), "{provider}");
    assert!(provider.contains("exports.read = read"), "{provider}");
    assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_var_redeclared_exports_parameter_lowers_to_exports_writes() {
    // shape: hypothetical (`var` redeclarations of the parameter that the
    // lifetime split cannot lift)
    for (body, expected) in [
        (
            "t.first = 1; if (globalThis.flag) { var t = e.exports = { second: 2 }; } t.third = 3;",
            &["exports.first = 1", "exports = module.exports", "exports.third = 3"][..],
        ),
        (
            "function read() { return t; } var other = 1, t = e.exports = { first: other }, last = 2; t.read = read; t.last = last;",
            &["return exports;", "exports = module.exports", "exports.read = read", "exports.last = last"][..],
        ),
    ] {
        let output = unpack_webpack5(
            &format!(
                r#"
    0: ((e, t, n) => {{ {body} }}),
    1: ((e, t, n) => {{ console.log(n(0)); }}),
"#
            ),
            1,
        );
        let provider = assert_commonjs_exports_provider(&output, "module-0.js");
        for snippet in expected {
            assert!(provider.contains(snippet), "{body}: missing {snippet}:\n{provider}");
        }
        assert!(
            !provider.contains("var ") && !provider.contains(" t =") && !provider.contains("t."),
            "{body}: no local may stand in for the parameter:\n{provider}"
        );
        assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
        assert_eq!(validate_output_modules(&output.modules), vec![]);
    }
}

#[test]
fn webpack5_bare_var_redeclaration_still_names_the_exports_parameter() {
    // shape: hypothetical. `var t;` does not change the parameter's value, so
    // `t.first` is an export write, not a property write on an undefined local.
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      var t;
      t.first = 1;
    }),
    1: ((e, t, n) => {
      console.log(n(0).first);
    }),
"#,
        1,
    );
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "{:?}",
        output.warnings
    );
    let provider = module_code(&output, "module-0.js");
    assert!(provider.contains("first = 1"), "{provider}");
    assert!(
        !provider.contains("var t") && !provider.contains("let t") && !provider.contains("t.first"),
        "{provider}"
    );
    assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_tsc_enum_through_a_merged_exports_parameter_keeps_the_exports_writes() {
    // shape: hypothetical (UglifyJS `merge_vars` reuses the dead exports
    // parameter as the enum temp of tsc's CommonJS output)
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      Object.defineProperty(t, "__esModule", { value: !0 });
      t.Kind = void 0;
      (function (e) { e[e.alpha = 0] = "alpha"; e[e.beta = 1] = "beta"; })(t = t.Kind || (t.Kind = {}));
    }),
    1: ((e, t, n) => {
      console.log(n(0).Kind.alpha);
    }),
"#,
        1,
    );
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(
        provider.contains("exports = exports.Kind || (exports.Kind = {})"),
        "{provider}"
    );
    assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_exports_writes_stay_opaque_where_keeping_them_is_unproven() {
    // shape: hypothetical. Only the `exports` parameter has a CommonJS
    // counterpart; a loader write, a written `exports` that webpack's runtime
    // helpers also define getters on, and a redeclaration in a `for` head
    // still isolate their factory.
    for (factory, parameter) in [
        ("if (globalThis.flag) n = n(2); e.exports = n;", "require"),
        (
            "n.r(t); n.d(t, { alpha: () => r }); var r = 1; if (globalThis.flag) (t = t.Meta || (t.Meta = {})).x = 1;",
            "exports",
        ),
        ("t.first = 1; for (var t in globalThis.source) console.log(t);", "exports"),
    ] {
        let output = unpack_webpack5(
            &format!(
                r#"
    0: ((e, t, n) => {{ {factory} }}),
    1: ((e, t, n) => {{ console.log(n(0), n(2)); }}),
    2: ((e) => {{ e.exports = "stable"; }}),
"#
            ),
            1,
        );
        let failures = output
            .warnings
            .iter()
            .filter(|warning| {
                warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
            })
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1, "{factory}: {:?}", output.warnings);
        assert_eq!(failures[0].filename, "module-0.js", "{factory}");
        // The warning names the parameter whose reuse kept the factory opaque.
        assert!(
            failures[0]
                .message
                .contains(&format!("runtime-parameter reuse of its `{parameter}` parameter")),
            "{factory}: {}",
            failures[0].message
        );
        let consumer = module_code(&output, "module-1.js");
        assert!(consumer.contains("require(0)"), "{factory}: {consumer}");
        assert!(consumer.contains("./module-2.js"), "{factory}: {consumer}");
    }
}

#[test]
fn webpack5_written_exports_beside_a_free_exports_reference_is_a_free_name_capture() {
    // shape: hypothetical. Renaming the written parameter to `exports` would
    // capture the free `exports` probe.
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => {
      if (globalThis.flag) t = {};
      e.exports = [t, typeof exports];
    }),
    1: ((e, t, n) => {
      console.log(n(0));
    }),
"#,
        1,
    );
    assert_eq!(free_name_capture_failures(&output), ["module-0.js"]);
}

#[test]
fn webpack5_exports_writes_match_a_factory_that_names_its_parameter_exports() {
    // The fallback keeps the writes exactly as a factory whose parameter is
    // already spelled `exports` (the shape webpack emits without a minifier).
    let minified = r#"
    0: ((e, t, n) => {
      !function () {
        function Lib() {}
        t = e.exports = Lib;
        t.Lib = Lib;
      }();
      if (globalThis.flag) t.extra = n(2);
    }),
    1: ((e, t, n) => {
      if (globalThis.flag) t = e.exports = { first: 1 };
      t.second = 2;
    }),
    2: ((e, t, n) => {
      console.log(n(0), n(1));
    }),
"#;
    let named = minified
        .replace(
            "(e, t, n) => {\n      !function",
            "(e, exports, n) => {\n      !function",
        )
        .replace(
            "t = e.exports = Lib;\n        t.Lib",
            "exports = e.exports = Lib;\n        exports.Lib",
        )
        .replace("t.extra", "exports.extra")
        .replace(
            "(e, t, n) => {\n      if (globalThis.flag) t = e.exports",
            "(e, exports, n) => {\n      if (globalThis.flag) exports = e.exports",
        )
        .replace("t.second", "exports.second");
    assert_eq!(named.matches("exports, n").count(), 2, "{named}");
    assert!(!named.contains("t.") && !named.contains("t ="), "{named}");

    let minified_output = unpack_webpack5(minified, 2);
    let named_output = unpack_webpack5(&named, 2);
    assert_commonjs_exports_provider(&minified_output, "module-0.js");
    assert_commonjs_exports_provider(&minified_output, "module-1.js");
    assert_eq!(minified_output.modules, named_output.modules);
    assert_eq!(minified_output.warnings, named_output.warnings);
}

#[test]
fn aggressive_nested_scope_split_leaves_a_commonjs_module_whole() {
    // shape: producer webpack@5.109.0 mode=production (a library IIFE with
    // two helper families and the guarded exports bridge, as in
    // wp5-exports-bridge-min); the same body unwrapped is hypothetical and
    // reaches the split of the module's own text. Splitting either would
    // emit ES module children beside a parent that still writes
    // `module.exports` and `exports`.
    let body = r#"
        var api = { version: "1", settings: { alpha: 1, beta: 2 } };
        var map = Array.prototype.map;
        function helperA1(x) { return x + api.settings.alpha; }
        function helperA2(x) { return helperA1(x) * 2; }
        function helperA3(x) { return helperA2(x) + 3; }
        function helperB1(x) { return x - api.settings.beta; }
        function helperB2(x) { return helperB1(x) * 4; }
        function helperB3(x) { return map.call([x], helperB2)[0]; }
        var first = api.first = function (x) { return helperA3(x); };
        var second = api.second = function (x) { return helperB3(x); };
        api.both = function (x) { return first(x) + second(x); };
        e.exports && (t = e.exports = api), t.api = api;
"#;
    for factory in [format!("!function () {{ {body} }}();"), body.to_string()] {
        let source = webpack5_bundle(
            &format!(
                r#"
    0: ((e, t) => {{ {factory} }}),
    1: ((e, t, n) => {{
      var api = n(0);
      t.api = api;
    }}),
"#
            ),
            1,
        );
        let output = unpack(
            &source,
            DecompileOptions {
                filename: "webpack5-nested-commonjs.js".to_string(),
                heuristic_split: true,
                level: wakaru_core::RewriteLevel::Aggressive,
                ..Default::default()
            },
        )
        .expect("webpack5 unpack should succeed");
        let names = output
            .modules
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert!(
            names.iter().all(|name| !name.contains('/')),
            "a CommonJS module must not be split into ES module children: {names:?}"
        );
        let provider = assert_commonjs_exports_provider(&output, "module-0.js");
        assert!(provider.contains("function helperA1"), "{provider}");
        assert!(provider.contains("function helperB1"), "{provider}");
        assert!(provider.contains("exports.api = api"), "{provider}");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
    }
}

fn assert_no_factory_recovery_failure(output: &wakaru_core::driver::test_support::UnpackOutput) {
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
}

/// The one `webpack_factory_recovery_failed` warning, on `filename`, naming
/// `parameter` as the reused one.
fn assert_opaque_factory(
    output: &wakaru_core::driver::test_support::UnpackOutput,
    filename: &str,
    parameter: &str,
    case: &str,
) {
    let failures = output
        .warnings
        .iter()
        .filter(|warning| {
            warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        })
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1, "{case}: {:?}", output.warnings);
    assert_eq!(failures[0].filename, filename, "{case}");
    assert!(
        failures[0].message.contains(&format!(
            "runtime-parameter reuse of its `{parameter}` parameter"
        )),
        "{case}: {}",
        failures[0].message
    );
}

/// Two exported tsc enums in an ES module. UglifyJS inlines each enum IIFE
/// and reuses a parameter that is dead after the `require.d` getters (the
/// loader, then `exports`) as the enum temp, so the first write to each is the
/// member object at the start of a sequence element.
const ENUM_TEMP_HARMONY_PROVIDER: &str = r#"function(e,t,n){"use strict";var r,o;function u(e){return e?r.Alpha:o.Delta}n.d(t,"a",function(){return r}),n.d(t,"b",function(){return o}),n.d(t,"c",function(){return u}),(n=r=r||{}).Alpha="alpha",n.Beta="beta",(t=o=o||{}).Gamma="gamma",t.Delta="delta"}"#;
const ENUM_TEMP_HARMONY_CONSUMER: &str =
    r#"function(e,t,n){"use strict";n=n(0);console.log(n.a.Beta,n.b.Gamma,Object(n.c)(!0))}"#;

#[test]
fn webpack4_enum_temps_reusing_dead_runtime_parameters_start_local_lifetimes() {
    // shape: producer typescript@5.9.3 {"module":"ESNext","target":"ES2015"}
    // then webpack@4.47.0 mode=production minimize=false then
    // uglify-js@3.19.3 (defaults); wild-observed
    let source = webpack4_bundle(
        &format!("{ENUM_TEMP_HARMONY_PROVIDER},\n{ENUM_TEMP_HARMONY_CONSUMER}"),
        1,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("webpack4 unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert_no_factory_recovery_failure(&output);
    let provider = module_code(&output, "module-0.js");
    assert!(
        !provider.contains("exports"),
        "the getters define named exports, and the enum writes go to locals:\n{provider}"
    );
    assert!(provider.contains("export let a;"), "{provider}");
    assert!(provider.contains("export function c("), "{provider}");
    assert!(provider.contains("const _n = a = a || {};"), "{provider}");
    assert!(provider.contains("_n.Beta = \"beta\";"), "{provider}");
    assert!(provider.contains("const _t = b = b || {};"), "{provider}");
    assert!(provider.contains("_t.Delta = \"delta\";"), "{provider}");
    let consumer = module_code(&output, "entry.js");
    assert!(
        consumer.contains("import { a, b, c } from \"./module-0.js\";"),
        "{consumer}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    // Before any rule runs, the getters still define on the factory's real
    // exports object, and only the writes after them follow the locals.
    let raw = unpack_raw(&source, &DecompileOptions::default()).expect("webpack4 raw unpack");
    let raw_provider = module_code(&raw, "module-0.js");
    assert!(
        raw_provider.contains("require.d(exports, \"c\""),
        "{raw_provider}"
    );
    assert!(
        raw_provider.contains("var _n = r = r || {};"),
        "{raw_provider}"
    );
    assert!(
        raw_provider.contains("var _t = o = o || {};"),
        "{raw_provider}"
    );
    assert!(!raw_provider.contains("exports ="), "{raw_provider}");

    // Equivalence: the same body written with fresh locals.
    let named = ENUM_TEMP_HARMONY_PROVIDER.replace(
        r#",(n=r=r||{}).Alpha="alpha",n.Beta="beta",(t=o=o||{}).Gamma="gamma",t.Delta="delta""#,
        r#";var _n=r=r||{};_n.Alpha="alpha",_n.Beta="beta";var _t=o=o||{};_t.Gamma="gamma",_t.Delta="delta""#,
    );
    assert_ne!(named, ENUM_TEMP_HARMONY_PROVIDER);
    let named_output = unpack(
        &webpack4_bundle(&format!("{named},\n{ENUM_TEMP_HARMONY_CONSUMER}"), 1),
        DecompileOptions::default(),
    )
    .expect("webpack4 unpack");
    assert_eq!(output.modules, named_output.modules);
    assert_eq!(output.warnings, named_output.warnings);
}

#[test]
fn webpack4_enum_temp_rewritten_after_its_first_write_follows_the_local() {
    // shape: producer typescript@5.9.3 {"module":"ESNext","target":"ES2015"}
    // then webpack@4.47.0 mode=production minimize=false then
    // uglify-js@3.14.5 (defaults): both enums reuse the exports parameter
    let provider = ENUM_TEMP_HARMONY_PROVIDER
        .replace("(n=r=r||{})", "(t=r=r||{})")
        .replace(r#"n.Beta="beta""#, r#"t.Beta="beta""#);
    let output = unpack(
        &webpack4_bundle(&format!("{provider},\n{ENUM_TEMP_HARMONY_CONSUMER}"), 1),
        DecompileOptions::default(),
    )
    .expect("webpack4 unpack");
    assert_no_factory_recovery_failure(&output);
    let provider = module_code(&output, "module-0.js");
    assert!(!provider.contains("exports"), "{provider}");
    assert!(provider.contains("let _t = a = a || {};"), "{provider}");
    assert!(provider.contains("_t = b = b || {};"), "{provider}");
    assert!(provider.contains("_t.Delta = \"delta\";"), "{provider}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_enum_temps_reusing_dead_runtime_parameters_start_local_lifetimes() {
    // shape: producer typescript@5.9.3 {"module":"ESNext","target":"ES2015"}
    // then webpack@5.111.1 mode=production minimize=false then
    // uglify-js@3.19.3 (defaults), with the getter that UglifyJS inlined a
    // function into left out
    let output = unpack_webpack5(
        r#"
    0: ((e, t, a) => { var r, o; a.d(t, { Kt: () => r, kL: () => o }), (a = r = r || {}).Alpha = "alpha", a.Beta = "beta", (t = o = o || {}).Gamma = "gamma", t.Delta = "delta"; }),
    1: ((e, t, a) => { a = a(0); console.log(a.Kt.Beta, a.kL.Gamma); }),
"#,
        1,
    );
    assert_no_factory_recovery_failure(&output);
    let provider = module_code(&output, "module-0.js");
    assert!(!provider.contains("exports"), "{provider}");
    assert!(provider.contains("export let Kt;"), "{provider}");
    assert!(provider.contains("const _a = Kt = Kt || {};"), "{provider}");
    assert!(provider.contains("const _t = kL = kL || {};"), "{provider}");
    assert!(module_code(&output, "module-1.js").contains("./module-0.js"));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_module_parameter_enum_temp_in_an_initializer_sequence_starts_its_local_lifetime() {
    // shape: hypothetical (the producer shape above, with the `module`
    // parameter as the temp and the write in a declaration initializer)
    let source = webpack5_bundle(
        r#"
    0: ((e, t, n) => { var r, s = (e.exports.ready = n(2), (e = r = r || {}).Alpha = "alpha"); e.Beta = s; }),
    1: ((e, t, n) => { console.log(n(0)); }),
    2: ((e) => { e.exports = "stable"; }),
"#,
        1,
    );
    let options = DecompileOptions {
        filename: "webpack5-module-enum-temp.js".to_string(),
        ..Default::default()
    };
    let raw = unpack_raw(&source, &options).expect("webpack5 raw unpack");
    let provider = module_code(&raw, "module-0.js");
    let runtime = provider
        .find("module.exports.ready = require(\"./module-2.js\")")
        .unwrap_or_else(|| panic!("{provider}"));
    let local = provider
        .find("var _e = r = r || {};")
        .unwrap_or_else(|| panic!("{provider}"));
    let suffix = provider
        .find("s = _e.Alpha = \"alpha\"")
        .unwrap_or_else(|| panic!("{provider}"));
    assert!(runtime < local && local < suffix, "{provider}");
    assert!(provider.contains("_e.Beta = s"), "{provider}");

    let output = unpack(&source, options).expect("webpack5 unpack");
    assert_no_factory_recovery_failure(&output);
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_enum_temp_writes_that_cannot_start_a_lifetime_stay_opaque() {
    // shape: hypothetical. Each write sits where the old value is read first
    // (in its own right-hand side, the assignment target's object, an earlier
    // argument or operand) or where it may not run (a conditional). The
    // harmony getters rule out keeping the writes on `exports`.
    let harmony_factory = |write: &str| {
        format!(
            r#"
    0: ((e, t, n) => {{ var r; n.r(t); n.d(t, {{ alpha: () => r }}), observe(), {write}; }}),
    1: ((e, t, n) => {{ console.log(n(0), n(2)); }}),
    2: ((e) => {{ e.exports = "stable"; }}),
"#
        )
    };
    // Control: the same factory with the write first in its element.
    let control = unpack_webpack5(&harmony_factory("(t = r = r || {}).x = 1"), 1);
    assert_no_factory_recovery_failure(&control);
    assert!(module_code(&control, "module-0.js").contains("_t.x = 1"));

    for factory in [
        "(t = r = t.seed || {}).x = 1",
        "t.alpha = (t = r = r || {}).beta",
        "consume(t, (t = r = r || {}).x = 1)",
        "t.alpha + (t = r = r || {}).beta",
        "globalThis.flag && ((t = r = r || {}).x = 1)",
        "globalThis.flag ? (t = r = r || {}).x = 1 : 0",
        "(observe(), t = r = r || {}).x = 1",
    ] {
        let output = unpack_webpack5(&harmony_factory(factory), 1);
        assert_opaque_factory(&output, "module-0.js", "exports", factory);
        let consumer = module_code(&output, "module-1.js");
        assert!(consumer.contains("require(0)"), "{factory}: {consumer}");
        assert!(consumer.contains("./module-2.js"), "{factory}: {consumer}");
    }

    // The loader's old value is read in the same element before the write.
    let output = unpack_webpack5(
        r#"
    0: ((e, t, n) => { var r; n.r(t); n.d(t, { alpha: () => r }), observe(), consume(n(2), (n = r = r || {}).x = 1); }),
    1: ((e, t, n) => { console.log(n(0), n(2)); }),
    2: ((e) => { e.exports = "stable"; }),
"#,
        1,
    );
    assert_opaque_factory(&output, "module-0.js", "require", "loader call argument");
}

/// `modules` without statements that only read `local`.
fn without_bare_reads(modules: &[(String, String)], local: &str) -> Vec<(String, String)> {
    let bare_read = format!("{local};");
    modules
        .iter()
        .map(|(name, code)| {
            let code = code
                .lines()
                .filter(|line| line.trim() != bare_read)
                .collect::<Vec<_>>()
                .join("\n");
            (name.clone(), code)
        })
        .collect()
}

/// A core-js 2 style global and version holder, each a factory that reuses
/// its dead `module` parameter for `var local = module.exports = value`.
const MODULE_EXPORTS_BRIDGE_FACTORIES: &str = r#"function(e,n,t){var r=t(1),t=t(2);n.version=t.version,n.hasMath=r.Math===Math},
function(e,n){e=e.exports="undefined"!=typeof window&&window.Math==Math?window:"undefined"!=typeof self&&self.Math==Math?self:Function("return this")();"number"==typeof __alpha&&(__alpha=e)},
function(e,n){e=e.exports={version:"1.0.0"};"number"==typeof __beta&&(__beta=e)}"#;

#[test]
fn webpack4_module_parameter_rebound_to_its_exports_value_starts_a_local_lifetime() {
    // shape: producer core-js@2.6.12 (modules/_global.js, modules/_core.js)
    // under webpack@4.47.0 mode=production target=node minimize=false then
    // uglify-js@3.19.3 (defaults)
    let source = webpack4_bundle(MODULE_EXPORTS_BRIDGE_FACTORIES, 0);
    let output = unpack(&source, DecompileOptions::default()).expect("webpack4 unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert_no_factory_recovery_failure(&output);
    let global = module_code(&output, "module-1.js");
    assert!(global.contains("export default _e;"), "{global}");
    assert!(global.contains("__alpha = _e;"), "{global}");
    assert!(!global.contains("module"), "{global}");
    let core = module_code(&output, "module-2.js");
    assert!(core.contains("const _e = {"), "{core}");
    assert!(core.contains("export default _e;"), "{core}");
    assert!(core.contains("__beta = _e;"), "{core}");
    let consumer = module_code(&output, "entry.js");
    assert!(consumer.contains("from \"./module-1.js\""), "{consumer}");
    assert!(consumer.contains("from \"./module-2.js\""), "{consumer}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    // Before any rule runs, the old value is the runtime `module`.
    let raw = unpack_raw(&source, &DecompileOptions::default()).expect("webpack4 raw unpack");
    let raw_core = module_code(&raw, "module-2.js");
    assert!(
        raw_core.contains("var _e = module.exports = {"),
        "{raw_core}"
    );

    // Equivalence: the same bodies written with a fresh local.
    let named = MODULE_EXPORTS_BRIDGE_FACTORIES
        .replace("{e=e.exports=", "{var _e=e.exports=")
        .replace("(__alpha=e)", "(__alpha=_e)")
        .replace("(__beta=e)", "(__beta=_e)");
    assert_eq!(named.matches("var _e=e.exports=").count(), 2, "{named}");
    let named_output =
        unpack(&webpack4_bundle(&named, 0), DecompileOptions::default()).expect("webpack4 unpack");
    // The statement-level lift leaves the rebound statement behind as a bare
    // read of the local, which has no effect; the rest is identical.
    assert_eq!(
        without_bare_reads(&output.modules, "_e"),
        without_bare_reads(&named_output.modules, "_e")
    );
    assert_eq!(output.warnings, named_output.warnings);
}

#[test]
fn webpack5_module_parameter_rebound_to_its_exports_value_starts_a_local_lifetime() {
    // shape: producer core-js@2.6.12 (modules/_core.js) under
    // webpack@5.111.1 mode=production target=node minimize=false then
    // uglify-js@3.19.3 (defaults); the `var` form and the function value are
    // wild-observed and hypothetical respectively
    for (factory, expected) in [
        (
            r#"(e => { e = e.exports = { version: "1.0.0" }; "number" == typeof __beta && (__beta = e); })"#,
            "__beta = _e;",
        ),
        (
            r#"(e => { var e = e.exports = { version: "1.0.0" }; e.extra = 1; })"#,
            "_e.extra = 1;",
        ),
        (
            r#"(e => { e = e.exports = function () { return e.version; }; e.version = "1.0.0"; })"#,
            "return _e.version;",
        ),
    ] {
        let output = unpack_webpack5(
            &format!(
                r#"
    0: {factory},
    1: ((e, t, n) => {{ console.log(n(0)); }}),
"#
            ),
            1,
        );
        assert_no_factory_recovery_failure(&output);
        let provider = module_code(&output, "module-0.js");
        assert!(provider.contains(expected), "{factory}:\n{provider}");
        assert!(
            provider.contains("export default _e;"),
            "{factory}:\n{provider}"
        );
        assert!(!provider.contains("module"), "{factory}:\n{provider}");
        assert!(
            module_code(&output, "module-1.js").contains("./module-0.js"),
            "{factory}"
        );
        assert_eq!(
            validate_output_modules(&output.modules),
            vec![],
            "{factory}"
        );
    }
}

#[test]
fn webpack5_module_parameter_rebinds_that_read_its_old_value_stay_opaque() {
    // shape: hypothetical. Only `param = param.exports = value` whose value
    // does not read the parameter is a lifetime boundary.
    for factory in [
        "e = e.exports = { self: e };",
        "e = e.exports = e.id;",
        "e = e.exports = (e.loaded, {});",
        "e = e.other = {};",
        "e = e.exports.inner = {};",
        "e = e.exports += 1;",
        "globalThis.flag && (e = e.exports = {});",
    ] {
        let output = unpack_webpack5(
            &format!(
                r#"
    0: ((e, t, n) => {{ {factory} e.marker = n(2); }}),
    1: ((e, t, n) => {{ console.log(n(0), n(2)); }}),
    2: ((e) => {{ e.exports = "stable"; }}),
"#
            ),
            1,
        );
        assert_opaque_factory(&output, "module-0.js", "module", factory);
        let consumer = module_code(&output, "module-1.js");
        assert!(consumer.contains("require(0)"), "{factory}: {consumer}");
        assert!(consumer.contains("./module-2.js"), "{factory}: {consumer}");
    }
}

/// `var other = exports.other = value` in a CommonJS module, with the local
/// folded into the dead `exports` parameter.
const EXPORTS_MEMBER_BRIDGE_FACTORIES: &str = r#"function(e,t,n){n=n(1);console.log(n.helper(1),n.helper.extra,n.other.count)},
function(e,t){(t.helper=function(e){return e+1}).extra=2;t=t.other={count:1};"number"==typeof globalThis.__probe&&(globalThis.__probe=t)}"#;

#[test]
fn webpack4_exports_parameter_rebound_to_an_export_value_starts_a_local_lifetime() {
    // shape: producer webpack@4.47.0 mode=production target=node
    // minimize=false then uglify-js@3.19.3 (defaults), over
    // `var other = exports.other = { count: 1 }`
    let source = webpack4_bundle(EXPORTS_MEMBER_BRIDGE_FACTORIES, 0);
    let output = unpack(&source, DecompileOptions::default()).expect("webpack4 unpack");
    assert_no_factory_recovery_failure(&output);
    let provider = module_code(&output, "module-1.js");
    assert!(
        !provider.contains("exports"),
        "the second lifetime is a local, not a kept `exports` write:\n{provider}"
    );
    assert!(provider.contains("export const other = {"), "{provider}");
    assert!(
        provider.contains("globalThis.__probe = other;"),
        "{provider}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let raw = unpack_raw(&source, &DecompileOptions::default()).expect("webpack4 raw unpack");
    let raw_provider = module_code(&raw, "module-1.js");
    assert!(
        raw_provider.contains("var _t = exports.other = {"),
        "{raw_provider}"
    );

    // Equivalence: the same body written with a fresh local.
    let named = EXPORTS_MEMBER_BRIDGE_FACTORIES
        .replace(";t=t.other=", ";var _t=t.other=")
        .replace("(globalThis.__probe=t)", "(globalThis.__probe=_t)");
    assert_ne!(named, EXPORTS_MEMBER_BRIDGE_FACTORIES);
    let named_output =
        unpack(&webpack4_bundle(&named, 0), DecompileOptions::default()).expect("webpack4 unpack");
    // The statement-level lift leaves a bare read of the local behind.
    assert_eq!(
        without_bare_reads(&output.modules, "other"),
        without_bare_reads(&named_output.modules, "other")
    );
    assert_eq!(output.warnings, named_output.warnings);
}

#[test]
fn webpack5_exports_rebinds_that_read_the_old_value_keep_the_exports_writes() {
    // shape: hypothetical. A value that reads the parameter, or a chain that
    // is not exactly `param = param.name = value`, is no lifetime boundary;
    // CommonJS keeps the writes on `exports`.
    for (factory, kept) in [
        ("t = t.other = { self: t };", "exports = exports.other = {"),
        (
            "t = t.other = t.seed;",
            "exports = exports.other = exports.seed",
        ),
        ("t = t[\"other\"] = {};", "exports = exports.other = {}"),
        (
            "t = t.other.inner = {};",
            "exports = exports.other.inner = {}",
        ),
    ] {
        let output = unpack_webpack5(
            &format!(
                r#"
    0: ((e, t) => {{ {factory} t.marker = 1; }}),
    1: ((e, t, n) => {{ console.log(n(0)); }}),
"#
            ),
            1,
        );
        let provider = assert_commonjs_exports_provider(&output, "module-0.js");
        assert!(provider.contains(kept), "{factory}:\n{provider}");
    }
}

/// A webpack 4 array bundle around `factories` that starts at `entry`.
fn webpack4_bundle(factories: &str, entry: usize) -> String {
    format!(
        r#"
!function(modules) {{
  var cache = {{}};
  function load(id) {{
    if (cache[id]) return cache[id].exports;
    var module = cache[id] = {{ i: id, l: false, exports: {{}} }};
    modules[id].call(module.exports, module, module.exports, load);
    module.l = true;
    return module.exports;
  }}
  return load({entry});
}}([
{factories}
]);
"#
    )
}

/// `buildin/module.js` and `buildin/harmony-module.js` as webpack 4.47.0
/// ships them, before minification.
const WEBPACK4_MODULE_DECORATOR: &str = r#"
  function(module, exports) {
    module.exports = function(module) {
      if (!module.webpackPolyfill) {
        module.deprecate = function() {};
        module.paths = [];
        // module.parent = undefined by default
        if (!module.children) module.children = [];
        Object.defineProperty(module, "loaded", {
          enumerable: true,
          get: function() {
            return module.l;
          }
        });
        Object.defineProperty(module, "id", {
          enumerable: true,
          get: function() {
            return module.i;
          }
        });
        module.webpackPolyfill = 1;
      }
      return module;
    };
  }"#;
const WEBPACK4_HARMONY_MODULE_DECORATOR: &str = r#"
  function(module, exports) {
    module.exports = function(originalModule) {
      if (!originalModule.webpackPolyfill) {
        var module = Object.create(originalModule);
        // module.parent = undefined by default
        if (!module.children) module.children = [];
        Object.defineProperty(module, "loaded", {
          enumerable: true,
          get: function() {
            return module.l;
          }
        });
        Object.defineProperty(module, "id", {
          enumerable: true,
          get: function() {
            return module.i;
          }
        });
        Object.defineProperty(module, "exports", {
          enumerable: true
        });
        module.webpackPolyfill = 1;
      }
      return module;
    };
  }"#;

#[test]
fn webpack4_unminified_module_decorators_reduce_to_the_module() {
    // shape: hypothetical (webpack 4.47.0's `buildin/module.js` and
    // `buildin/harmony-module.js` source, unminified; the minified
    // production form is covered by wp4-module-decorator-min)
    let factories = format!(
        r#"
  function(module, exports, __webpack_require__) {{
    var lib = __webpack_require__(1), flags = __webpack_require__(2);
    console.log(lib, flags.loaded);
  }},
  function(module, exports, __webpack_require__) {{
    (function(module) {{
      if (module && module.exports) module.exports = {{ first: "first" }};
    }}).call(this, __webpack_require__(3)(module));
  }},
  function(module, __webpack_exports__, __webpack_require__) {{
    "use strict";
    __webpack_require__.r(__webpack_exports__);
    (function(module) {{
      __webpack_require__.d(__webpack_exports__, "loaded", function() {{ return loaded; }});
      var loaded = typeof module.id;
    }}).call(this, __webpack_require__(4)(module));
  }},{WEBPACK4_MODULE_DECORATOR},{WEBPACK4_HARMONY_MODULE_DECORATOR}
"#
    );
    let output = unpack(&webpack4_bundle(&factories, 0), DecompileOptions::default())
        .expect("webpack4 unpack should succeed");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    for (name, code) in &output.modules {
        assert!(!code.contains(".default(module)"), "{name}:\n{code}");
    }
    let commonjs = module_code(&output, "module-1.js");
    assert!(
        commonjs.contains(".call(this, module)") || commonjs.contains("module.exports"),
        "{commonjs}"
    );
    let harmony = module_code(&output, "module-2.js");
    assert!(
        harmony.contains("export") && harmony.contains("module.id") && !harmony.contains("require"),
        "{harmony}"
    );
    assert!(module_code(&output, "module-3.js").contains("webpackPolyfill"));
    assert!(module_code(&output, "module-4.js").contains("Object.create"));
}

#[test]
fn webpack4_module_decorator_lookalikes_are_not_reduced() {
    // shape: hypothetical. A factory that only resembles the generated
    // decorator (one more effect, or a different result) is user code; its
    // call stays as written.
    for decorator in [
        WEBPACK4_MODULE_DECORATOR
            .replace("module.paths = [];", "module.paths = []; module.extra = 1;"),
        WEBPACK4_MODULE_DECORATOR.replace("return module;", "return module.exports;"),
        WEBPACK4_HARMONY_MODULE_DECORATOR.replace(
            "Object.create(originalModule)",
            "Object.assign({}, originalModule)",
        ),
    ] {
        let factories = format!(
            r#"
  function(module, exports, __webpack_require__) {{
    (function(module) {{
      if (module && module.exports) module.exports = {{ first: "first" }};
    }}).call(this, __webpack_require__(1)(module));
  }},{decorator}
"#
        );
        let output = unpack(&webpack4_bundle(&factories, 0), DecompileOptions::default())
            .expect("webpack4 unpack should succeed");
        let consumer = module_code(&output, "entry.js");
        assert!(
            consumer.contains("(module)"),
            "a lookalike decorator call must stay:\n{consumer}"
        );
    }
}

#[test]
fn webpack4_module_wrapper_that_writes_its_parameter_is_not_lifted() {
    // shape: hypothetical. Renaming a written wrapper parameter to the
    // factory's `module` would turn a local write into a module write.
    let factories = format!(
        r#"
  function(module, exports, __webpack_require__) {{
    var value = __webpack_require__(1);
    console.log(value.loaded);
  }},
  function(module, __webpack_exports__, __webpack_require__) {{
    "use strict";
    __webpack_require__.r(__webpack_exports__);
    (function(module) {{
      __webpack_require__.d(__webpack_exports__, "loaded", function() {{ return loaded; }});
      var loaded = typeof module.id;
      module = null;
    }}).call(this, __webpack_require__(2)(module));
  }},{WEBPACK4_HARMONY_MODULE_DECORATOR}
"#
    );
    let output = unpack(&webpack4_bundle(&factories, 0), DecompileOptions::default())
        .expect("webpack4 unpack should succeed");
    let harmony = module_code(&output, "module-1.js");
    assert!(!harmony.contains(".default(module)"), "{harmony}");
    assert!(
        harmony.contains("})(module)"),
        "the wrapper must stay:\n{harmony}"
    );
}

fn free_name_capture_failures(
    output: &wakaru_core::driver::test_support::UnpackOutput,
) -> Vec<String> {
    let mut filenames = output
        .warnings
        .iter()
        .filter(|warning| {
            warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
                && warning.message.contains("capturing a free reference")
        })
        .map(|warning| warning.filename.clone())
        .collect::<Vec<_>>();
    filenames.sort();
    filenames
}

#[test]
fn webpack5_free_name_capture_isolates_only_its_factory() {
    // Factory 1 reads a free `require` as a value and factory 2 a free
    // `exports` (an ESM module's environment probe). Renaming their loader or
    // exports parameter would capture those names.
    let source = r#"
(() => {
  var modules = ({
    0: ((e, t, n) => {
      e.exports = [n(1), n(2), n(3)];
    }),
    1: ((e, t, n) => {
      e.exports = function () { return [n(3), globalThis.hostRequire = require]; };
    }),
    2: ((e, t, n) => {
      n.d(t, { probe: () => r });
      var r = typeof exports;
    }),
    3: ((e) => {
      e.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-free-name-capture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a free-name capture should isolate only its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert_eq!(
        free_name_capture_failures(&output),
        ["module-1.js", "module-2.js"],
        "unexpected warnings: {:?}",
        output.warnings
    );
    let code = |name: &str| {
        output
            .modules
            .iter()
            .find(|(filename, _)| filename == name)
            .map(|(_, code)| code.clone())
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    let opaque = code("module-1.js");
    assert!(
        opaque.contains("globalThis.hostRequire = require"),
        "{opaque}"
    );
    let entry = code("module-0.js");
    assert!(entry.contains("require(1)"), "{entry}");
    assert!(entry.contains("require(2)"), "{entry}");
    assert!(entry.contains("./module-3.js"), "{entry}");
    assert!(code("module-3.js").contains("dependency"));
}

#[test]
fn webpack4_free_name_capture_isolates_only_its_factory() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(2);
}([
  function(e, t, n) {
    e.exports = function () { return [n(1), typeof require]; };
  },
  function(e) {
    e.exports = "stable";
  },
  function(e, t, n) {
    e.exports = [n(0), n(1)];
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-free-name-capture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a free-name capture should isolate only its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert_eq!(
        free_name_capture_failures(&output),
        ["module-0.js"],
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| code)
        .expect("expected recoverable sibling factory");
    assert!(entry.contains("require(0)"), "{entry}");
    assert!(entry.contains("./module-1.js"), "{entry}");
}

#[test]
fn webpack5_free_require_calls_share_the_loader_name() {
    // `__non_webpack_require__("os")` compiles to a free `require` call. The
    // output's `require` is the host require, so the loader parameter takes
    // the name and the free call stays as written.
    let source = r#"
(() => {
  var modules = ({
    0: ((e, t, n) => {
      e.exports = function (name) { return [n(1), require("os"), require(name)]; };
    }),
    1: ((e) => {
      e.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-free-require-call.js".to_string(),
            ..Default::default()
        },
    )
    .expect("free require calls should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert_eq!(
        free_name_capture_failures(&output),
        Vec::<String>::new(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered entry");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains(r#"require("os")"#), "{entry}");
    assert!(entry.contains("require(name)"), "{entry}");
}

#[test]
fn webpack5_free_require_calls_that_name_a_module_id_stay_opaque() {
    // A free call with a number or an id-like string would be rewritten into
    // an internal edge once it shares the loader's name.
    for call in [
        r#"require(1)"#,
        r#"require("1")"#,
        r#"require.resolve("os")"#,
    ] {
        let source = format!(
            r#"
(() => {{
  var modules = ({{
    0: ((e, t, n) => {{
      e.exports = function () {{ return [n(1), {call}]; }};
    }}),
    1: ((e) => {{
      e.exports = "dependency";
    }})
  }});
  var cache = {{}};
  (function load(id) {{
    var module = cache[id] = {{ exports: {{}} }};
    modules[id](module, module.exports, load);
    return module.exports;
  }})(0);
}})();
"#
        );
        let output = unpack(
            &source,
            DecompileOptions {
                filename: "webpack5-free-require-id.js".to_string(),
                ..Default::default()
            },
        )
        .expect("an isolated factory should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5], "{call}");
        assert_eq!(
            free_name_capture_failures(&output),
            ["module-0.js"],
            "{call}: unexpected warnings: {:?}",
            output.warnings
        );
    }
}

#[test]
fn webpack5_free_name_capture_in_a_named_id_container_stays_opaque() {
    let source = r#"
(() => {
  var modules = ({
    "./src/entry.js": ((e, t, n) => {
      e.exports = [n("./src/host.js"), n("./src/value.js")];
    }),
    "./src/host.js": ((e, t, n) => {
      e.exports = function () { return [n("./src/value.js"), globalThis.hostRequire = require]; };
    }),
    "./src/value.js": ((e) => {
      e.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })("./src/entry.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-named-free-name-capture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an isolated factory should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert_eq!(free_name_capture_failures(&output), ["src/host.js"]);
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "src/entry.js")
        .map(|(_, code)| code)
        .expect("the consumer should remain recovered");
    assert!(!entry.contains("\"./host.js\""), "{entry}");
    assert!(entry.contains("\"./value.js\""), "{entry}");
}

#[test]
fn webpack5_exports_reuse_that_reads_the_old_value_keeps_the_write() {
    // shape: hypothetical (an initializer that reads the old value has no
    // provable lifetime boundary)
    let source = r#"
(() => {
  var modules = ({
    0: ((module, publicValue) => {
      publicValue = chooseValue(publicValue, globalThis.replacement);
      module.exports = publicValue;
    }),
    1: ((module, exports, load) => {
      module.exports = [load(0), "stable"];
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(1);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-read-before-exports-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("value-flow reuse should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    let provider = assert_commonjs_exports_provider(&output, "module-0.js");
    assert!(
        provider.contains("exports = chooseValue(exports, globalThis.replacement);"),
        "the write reads the module's own exports binding:\n{provider}"
    );
    assert!(provider.contains("module.exports = exports;"), "{provider}");
    let consumer = module_code(&output, "module-1.js");
    assert!(consumer.contains("./module-0.js"), "{consumer}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_post_loader_write_module_decorator_shape_stays_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((context, exports, load) => {
      load = load(1);
      context = load.hmd(context);
      context.exports = "local";
    }),
    1: ((module) => {
      module.exports = { hmd: value => value };
    }),
    2: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(2);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-post-write-decorator-lookalike.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a second-lifetime decorator lookalike should isolate its factory");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque decorator-lookalike factory");
    assert!(opaque.contains("load.hmd(context)"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack4_opaque_loader_reuse_preserves_other_structural_modules() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(2);
}([
  function(module, exports, load) {
    if (globalThis.useAlternate) load = globalThis.alternateLoader;
    module.exports = load;
  },
  function(module) {
    module.exports = "stable";
  },
  function(module, exports, load) {
    module.exports = [load(0), load(1)];
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-mixed-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("one unsupported webpack 4 factory must not discard its container");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert_eq!(output.modules.len(), 3);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("require(0)"))
        .map(|(_, code)| code)
        .expect("consumer should remain structurally recovered");
    assert!(entry.contains("require(0)"), "{entry}");
    assert!(!entry.contains("./module-0.js"), "{entry}");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_assignment_can_initialize_from_a_module() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const get = (load = load(1)).get;
      module.exports = get();
    }),
    1: ((module) => {
      module.exports = { get: () => "ok" };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-reused-loader.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 reused loader parameter should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);

    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains(".get"))
        .map(|(_, code)| code)
        .expect("expected webpack5 module that reuses the loader");
    assert!(
        entry.contains("import "),
        "expected recovered import:\n{entry}"
    );
    assert!(
        !entry.contains("require"),
        "the nested loader assignment must not retain a free require:\n{entry}"
    );
    assert!(
        entry.contains("import _load from") && entry.contains("const get = _load.get;"),
        "the assigned module value must have a declared owner:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_function_keeps_followup_property_initialization() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const dependency = load(1);
      (load = function(value) { return value; }).normalize = value => String(value);
      module.exports = [dependency, load("ok"), load.normalize(2)];
    }),
    1: ((module) => {
      module.exports = "dependency";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-reused-loader-function.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 function reuse should unpack");
    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);

    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("normalize"))
        .map(|(_, code)| code)
        .expect("expected module with the recovered function local");
    assert!(
        entry.contains("import "),
        "expected recovered import:\n{entry}"
    );
    assert!(
        !entry.contains("require"),
        "the function-valued local must not retain a free require:\n{entry}"
    );
    assert!(
        entry.contains("const _load =") && entry.contains("_load.normalize ="),
        "the property writer must target a declared recovered local:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_normalizes_only_the_loader_lifetime() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      "use strict";
      load.r(exports);
      const before = load(1);
      const eager = load.bind(load, 1);
      const getter = load.n(before);
      const namespaceValue = load.t(before, 2).value;
      load = load(2);
      load.r(exports);
      const direct = load(1);
      const lateEager = load.bind(load, 1);
      const lateGetter = load.n(before);
      const lateNamespace = load.t(before, 2);
      const runtimeGlobal = load.g;
      module.exports = [
        before,
        eager,
        getter,
        namespaceValue,
        direct,
        lateEager,
        lateGetter,
        lateNamespace,
        runtimeGlobal,
        load
      ];
    }),
    1: ((module) => {
      module.exports = "dependency";
    }),
    2: ((module) => {
      module.exports = function runtimeValue(value) { return value; };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-lifetimes.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a proven loader/local lifetime boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered loader-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("./module-2.js"), "{entry}");
    assert!(
        entry.contains("_load.r(exports)")
            && entry.contains("_load(1)")
            && entry.contains("_load.bind(_load, 1)")
            && entry.contains("_load.n")
            && entry.contains("_load.t")
            && entry.contains("_load.g"),
        "post-write calls and helpers belong to the localized value:\n{entry}"
    );
    assert!(
        !entry.contains("require.r")
            && !entry.contains("require.t")
            && entry.matches("_load.r(exports)").count() == 1,
        "pre-write webpack helpers should be consumed:\n{entry}"
    );
    // The arbitrary post-write runtime value still receives the factory's
    // original `exports` object. That object has no proven ESM replacement;
    // keep the residual visible rather than pretending the graph is clean.
    let findings = validate_output_modules(&output.modules);
    assert_eq!(findings.len(), 1, "unexpected findings: {findings:#?}");
    assert_eq!(
        findings[0].kind,
        wakaru_core::OutputFindingKind::EsmCommonJsResidual
    );
    assert_eq!(findings[0].filename, "module-0.js");
}

#[test]
fn webpack4_reused_loader_normalizes_prewrite_runtime_helpers() {
    let source = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load(0);
}([
  function(module, exports, load) {
    "use strict";
    load.r(exports);
    load.d(exports, "value", function() { return value; });
    var value = load(1);
    load = load(2);
    load.r(exports);
    var direct = load(1);
    module.exports = [value, direct, load];
  },
  function(module) {
    module.exports = "dependency";
  },
  function(module) {
    module.exports = function runtimeValue(value) { return value; };
  }
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack4-loader-helpers.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack 4 runtime helpers before a reuse boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack4]);
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(_, code)| code.contains("runtimeValue") || code.contains("_load"))
        .map(|(_, code)| code)
        .expect("expected recovered webpack 4 loader-reuse module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("./module-2.js"), "{entry}");
    assert!(entry.contains("_load.r(exports)"), "{entry}");
    assert!(entry.contains("_load(1)"), "{entry}");
    assert!(
        !entry.contains("require.r")
            && !entry.contains("require.d")
            && entry.matches("_load.r(exports)").count() == 1,
        "{entry}"
    );
    // The arbitrary post-write runtime value still receives the factory's
    // original `exports` object. That object has no proven ESM replacement;
    // keep the residual visible rather than pretending the graph is clean.
    let findings = validate_output_modules(&output.modules);
    assert_eq!(findings.len(), 1, "unexpected findings: {findings:#?}");
    assert_eq!(
        findings[0].kind,
        wakaru_core::OutputFindingKind::EsmCommonJsResidual
    );
    assert_eq!(findings[0].filename, "entry.js");
}

#[test]
fn webpack5_reused_loader_var_redeclaration_starts_the_local_lifetime() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      var load = load(1);
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "localized";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-redeclaration.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a var redeclaration should identify the loader lifetime boundary");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered redeclaration module");
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(!entry.contains("require(1)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_named_loader_normalizes_only_prewrite_ids() {
    let source = r#"
(() => {
  var modules = ({
    "./entry.js": ((module, exports, load) => {
      const before = load("./before.js");
      load = load("./runtime.js");
      const after = load("./before.js");
      module.exports = [before, after, load];
    }),
    "./before.js": ((module) => {
      module.exports = "before";
    }),
    "./runtime.js": ((module) => {
      module.exports = function runtimeValue(value) { return value; };
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })("./entry.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-named-loader-lifetimes.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a proven named-ID loader boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "entry.js")
        .map(|(_, code)| code)
        .expect("expected recovered named-ID entry");
    assert!(entry.contains("./before.js"), "{entry}");
    assert!(entry.contains("./runtime.js"), "{entry}");
    assert!(
        entry.contains("_load(\"./before.js\")"),
        "the post-write string call must retain the localized runtime:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_recovers_proven_commonjs_reads_without_runtime_residuals() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load(1);
      load(2);
      module.exports = "entry";
    }),
    1: ((module) => {
      const api = () => "ready";
      module.exports = api;
      if (typeof window !== "undefined") {
        window.syntheticApi = module.exports;
      }
    }),
    2: ((module, exports) => {
      exports.second = exports.first = void 0;
      exports.first = function(value) { return value + 1; };
      exports.second = function(value) { return exports.first(value); };
      consume(exports.second(1));
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-commonjs-read-recovery.js".to_string(),
            ..Default::default()
        },
    )
    .expect("the synthetic webpack container should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    let default_module = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code)
        .expect("expected the default-exporting module");
    assert!(
        default_module.contains("window.syntheticApi = api"),
        "{default_module}"
    );
    let named_module = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-2.js")
        .map(|(_, code)| code)
        .expect("expected the named-exporting module");
    assert!(named_module.contains("first(value)"), "{named_module}");
    assert!(
        named_module.contains("consume(second(1))"),
        "{named_module}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let mapped = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-commonjs-read-recovery.js".to_string(),
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("source-map materialization should run the same recovery");
    assert_eq!(validate_output_modules(&mapped.modules), vec![]);
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-1.js"));
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-2.js"));
}

#[test]
fn webpack5_observable_self_require_stays_at_the_commonjs_boundary() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load(1);
    }),
    1: ((module, exports, load) => {
      var self = load(1);
      observe(exports === self);
      for (var key in self) globalThis[key] = self[key];
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-self-require.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the synthetic webpack self-require should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        let module = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected the self-requiring factory module");
        assert!(
            module.contains(r#"require("./module-1.js")"#)
                && !module.contains(r#"import self from "./module-1.js""#),
            "an observed factory exports alias must keep the self-require at the CommonJS boundary:\n{module}"
        );
        let findings = validate_output_modules(&output.modules);
        assert!(
            findings.iter().all(|finding| {
                finding.kind != wakaru_core::OutputFindingKind::MissingImportedName
            }),
            "CommonJS-boundary preservation must avoid a false default-export contract: {findings:#?}"
        );
    }
}

#[test]
fn webpack5_self_required_empty_default_remains_linkable_to_consumers() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      function asDefault(value) {
        return value && value.__esModule ? value : { default: value };
      }
      var dependency = asDefault(load(1));
      consume(dependency.default.open);
    }),
    1: ((module, exports, load) => {
      var current = load(1);
      for (var key in current) globalThis[key] = current[key];
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let raw = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-self-default-consumer.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw extraction should preserve the detector body");
    let raw_provider = raw
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code)
        .expect("expected raw self-required provider");
    assert!(
        raw_provider.contains(r#"require("./module-1.js")"#)
            && !raw_provider.contains("module.exports = {};"),
        "runtime-default restoration must stay out of raw output:\n{raw_provider}"
    );

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-self-default-consumer.js".to_string(),
                emit_source_map,
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("the synthetic webpack self-require consumer should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        assert!(
            output
                .warnings
                .iter()
                .all(|warning| warning.kind != wakaru_core::UnpackWarningKind::TdzViolation),
            "the restored self default must initialize before its first read: {:#?}",
            output.warnings
        );
        let provider = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected self-required provider");
        let default = provider
            .find("export default")
            .expect("webpack's runtime-created object should become a default export");
        let first_read = provider
            .find("for(")
            .expect("expected the self-required object read");
        assert!(
            default < first_read,
            "the self default must initialize before the loop reads it:\n{provider}"
        );
        let findings = validate_output_modules(&output.modules);
        assert!(
            findings.iter().all(|finding| {
                finding.kind != wakaru_core::OutputFindingKind::MissingImportedName
            }),
            "a consumer must not default-import an unexported self-required value: {findings:#?}"
        );
    }
}

#[test]
fn webpack5_recovers_coupled_lazy_default_helpers_across_phase1_paths() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const helper = load(1);
      consume(helper("value"), helper.default);
      module.exports = "entry";
    }),
    1: ((module) => {
      function helper(value) {
        module.exports = helper = (next) => typeof next;
        module.exports.default = module.exports;
        return helper(value);
      }
      module.exports = helper;
      module.exports.default = module.exports;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-coupled-lazy-default.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the coupled lazy helper should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        let helper = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected the helper module");
        assert!(
            helper.contains("export { helper as default }"),
            "the mutable helper needs a live default binding:\n{helper}"
        );
        assert_eq!(
            helper.matches("helper.default = helper").count(),
            2,
            "both CommonJS property mirrors must remain:\n{helper}"
        );
        assert!(!helper.contains("module.exports"), "{helper}");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        if emit_source_map {
            assert!(output
                .source_maps
                .iter()
                .any(|(filename, _)| filename == "module-1.js"));
        }
    }
}

#[test]
fn webpack5_recovers_default_only_compat_adapter_without_runtime_residuals() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      module.exports = load(1);
    }),
    1: ((module, exports, load) => {
      var typeHelpers = load(2);
      Object.defineProperty(exports, "__esModule", {
        value: true
      });
      Object.defineProperty(exports, "default", {
        enumerable: true,
        get: function() {
          return entry;
        }
      });
      function entry(value) {
        return value;
      }
      ("function" == typeof exports.default || "object" === typeHelpers._(exports.default) && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", {
        value: true
      }), Object.assign(exports.default, exports), module.exports = exports.default);
    }),
    2: ((module, exports) => {
      function typeOf(value) {
        return typeof value;
      }
      exports._ = typeOf;
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    for emit_source_map in [false, true] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: "webpack5-default-only-compat-adapter.js".to_string(),
                emit_source_map,
                ..Default::default()
            },
        )
        .expect("the exact default-only adapter should unpack");

        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        let provider = output
            .modules
            .iter()
            .find(|(filename, _)| filename == "module-1.js")
            .map(|(_, code)| code)
            .expect("expected the default-only provider");
        assert!(
            provider.contains("export { entry as default }"),
            "{provider}"
        );
        assert!(provider.contains("(entry) === \"object\""), "{provider}");
        assert!(provider.contains("entry.default = entry"), "{provider}");
        assert!(!provider.contains("exports.default"), "{provider}");
        assert!(!provider.contains("module.exports"), "{provider}");
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        if emit_source_map {
            assert!(
                output
                    .source_maps
                    .iter()
                    .any(|(filename, _)| filename == "module-1.js"),
                "the recovered provider should retain its source map"
            );
        }
    }
}

#[test]
fn webpack5_reused_loader_splits_a_mid_initializer_sequence() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      var absent = load(137), marker,
        build = (marker = load(1), load = load(2), function() { return load; });
      module.exports = [absent, marker, build(), load];
    }),
    1: ((module) => {
      module.exports = "dependency";
    }),
    2: ((module) => {
      module.exports = "localized";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-initializer-sequence.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a mid-initializer loader boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(
        output.warnings.iter().all(|warning| {
            warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        }),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered initializer module");
    assert!(
        entry.contains("require(137)"),
        "an absent numeric id must remain an honest runtime call:\n{entry}"
    );
    assert!(
        !entry.contains("module-137") && !entry.contains("from \"137\""),
        "an absent numeric id must not synthesize an ESM edge:\n{entry}"
    );
    assert!(entry.contains("./module-1.js"), "{entry}");
    assert!(entry.contains("./module-2.js"), "{entry}");
    assert!(
        entry.matches("_load").count() >= 3,
        "the suffix closure and later reads must use the localized value:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_splits_a_top_level_sequence_in_order() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      observe("before"), load = alternateRuntime, observe("after", load);
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-loader-top-level-sequence.js".to_string(),
            ..Default::default()
        },
    )
    .expect("a top-level sequence boundary should unpack");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered sequence module");
    let before = entry.find("observe(\"before\")").expect("prefix effect");
    let local = entry.find("alternateRuntime").expect("localized write");
    let after = entry.find("observe(\"after\"").expect("suffix effect");
    assert!(
        before < local && local < after,
        "evaluation order changed:\n{entry}"
    );
    assert!(entry.contains("observe(\"after\", _load)"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_keeps_consumed_mid_sequence_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const value = (observe("before"), load = load(1)).value;
      module.exports = value;
    }),
    1: ((module) => {
      module.exports = { value: "stable" };
    }),
    2: ((module) => {
      module.exports = "independent";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(2);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-consumed-loader-sequence.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an unsupported factory should stay isolated");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque factory");
    assert!(opaque.contains("observe(\"before\")"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_recovers_conditional_runtime_global_reads() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load = typeof load.g === "object" && load.g && load.g.Object === Object && load.g;
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "independent";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-conditional-runtime-global.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack runtime-global reads should identify the old loader lifetime");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().all(|warning| {
        warning.kind != wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected recovered runtime-global module");
    assert!(
        !entry.contains("require.g") && !entry.contains("load.g"),
        "{entry}"
    );
    assert!(entry.contains("global"), "{entry}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_reused_loader_keeps_bare_old_lifetime_values_opaque() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load = typeof factory === "function"
        ? factory.call(exports, load, exports, module)
        : factory;
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "independent";
    })
  });
  var cache = {};
  (function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  })(1);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-bare-loader-lifetime.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an unsupported factory should stay isolated");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("expected opaque factory");
    assert!(opaque.contains("factory.call(exports, load"), "{opaque}");
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_opaque_loader_reuse_preserves_other_structural_modules() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      const opaque = load(1);
      const stable = load(2);
      module.exports = [opaque, stable];
    }),
    1: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    2: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-mixed-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("one unsupported factory must not discard its container");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert_eq!(output.modules.len(), 4);
    let failures = output
        .warnings
        .iter()
        .filter(|warning| {
            warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failures.len(),
        1,
        "unexpected warnings: {:?}",
        output.warnings
    );
    assert_eq!(failures[0].filename, "module-1.js");

    let opaque = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-1.js")
        .map(|(_, code)| code)
        .expect("opaque factory should be preserved");
    assert!(opaque.contains("if (globalThis.useAlternate)"), "{opaque}");

    let entry = output
        .modules
        .iter()
        .find(|(filename, _)| filename == "module-0.js")
        .map(|(_, code)| code)
        .expect("recoverable entry should be emitted");
    assert!(
        entry.contains("require(1)"),
        "an opaque target must follow the absent-id convention:\n{entry}"
    );
    assert!(
        !entry.contains("./module-1.js"),
        "an opaque target must not gain a synthetic graph edge:\n{entry}"
    );
    assert!(
        entry.contains("./module-2.js"),
        "the independent structural edge should still recover:\n{entry}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);

    let mapped = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-mixed-loader-reuse.js".to_string(),
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("source-map materialization must preserve the opaque sidecar");
    assert!(mapped.warnings.iter().any(|warning| {
        warning.filename == "module-1.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    assert!(mapped
        .source_maps
        .iter()
        .all(|(filename, _)| filename != "module-1.js"));
    assert!(mapped
        .source_maps
        .iter()
        .any(|(filename, _)| filename == "module-2.js"));
}

#[test]
fn webpack_named_id_loader_reuse_stays_opaque() {
    let webpack4 = r#"
!function(modules) {
  function load(id) {
    var module = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  return load("./entry.js");
}({
  "./opaque.js": function(module, exports, load) {
    if (globalThis.useAlternate) load = globalThis.alternateLoader;
    module.exports = load;
  },
  "./entry.js": function(module, exports, load) {
    module.exports = load("./opaque.js");
  }
});
"#;
    let webpack5 = r#"
(() => {
  var modules = ({
    "./opaque.js": ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    "./entry.js": ((module, exports, load) => {
      module.exports = load("./opaque.js");
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load("./entry.js");
})();
"#;

    for (filename, source, format) in [
        (
            "webpack4-named-loader-reuse.js",
            webpack4,
            BundleFormat::Webpack4,
        ),
        (
            "webpack5-named-loader-reuse.js",
            webpack5,
            BundleFormat::Webpack5,
        ),
    ] {
        let output = unpack(
            source,
            DecompileOptions {
                filename: filename.to_string(),
                ..Default::default()
            },
        )
        .expect("one opaque factory should not discard its container");
        assert_eq!(output.detected_formats, [format], "{filename}");
        assert!(
            output.warnings.iter().any(|warning| {
                warning.filename == "opaque.js"
                    && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
            }),
            "{filename}: {:?}",
            output.warnings
        );
        let opaque = output
            .modules
            .iter()
            .find(|(name, _)| name == "opaque.js")
            .map(|(_, code)| code)
            .unwrap_or_else(|| panic!("{filename}: the opaque module should be emitted"));
        assert!(opaque.contains("useAlternate"), "{filename}: {opaque}");
        assert!(
            output.modules.iter().any(|(name, _)| name == "entry.js"),
            "{filename}: {:?}",
            output.modules
        );
    }
}

#[test]
fn webpack5_opaque_loader_reuse_retains_entry_provenance() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    }),
    1: ((module) => {
      module.exports = "stable";
    })
  });
  var cache = {};
  function __nccwpck_require__(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, __nccwpck_require__);
    return module.exports;
  }
  module.exports = __nccwpck_require__(0);
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-opaque-entry.js".to_string(),
            ..Default::default()
        },
    )
    .expect("an opaque entry should coexist with recoverable siblings");

    assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
    assert!(output
        .provenance
        .iter()
        .any(|provenance| { provenance.filename == "module-0.js" && provenance.is_entry }));
    assert!(output.warnings.iter().any(|warning| {
        warning.filename == "module-0.js"
            && warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
    }));
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_loader_reuse_demotion_reaches_an_order_independent_fixed_point() {
    fn run(module_table: &str) -> Vec<String> {
        let source = format!(
            r#"
(() => {{
  var modules = ({{ {module_table} }});
  var cache = {{}};
  (function load(id) {{
    var module = cache[id] = {{ exports: {{}} }};
    modules[id](module, module.exports, load);
    return module.exports;
  }})(2);
}})();
"#
        );
        let output = unpack(
            &source,
            DecompileOptions {
                filename: "webpack5-loader-fixed-point.js".to_string(),
                ..Default::default()
            },
        )
        .expect("the stable factory should keep the container recoverable");
        assert_eq!(output.detected_formats, [BundleFormat::Webpack5]);
        assert_eq!(validate_output_modules(&output.modules), vec![]);
        let mut failed = output
            .warnings
            .iter()
            .filter(|warning| {
                warning.kind == wakaru_core::UnpackWarningKind::WebpackFactoryRecoveryFailed
            })
            .map(|warning| warning.filename.clone())
            .collect::<Vec<_>>();
        failed.sort();
        failed
    }

    let dependent = r#"0: ((module, exports, load) => {
      load = load(1);
      module.exports = load;
    })"#;
    let inherently_opaque = r#"1: ((module, exports, load) => {
      if (globalThis.useAlternate) load = globalThis.alternateLoader;
      module.exports = load;
    })"#;
    let stable = r#"2: ((module) => { module.exports = "stable"; })"#;

    let forward = run(&format!("{dependent}, {inherently_opaque}, {stable}"));
    let reverse = run(&format!("{stable}, {inherently_opaque}, {dependent}"));
    assert_eq!(
        forward,
        vec!["module-1.js"],
        "an absent numeric target stays an honest runtime call instead of demoting its caller"
    );
    assert_eq!(reverse, forward);
}

#[test]
fn webpack5_reused_loader_with_conditional_first_write_fails_closed() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      if (useAlternate) load = alternateLoader;
      module.exports = load;
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-conditional-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unsupported loader reuse should preserve the input");
    assert!(
        output.detected_formats.is_empty(),
        "an unprovable conditional boundary must reject structural extraction"
    );
    assert_eq!(output.modules.len(), 1, "expected one fallback module");
    assert!(
        output.modules[0].1.contains("if (useAlternate)"),
        "fallback output must preserve the conditional write"
    );
}

#[test]
fn webpack5_reused_loader_initializer_that_reads_old_loader_fails_closed() {
    let source = r#"
(() => {
  var modules = ({
    0: ((module, exports, load) => {
      load = (() => load.runtime)();
      module.exports = load;
    })
  });
  var cache = {};
  function load(id) {
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
})();
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "webpack5-read-before-loader-reuse.js".to_string(),
            ..Default::default()
        },
    )
    .expect("read-before-write loader reuse should preserve the input");
    assert!(
        output.detected_formats.is_empty(),
        "an initializer that executes a read of the old loader must reject extraction"
    );
    assert_eq!(output.modules.len(), 1, "expected one fallback module");
    assert!(
        output.modules[0].1.contains("load.runtime"),
        "fallback output must preserve the old loader read"
    );
}

#[test]
fn webpack5_extensionless_string_id_gets_a_resolvable_javascript_filename() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "pkg/side-effect": ((module) => {
      module.exports = "loaded";
    }),
    "./src/index.js": ((module, exports, __webpack_require__) => {
      __webpack_require__("pkg/side-effect");
      module.exports = "entry";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/index.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-path-id.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 path-like module id should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.js")
        .map(|(_, code)| code)
        .expect("expected index module");
    assert!(
        index.contains(r#""../pkg/side-effect.js""#),
        "the path-like module id must be relative to its consumer:\n{index}"
    );
    assert!(
        output
            .modules
            .iter()
            .any(|(name, _)| name == "pkg/side-effect.js"),
        "expected the extensionless module id to append .js"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_string_id_collisions_and_queries_keep_distinct_edges() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./widgets/Button.less": ((module) => {
      module.exports = "compiled style";
    }),
    "./widgets/Button.less.js": ((module) => {
      module.exports = "authored JavaScript";
    }),
    "./App.vue?vue&type=script": ((module) => {
      module.exports = "script block";
    }),
    "./App.vue?vue&type=style&index=0": ((module) => {
      module.exports = "style block";
    }),
    "./src/index.js": ((module, exports, __webpack_require__) => {
      const style = __webpack_require__("./widgets/Button.less");
      const authored = __webpack_require__("./widgets/Button.less.js");
      const script = __webpack_require__("./App.vue?vue&type=script");
      const vueStyle = __webpack_require__("./App.vue?vue&type=style&index=0");
      module.exports = [style, authored, script, vueStyle];
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/index.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-string-id-collisions.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 colliding and queried module ids should unpack");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"widgets/Button.less.js"), "{names:?}");
    assert!(names.contains(&"widgets/Button.less_2.js"), "{names:?}");
    assert!(names.contains(&"App.vue.js"), "{names:?}");
    assert!(names.contains(&"App.vue_2.js"), "{names:?}");
    assert!(
        names.iter().all(|name| !name.contains(['?', '#'])),
        "{names:?}"
    );

    let index = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/index.js")
        .map(|(_, code)| code)
        .expect("expected index module");
    for specifier in [
        "../widgets/Button.less.js",
        "../widgets/Button.less_2.js",
        "../App.vue.js",
        "../App.vue_2.js",
    ] {
        assert!(
            index.contains(&format!(r#""{specifier}""#)),
            "missing {specifier} in consumer:\n{index}"
        );
    }
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn webpack5_string_module_id_with_overlapping_dots_cannot_emit_parent_path() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "....//node_modules/@wakaru/cli/bin/wakaru": ((module) => {
      module.exports = "pwned";
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  console.log(__webpack_require__("....//node_modules/@wakaru/cli/bin/wakaru"));
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-overlap-path.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let names: Vec<&str> = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        names.contains(&"..._/node_modules/@wakaru/cli/bin/wakaru.js"),
        "expected sanitized overlap path, got {names:?}"
    );
    assert!(
        names
            .iter()
            .all(|name| !name.split('/').any(|part| part == "..")),
        "module filenames must not contain parent components: {names:?}"
    );
}

#[test]
fn webpack5_require_g_is_recovered_as_global() {
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    "./src/browser-process.js": ((module) => {
      module.exports = { env: {} };
    }),
    "./src/global.js": ((__unused_webpack_module, exports, __webpack_require__) => {
      exports.envProcess = __webpack_require__.g.process?.env && typeof __webpack_require__.g.process?.env === "object"
        ? __webpack_require__.g.process
        : __webpack_require__("./src/browser-process.js");
      exports.readLocal = function(require) {
        return require.g;
      };
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) return cachedModule.exports;
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.g = (function() {
    if (typeof globalThis === "object") return globalThis;
    try {
      return this || new Function("return this")();
    } catch (e) {
      if (typeof window === "object") return window;
    }
  })();
  __webpack_require__("./src/global.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-require-g.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let global = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/global.js")
        .map(|(_, code)| code)
        .expect("expected global module");

    assert!(
        global.contains("global.process?.env")
            && global.contains("typeof global.process?.env === \"object\"")
            && global.contains("global.process"),
        "expected webpack require.g to recover as global:\n{global}"
    );
    assert!(
        !global.contains("require.g.process"),
        "webpack require.g.process should not survive:\n{global}"
    );
    assert!(
        global.contains("function(require)") && global.contains("return require.g"),
        "inner parameter named require should not be rewritten:\n{global}"
    );
}

#[test]
fn webpack5_amd_and_module_decorators_are_recovered() {
    let source = r#"
(() => {
    var __webpack_modules__ = ({
    "./src/runtime-helpers.js": ((module, exports, __webpack_require__) => {
      module = __webpack_require__.hmd(module);
      __webpack_require__.d(exports, { named: function() { return named; } }), module = __webpack_require__.hmd(module);
      const named = 1;
      exports.amd = __webpack_require__.amdO;
      exports.load = function(name) {
        return module.require(name);
      };
      exports.localRequire = function(require) {
        return require.amdO;
      };
    }),
    "./src/node-module.js": ((module, exports, __webpack_require__) => {
      module = __webpack_require__.nmd(module);
      exports.children = module.children;
      exports.localModule = function(module) {
        module = __webpack_require__.nmd(module);
        return module.children;
      };
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) return cachedModule.exports;
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__("./src/runtime-helpers.js");
  __webpack_require__("./src/node-module.js");
})();
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "webpack5-runtime-helpers.js".to_string(),
            ..Default::default()
        },
    )
    .expect("webpack5 unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let runtime_helpers = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/runtime-helpers.js")
        .map(|(_, code)| code)
        .expect("expected runtime helpers module");
    assert!(
        runtime_helpers.contains(r#"typeof define === "function" && define.amd"#),
        "expected require.amdO to recover as AMD detection:\n{runtime_helpers}"
    );
    assert!(
        !runtime_helpers.contains("module = require.hmd(module)")
            && !runtime_helpers.contains("amd = require.amdO"),
        "webpack hmd/amdO helpers should not survive:\n{runtime_helpers}"
    );
    assert!(
        runtime_helpers.contains("function(require)")
            && runtime_helpers.contains("return require.amdO"),
        "inner parameter named require should not be rewritten:\n{runtime_helpers}"
    );

    let node_module = output
        .modules
        .iter()
        .find(|(name, _)| name == "src/node-module.js")
        .map(|(_, code)| code)
        .expect("expected node module");
    let nmd_decorator_count = node_module.matches("module = require.nmd(module);").count();
    assert!(
        nmd_decorator_count == 1,
        "only shadowed local module decorator should remain:\n{node_module}"
    );
}

#[test]
fn webpack5_module_decorators_in_expression_position_are_recovered() {
    // `hmd` / `nmd` return their argument, so each decorated factory must
    // recover exactly like the same factory without the decorator. The
    // decorator on the shadowing inner `e` parameter is a statement and is
    // not part of the substitution.
    let decorated = r#"
(() => {
  var modules = ({
    0: ((e, t, n) => {
      (e = n.nmd(e)).exports = function() { return "member"; };
    }),
    1: ((e, t, n) => {
      !function(m, root) {
        m.exports = { root: root };
      }((e = n.nmd(e)), this);
    }),
    2: ((e, t, n) => {
      void 0 !== (e = n.hmd(e)) && (e.exports = function(e) {
        e = n.nmd(e);
        return e.children;
      });
    }),
    3: ((e, t, n) => {
      var r = (e = n.hmd(e)).exports || "fallback";
      t.value = r;
    }),
    4: ((e) => {
      e.exports = "control";
    })
  });
  var cache = {};
  function load(id) {
    var cached = cache[id];
    if (cached !== undefined) return cached.exports;
    var module = cache[id] = { exports: {} };
    modules[id](module, module.exports, load);
    return module.exports;
  }
  load(0);
  load(1);
  load(2);
  load(3);
  load(4);
})();
"#;
    let plain = decorated
        .replace("(e = n.nmd(e))", "e")
        .replace("(e = n.hmd(e))", "e");
    assert!(!plain.contains(".hmd(") && plain.matches(".nmd(").count() == 1);

    let unpack_named = |source: &str| {
        unpack(
            source,
            DecompileOptions {
                filename: "webpack5-expression-module-decorators.js".to_string(),
                ..Default::default()
            },
        )
        .expect("webpack5 unpack should succeed")
    };
    let decorated_output = unpack_named(decorated);
    let plain_output = unpack_named(&plain);
    assert_eq!(decorated_output.detected_formats, [BundleFormat::Webpack5]);
    // Module 3 reads the whole `module.exports` object, which stays a
    // reported CommonJS residual in the plain factory too; the module
    // comparison below proves the decorator changes nothing.
    let opaque: Vec<_> = decorated_output
        .warnings
        .iter()
        .filter(|warning| warning.kind != wakaru_core::UnpackWarningKind::CommonJsExportUnrecovered)
        .collect();
    assert!(
        opaque.is_empty(),
        "decorators should not make factories opaque: {opaque:?}"
    );
    assert_eq!(decorated_output.modules, plain_output.modules);

    let shadowed = decorated_output
        .modules
        .iter()
        .find(|(name, _)| name == "module-2.js")
        .map(|(_, code)| code)
        .expect("expected module-2.js");
    assert!(
        shadowed.contains("e = require.nmd(e)"),
        "the shadowing inner parameter keeps its decorator:\n{shadowed}"
    );
}

#[test]
fn browserify_unpack_extracts_multiple_modules() {
    let source_path = "../../testcases/browserify/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read browserify testcase");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("browserify unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    assert!(
        pairs.len() > 1,
        "expected browserify unpack to split modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "expected browserify unpack to include entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );

    let entry = pairs
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("expected browserify entry module");
    assert!(
        entry.contains(r#""./calculator.js""#) && entry.contains(r#""./greeting.js""#),
        "browserify dependency maps should target emitted module filenames:\n{entry}"
    );
    assert!(
        !entry.contains(r#""./calculator""#) && !entry.contains(r#""./greeting""#),
        "original browserify request names should be remapped:\n{entry}"
    );
}

#[test]
fn webpack5_does_not_misdetect_method_object_without_require_lifecycle() {
    // Regression: esbuild browser bundles contain vendor objects whose props
    // are all functions (e.g. rxjs's Immediate polyfill). Once the outer IIFE
    // is unwrapped, that body becomes a detection candidate — a real webpack5
    // modules object must be backed by the proven require lifecycle
    // (cache write + indexed invocation + returned exports); named-only
    // property access must not count as webpack5 evidence.
    let source = r#"
(() => {
    var handles = {};
    var Immediate = {
        setImmediate(cb) { handles[1] = cb; return 1; },
        clearImmediate(handle) { delete handles[handle]; }
    };
    var id = Immediate.setImmediate(() => console.log("tick"));
    Immediate.clearImmediate(id);
    console.log("done");
})();
"#;
    let output = unpack(source, DecompileOptions::default()).expect("unpack should succeed");
    let names: Vec<_> = output.modules.iter().map(|(n, _)| n.clone()).collect();
    assert!(
        !names.iter().any(|n| n.starts_with("module-")),
        "method-only object misdetected as a webpack5 modules object: {names:?}"
    );
}

#[test]
fn webpack5_kept_require_of_a_module_exports_provider_reads_its_default() {
    // shape: producer webpack@5.111.1 mode=production (Terser). Terser inlines
    // each single-use require into its use, so `o(971)(…)` and
    // `o(451)("toStringTag")` stay calls and their modules stay CommonJS. Each
    // provider was `module.exports = function` and is now `export default
    // function`, whose namespace `require` returns.
    let source = r#"(()=>{var t={97(t,e,o){o(817);var r=o(825);t.exports={tag:r("entry"),registry:globalThis.__polyfillRegistry}},971(t){t.exports=function(t,e){(globalThis.__polyfillRegistry||(globalThis.__polyfillRegistry=[])).push([t.target,t.forced,e.label,"export-module"])}},232(t){t.exports="function"!=typeof globalThis.FeatureNative},817(t,e,o){o(971)({target:"Feature",forced:o(232)},{label:"installer-module"})},825(t,e,o){var r=o(971),l=o(451)("toStringTag");t.exports=function(t){return l+":"+t+":"+typeof r}},451(t){t.exports=function(t){return"well-known-module:"+t}}};const e={};globalThis.__artificialSink=function o(r){const l=e[r];if(void 0!==l)return l.exports;const i=e[r]={exports:{}};return t[r](i,i.exports,o),i.exports}(97)})();"#;
    let output = unpack(source, DecompileOptions::default()).expect("bundle should unpack");
    let module = |filename: &str| {
        output
            .modules
            .iter()
            .find(|(name, _)| name == filename)
            .map(|(_, code)| code.clone())
            .expect("module should exist")
    };
    let installer = module("module-817.js");
    assert!(
        installer.contains(r#"require("./module-971.js").default({"#),
        "{installer}"
    );
    assert!(
        installer.contains(r#"forced: require("./module-232.js").default"#),
        "{installer}"
    );
    let tag = module("module-825.js");
    assert!(
        tag.contains(r#"require("./module-451.js").default("toStringTag")"#),
        "{tag}"
    );
    assert!(tag.contains("module.exports = "), "{tag}");
    assert!(module("module-971.js").contains("export default function"));
}

#[test]
fn commonjs_require_of_a_marked_provider_now_esm_is_reported() {
    // hypothetical, with webpack 5's `require.n`: module 1 keeps a require in
    // a function, so it stays CommonJS. `UnWebpackInterop` replaced the
    // interop getter with the binding, which then means module 2's default;
    // module 2 marked itself `__esModule` and is now ESM, so the binding holds
    // a namespace and `n.k` misses. Which reads mean the default is not
    // known, so the require is reported.
    let source = r#"(()=>{var t={1(t,e,o){var n=o(2),a=o.n(n);e.run=function(){return a().k+o(3).x}},2(t,e){"use strict";Object.defineProperty(e,"__esModule",{value:!0}),e.default={k:1}},3(t,e){e.x=1}};const e={};function o(r){const l=e[r];if(void 0!==l)return l.exports;const i=e[r]={exports:{}};return t[r](i,i.exports,o),i.exports}o.n=t=>{var e=t&&t.__esModule?()=>t.default:()=>t;return o.d(e,{a:e}),e},o.d=(t,e)=>{for(var r in e)o.o(e,r)&&!o.o(t,r)&&Object.defineProperty(t,r,{enumerable:!0,get:e[r]})},o.o=(t,e)=>Object.prototype.hasOwnProperty.call(t,e),globalThis.lib=o(1)})();"#;
    let output = unpack(source, DecompileOptions::default()).expect("bundle should unpack");
    let warnings = output
        .warnings
        .iter()
        .filter(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm)
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1, "{:#?}", output.warnings);
    assert_eq!(warnings[0].filename, "module-1.js");
    assert!(warnings[0].message.contains("\"./module-2.js\""));
    assert!(warnings[0]
        .message
        .contains("the interop that read its default was removed"));
    assert!(!warnings[0].kind.is_error());

    // An interop still in the output tests `__esModule`, which Node sets on
    // the namespace of a module with a default export: it reads the default,
    // so there is nothing to report.
    let tested = source.replace(
        "var n=o(2),a=o.n(n);e.run=function(){return a().k+",
        "var n=o(2);e.run=function(){return(n&&n.__esModule?n.default:n).k+",
    );
    let output = unpack(&tested, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-1.js")
        .map(|(_, code)| code.clone())
        .expect("consumer module");
    assert!(consumer.contains("__esModule"), "{consumer}");
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm),
        "{:#?}",
        output.warnings
    );

    // Without a default export Node adds no marker, so the same interop
    // wraps the namespace.
    let no_default = tested.replace("e.default={k:1}", "e.k=1");
    let output = unpack(&no_default, DecompileOptions::default()).expect("bundle should unpack");
    assert!(
        output.warnings.iter().any(|w| {
            w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm
                && w.message.contains("it has no default export")
        }),
        "{:#?}",
        output.warnings
    );

    // Without the interop the module reads the namespace as it read the
    // exports object: no warning.
    let direct = source.replace(
        "var n=o(2),a=o.n(n);e.run=function(){return a().k+",
        "var n=o(2);e.run=function(){return n.default.k+",
    );
    let output = unpack(&direct, DecompileOptions::default()).expect("bundle should unpack");
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm),
        "{:#?}",
        output.warnings
    );
}

/// A webpack 5 runtime around `modules`, entering module 1; module 1 keeps a
/// require inside a function, so it stays CommonJS.
fn kept_require_bundle(modules: &str) -> String {
    format!(
        r#"(()=>{{var t={{{modules},3(t,e){{e.x=1}}}};const e={{}};globalThis.lib=function o(r){{const l=e[r];if(void 0!==l)return l.exports;const i=e[r]={{exports:{{}}}};return t[r](i,i.exports,o),i.exports}}(1)}})();"#
    )
}

fn unpacked_module(
    output: &wakaru_core::driver::test_support::UnpackOutput,
    filename: &str,
) -> String {
    output
        .modules
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| code.clone())
        .expect("module should exist")
}

#[test]
fn kept_require_of_a_function_with_named_exports_reads_default_per_use() {
    // hypothetical: `module.exports = fn` plus `module.exports.flag = true`
    // becomes a default and a named export.
    let source = kept_require_bundle(
        r#"1(t,e,o){var i=o(2);e.run=function(){return i({a:1})+i.flag+i.name+o(3).x}},2(t){t.exports=function(e){return "plugin"},t.exports.flag=!0}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = unpacked_module(&output, "module-1.js");
    for present in ["i.default({", "i.flag", "i.default.name"] {
        assert!(
            consumer.contains(present),
            "missing `{present}`:\n{consumer}"
        );
    }
    assert!(output.warnings.is_empty(), "{:#?}", output.warnings);

    // The value escapes: no read of it is right, so it is reported.
    let escaped = kept_require_bundle(
        r#"1(t,e,o){var i=o(2);e.run=function(){return[i,o(3).x]}},2(t){t.exports=function(e){return "plugin"},t.exports.flag=!0}"#,
    );
    let output = unpack(&escaped, DecompileOptions::default()).expect("bundle should unpack");
    assert!(
        output.warnings.iter().any(|w| {
            w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm
                && w.message.contains("beside its named exports")
        }),
        "{:#?}",
        output.warnings
    );
}

#[test]
fn kept_require_of_a_function_with_named_exports_reports_uses_it_cannot_redirect() {
    // hypothetical: each consumer use below keeps the namespace after the
    // `.default` rewrite, so the binding is left alone and reported.
    let provider = r#"2(t){var f=function(){return 1};f.n=3,t.exports=f,t.exports.size=function(){return this.n},t.exports.flag=!0}"#;
    for consumer in [
        // An optional member reads the namespace.
        r#"1(t,e,o){var i=o(2);e.run=function(){return i?.name+i()+o(3).x}}"#,
        // `this` of a named export was the value, now the namespace.
        r#"1(t,e,o){var i=o(2);e.run=function(){return i.size()+i()+o(3).x}}"#,
        // A second `var` writes the binding.
        r#"1(t,e,o){var i=o(2);i();var i=o(3);e.run=function(){return i.x+o(3).x}}"#,
    ] {
        let source = kept_require_bundle(&format!("{consumer},{provider}"));
        let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
        let module = unpacked_module(&output, "module-1.js");
        assert!(!module.contains("i.default"), "{module}");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm),
            "{module}\n{:#?}",
            output.warnings
        );
    }

    // A write would land on the namespace instead of the value, but the
    // write keeps the provider CommonJS, so nothing is left to report.
    let source = kept_require_bundle(&format!(
        r#"1(t,e,o){{var i=o(2);e.run=function(){{return i.count=1,i()+o(3).x}}}},{provider}"#
    ));
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    assert!(unpacked_module(&output, "module-2.js").contains("module.exports"));
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm),
        "{:#?}",
        output.warnings
    );

    // The same call without a binding is reported too.
    let source = kept_require_bundle(&format!(
        r#"1(t,e,o){{e.run=function(){{return o(2).size()+o(3).x}}}},{provider}"#
    ));
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    assert!(
        output.warnings.iter().any(|w| {
            w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm
                && w.message.contains("as `this`")
        }),
        "{:#?}",
        output.warnings
    );
}

/// A webpack 5 bundle whose module 1 imports module 3 through the
/// passthrough module 2, `module.exports = require("./module-3.js")`.
fn passthrough_bundle(consumer: &str, provider: &str) -> String {
    format!(
        r#"(()=>{{var t={{1(t,e,o){{{consumer}}},2(t,e,o){{"use strict";t.exports=o(3)}},3(t,e,o){{{provider}}}}};const e={{}};function o(r){{const l=e[r];if(void 0!==l)return l.exports;const i=e[r]={{exports:{{}}}};return t[r](i,i.exports,o),i.exports}}o.r=t=>{{Object.defineProperty(t,"__esModule",{{value:!0}})}},o.d=(t,e)=>{{for(var r in e)o.o(e,r)&&!o.o(t,r)&&Object.defineProperty(t,r,{{enumerable:!0,get:e[r]}})}},o.o=(t,e)=>Object.prototype.hasOwnProperty.call(t,e),globalThis.lib=o(1)}})();"#
    )
}

#[test]
fn destructured_import_through_a_passthrough_reads_the_esm_provider() {
    // wild-observed, producer unknown: an ES module destructures a named
    // export of an ES module it imports through a CommonJS passthrough.
    let source = passthrough_bundle(
        r#""use strict";o.r(e),o.d(e,{run:()=>r});var z=o(2);let{sel:j}=z;function r(){return j(1)}"#,
        r#""use strict";o.r(e),o.d(e,{sel:()=>s});function s(x){return x}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = unpacked_module(&output, "module-1.js");
    assert!(
        consumer.contains(r#"import * as z from "./module-3.js""#),
        "{consumer}"
    );
}

#[test]
fn import_through_a_passthrough_of_a_function_provider_reads_its_default() {
    // hypothetical: module 3 was `module.exports = fn` with a property on
    // `fn`, which the namespace of its ES module form does not have.
    let source = passthrough_bundle(
        r#""use strict";o.r(e),o.d(e,{run:()=>r});var z=o(2);function r(){return z.version}"#,
        r#"function s(x){return x}s.version="1";t.exports=s"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = unpacked_module(&output, "module-1.js");
    assert!(
        consumer.contains(r#"import z from "./module-3.js""#),
        "{consumer}"
    );
    let provider = unpacked_module(&output, "module-3.js");
    assert!(provider.contains("export default s"), "{provider}");
}

/// Module 3 of [`passthrough_bundle`] with Next's default-object tail
/// (producer next@16.3.8 dist, minified), defining `default` as `value`.
fn next_tail_provider(value: &str) -> String {
    format!(
        r#""use strict";Object.defineProperty(e,"__esModule",{{value:!0}}),Object.defineProperty(e,"default",{{enumerable:!0,get:function(){{return s}}}});{value}("function"==typeof e.default||"object"==typeof e.default&&null!==e.default)&&void 0===e.default.__esModule&&(Object.defineProperty(e.default,"__esModule",{{value:!0}}),Object.assign(e.default,e),t.exports=e.default)"#
    )
}

#[test]
fn import_through_a_passthrough_of_a_next_tail_provider_reads_its_default() {
    // The tail made the provider's CommonJS value its default; a call result
    // is assumed a function or object (`default_object_tail_value`). The
    // consumer's `require.n` interop became a default import.
    let source = passthrough_bundle(
        r#""use strict";o.r(e),o.d(e,{run:()=>r});var z=o(2),a=o.n(z);function r(){return a()()}"#,
        &next_tail_provider(r#"function m(f){return f}var s=m(function(){return"rendered"});"#),
    )
    .replace(
        "o.r=t=>",
        "o.n=t=>{var e=t&&t.__esModule?()=>t.default:()=>t;return o.d(e,{a:e}),e},o.r=t=>",
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let passthrough = unpacked_module(&output, "module-2.js");
    assert!(
        passthrough.contains(r#"module.exports = require("./module-3.js").default"#),
        "{passthrough}"
    );
    let consumer = unpacked_module(&output, "module-1.js");
    assert!(
        consumer.contains(r#"import z from "./module-3.js""#),
        "{consumer}"
    );
}

#[test]
fn passthrough_of_a_next_tail_provider_with_a_primitive_default_keeps_the_whole_require() {
    // A string default fails the tail's test, so `module.exports` stays the
    // exports object, which the namespace stands for.
    let source = passthrough_bundle(
        r#""use strict";o.r(e),o.d(e,{run:()=>r});var z=o(2);function r(){return z.default}"#,
        &next_tail_provider(r#"var s="label";"#),
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let passthrough = unpacked_module(&output, "module-2.js");
    assert!(
        passthrough.contains(r#"module.exports = require("./module-3.js");"#),
        "{passthrough}"
    );
}

#[test]
fn es_source_module_kept_commonjs_is_reported_only_under_diagnostics() {
    // hypothetical: module 1 is webpack harmony output (`o.r`, `o.d`) whose
    // global write is a barrier, so its top-level load stays a call and the
    // module stays CommonJS.
    let source = r#"(()=>{var t={1(t,e,o){"use strict";o.r(e),o.d(e,{f:()=>f});globalThis.flag=1;var d=o(3);function f(){return d.x}},3(t,e){e.x=1}};const e={};function o(r){const l=e[r];if(void 0!==l)return l.exports;const i=e[r]={exports:{}};return t[r](i,i.exports,o),i.exports}o.r=t=>{Object.defineProperty(t,"__esModule",{value:!0})},o.d=(t,e)=>{for(var r in e)o.o(e,r)&&!o.o(t,r)&&Object.defineProperty(t,r,{enumerable:!0,get:e[r]})},o.o=(t,e)=>Object.prototype.hasOwnProperty.call(t,e),globalThis.lib=o(1)})();"#;
    let is_kept = |w: &&wakaru_core::UnpackWarning| {
        w.kind == wakaru_core::UnpackWarningKind::EsSourceKeptCommonJs
    };

    let output = unpack(
        source,
        DecompileOptions {
            diagnostics: true,
            ..DecompileOptions::default()
        },
    )
    .expect("bundle should unpack");
    let module = unpacked_module(&output, "module-1.js");
    assert!(!module.contains("import "), "{module}");
    let warnings = output.warnings.iter().filter(is_kept).collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1, "{:#?}", output.warnings);
    assert_eq!(warnings[0].filename, "module-1.js");
    assert!(
        warnings[0]
            .message
            .contains(r#""./module-3.js" (top-level)"#),
        "{}",
        warnings[0].message
    );

    let output = unpack(source, DecompileOptions::default()).expect("bundle should unpack");
    assert!(
        !output.warnings.iter().any(|w| is_kept(&w)),
        "{:#?}",
        output.warnings
    );
}

#[test]
fn es_source_module_with_only_lazy_requires_is_not_reported() {
    // hypothetical: a require in a function was written by the source.
    let source = r#"(()=>{var t={1(t,e,o){"use strict";o.r(e),o.d(e,{f:()=>f});function f(){return o(3).x}},3(t,e){e.x=1}};const e={};function o(r){const l=e[r];if(void 0!==l)return l.exports;const i=e[r]={exports:{}};return t[r](i,i.exports,o),i.exports}o.r=t=>{Object.defineProperty(t,"__esModule",{value:!0})},o.d=(t,e)=>{for(var r in e)o.o(e,r)&&!o.o(t,r)&&Object.defineProperty(t,r,{enumerable:!0,get:e[r]})},o.o=(t,e)=>Object.prototype.hasOwnProperty.call(t,e),globalThis.lib=o(1)})();"#;
    let output = unpack(
        source,
        DecompileOptions {
            diagnostics: true,
            ..DecompileOptions::default()
        },
    )
    .expect("bundle should unpack");
    let module = unpacked_module(&output, "module-1.js");
    assert!(module.contains(r#"require("./module-3.js")"#), "{module}");
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.kind == wakaru_core::UnpackWarningKind::EsSourceKeptCommonJs),
        "{:#?}",
        output.warnings
    );
}

#[test]
fn imported_call_of_a_require_in_a_function_keeps_the_module_commonjs() {
    // hypothetical: the require runs only when the function is called.
    let source = kept_require_bundle(
        r#"1(t,e,o){var h=o(2);e.y=function(){return h.wrap(o(3))}},2(t,e){e.wrap=function(x){return x}}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let module = unpacked_module(&output, "module-1.js");
    assert!(!module.contains("import "), "{module}");
    assert!(module.contains(r#"require("./module-3.js")"#), "{module}");
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireUnrecovered),
        "{:#?}",
        output.warnings
    );
}

#[test]
fn unproven_imported_call_of_a_require_imports_its_provider() {
    // hypothetical: an imported function that is no interop helper, called on
    // a require as an export's value or a binding's initializer. The call
    // stays; its provider becomes an import, a namespace for one with named
    // exports and a default import for `module.exports = v`.
    for (consumer, import) in [
        (
            r#"1(t,e,o){var h=o(2);e.y=h.wrap(o(3))}"#,
            r#"import * as module_3_1 from "./module-3.js";"#,
        ),
        (
            r#"1(t,e,o){var h=o(2),x=h.wrap(o(4));e.y=function(){return x}},4(t){t.exports={v:1}}"#,
            r#"import module_4 from "./module-4.js";"#,
        ),
    ] {
        let source = kept_require_bundle(&format!(
            r#"{consumer},2(t,e){{e.wrap=function(x){{return x}}}}"#
        ));
        let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
        let module = unpacked_module(&output, "module-1.js");
        assert!(module.contains(import), "{module}");
        assert!(module.contains("wrap(module_"), "{module}");
        assert!(!module.contains("require("), "{module}");
        assert!(
            !output
                .warnings
                .iter()
                .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireUnrecovered),
            "{:#?}",
            output.warnings
        );
        let findings = validate_output_modules(&output.modules);
        assert!(findings.is_empty(), "{findings:#?}");
    }
}

#[test]
fn imported_call_of_a_marked_provider_now_esm_keeps_what_the_marker_meant() {
    // shape: producer @babel/runtime interop, webpack@4.47.0 + UglifyJS
    // 3.14. The provider marked itself `__esModule` and becomes an ES
    // module, whose namespace has no marker: a proven interop helper
    // returned it unchanged, so the call goes, and `UnEsm` proves the
    // bundled helper at barrier 0 and reads `r.default` as the default
    // import; for any other function the marker may matter, so the require
    // stays.
    let marked = r#"4(t,e){"use strict";Object.defineProperty(e,"__esModule",{value:!0}),e.default=function(x){return x+1}}"#;
    let interop = r#"2(t){t.exports=function(e){return e&&e.__esModule?e:{default:e}},t.exports.__esModule=!0,t.exports.default=t.exports}"#;
    let source = kept_require_bundle(&format!(
        r#"1(t,e,o){{var n=o(2),r=n(o(4));r;e.y=function(){{return r.default(1)}}}},{interop},{marked}"#
    ));
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let module = unpacked_module(&output, "module-1.js");
    assert!(
        module.contains(r#"import r from "./module-4.js";"#),
        "{module}"
    );
    assert!(module.contains("return r(1)"), "{module}");
    assert!(!module.contains("require("), "{module}");
    let findings = validate_output_modules(&output.modules);
    assert!(findings.is_empty(), "{findings:#?}");

    let source = kept_require_bundle(&format!(
        r#"1(t,e,o){{var h=o(2),r=h.wrap(o(4));e.y=function(){{return r}}}},2(t,e){{e.wrap=function(x){{return x}}}},{marked}"#
    ));
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let module = unpacked_module(&output, "module-1.js");
    assert!(
        module.contains(r#"wrap(require("./module-4.js"))"#),
        "{module}"
    );
}

#[test]
fn imported_call_of_a_require_after_a_hoisting_barrier_keeps_the_module_commonjs() {
    // hypothetical: the global write must run before the provider loads,
    // which an import would load first.
    let source = kept_require_bundle(
        r#"1(t,e,o){var h=o(2);globalThis.flag=1;var x=h.wrap(o(3));e.y=function(){return x}},2(t,e){e.wrap=function(x){return x}}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let module = unpacked_module(&output, "module-1.js");
    assert!(!module.contains("import "), "{module}");
    let flag = module.find("globalThis.flag").expect("global write");
    let load = module
        .find(r#"require("./module-3.js")"#)
        .expect("load stays in place");
    assert!(flag < load, "{module}");
}

#[test]
fn interop_helper_call_terser_inlined_into_a_class_argument_becomes_an_import() {
    // shape: producer @babel/core@7.18.13 + plugin-transform-runtime@7.18.10
    // + plugin-transform-classes@7.18.9 + plugin-transform-modules-commonjs
    // @7.18.6, bundled with @babel/runtime@7.28.6 by webpack@5.111.1
    // production (Terser); wild-observed. Terser inlines the single-use
    // `wrapNativeSuper` binding into the class IIFE's argument,
    // `(0,o(r(837)).default)(Error)`. Sink renamed to `globalThis.lib`.
    let source = r#"(()=>{var e={97(e,t,r){const o=r(618);t.main=function(){const e=new o.AppError("entry-side");return[o.describe(),e instanceof Error,e instanceof o.AppError,e.toJSON().message]}},618(e,t,r){"use strict";var o=r(994);t.AppError=void 0,t.describe=function(){const e=new f("errors-side");return[e instanceof Error,e instanceof f,e.name,e.toJSON().message]};var n=o(r(383)),u=o(r(579)),s=o(r(511)),p=o(r(452)),i=o(r(72));let f=function(e){(0,s.default)(f,e);var t,r,o=(t=f,r=function(){if("undefined"==typeof Reflect||!Reflect.construct)return!1;if(Reflect.construct.sham)return!1;if("function"==typeof Proxy)return!0;try{return Boolean.prototype.valueOf.call(Reflect.construct(Boolean,[],function(){})),!0}catch(e){return!1}}(),function(){var e,o=(0,i.default)(t);if(r){var n=(0,i.default)(this).constructor;e=Reflect.construct(o,arguments,n)}else e=o.apply(this,arguments);return(0,p.default)(this,e)});function f(e){var t;return(0,n.default)(this,f),(t=o.call(this,e)).name="inlined-super-error",t}return(0,u.default)(f,[{key:"toJSON",value:function(){return{name:this.name,message:"["+this.message+"]"}}}]),f}((0,o(r(837)).default)(Error));t.AppError=f},475(e){e.exports=function(e){if(void 0===e)throw new ReferenceError("this hasn't been initialised - super() hasn't been called");return e},e.exports.__esModule=!0,e.exports.default=e.exports},383(e){e.exports=function(e,t){if(!(e instanceof t))throw new TypeError("Cannot call a class as a function")},e.exports.__esModule=!0,e.exports.default=e.exports},646(e,t,r){var o=r(550),n=r(636);e.exports=function(e,t,r){if(o())return Reflect.construct.apply(null,arguments);var u=[null];u.push.apply(u,t);var s=new(e.bind.apply(e,u));return r&&n(s,r.prototype),s},e.exports.__esModule=!0,e.exports.default=e.exports},579(e,t,r){var o=r(736);function n(e,t){for(var r=0;r<t.length;r++){var n=t[r];n.enumerable=n.enumerable||!1,n.configurable=!0,"value"in n&&(n.writable=!0),Object.defineProperty(e,o(n.key),n)}}e.exports=function(e,t,r){return t&&n(e.prototype,t),r&&n(e,r),Object.defineProperty(e,"prototype",{writable:!1}),e},e.exports.__esModule=!0,e.exports.default=e.exports},72(e){function t(r){return e.exports=t=Object.setPrototypeOf?Object.getPrototypeOf.bind():function(e){return e.__proto__||Object.getPrototypeOf(e)},e.exports.__esModule=!0,e.exports.default=e.exports,t(r)}e.exports=t,e.exports.__esModule=!0,e.exports.default=e.exports},511(e,t,r){var o=r(636);e.exports=function(e,t){if("function"!=typeof t&&null!==t)throw new TypeError("Super expression must either be null or a function");e.prototype=Object.create(t&&t.prototype,{constructor:{value:e,writable:!0,configurable:!0}}),Object.defineProperty(e,"prototype",{writable:!1}),t&&o(e,t)},e.exports.__esModule=!0,e.exports.default=e.exports},994(e){e.exports=function(e){return e&&e.__esModule?e:{default:e}},e.exports.__esModule=!0,e.exports.default=e.exports},691(e){e.exports=function(e){try{return-1!==Function.toString.call(e).indexOf("[native code]")}catch(t){return"function"==typeof e}},e.exports.__esModule=!0,e.exports.default=e.exports},550(e){function t(){try{var r=!Boolean.prototype.valueOf.call(Reflect.construct(Boolean,[],function(){}))}catch(r){}return(e.exports=t=function(){return!!r},e.exports.__esModule=!0,e.exports.default=e.exports)()}e.exports=t,e.exports.__esModule=!0,e.exports.default=e.exports},452(e,t,r){var o=r(738).default,n=r(475);e.exports=function(e,t){if(t&&("object"==o(t)||"function"==typeof t))return t;if(void 0!==t)throw new TypeError("Derived constructors may only return object or undefined");return n(e)},e.exports.__esModule=!0,e.exports.default=e.exports},636(e){function t(r,o){return e.exports=t=Object.setPrototypeOf?Object.setPrototypeOf.bind():function(e,t){return e.__proto__=t,e},e.exports.__esModule=!0,e.exports.default=e.exports,t(r,o)}e.exports=t,e.exports.__esModule=!0,e.exports.default=e.exports},45(e,t,r){var o=r(738).default;e.exports=function(e,t){if("object"!=o(e)||!e)return e;var r=e[Symbol.toPrimitive];if(void 0!==r){var n=r.call(e,t||"default");if("object"!=o(n))return n;throw new TypeError("@@toPrimitive must return a primitive value.")}return("string"===t?String:Number)(e)},e.exports.__esModule=!0,e.exports.default=e.exports},736(e,t,r){var o=r(738).default,n=r(45);e.exports=function(e){var t=n(e,"string");return"symbol"==o(t)?t:t+""},e.exports.__esModule=!0,e.exports.default=e.exports},738(e){function t(r){return e.exports=t="function"==typeof Symbol&&"symbol"==typeof Symbol.iterator?function(e){return typeof e}:function(e){return e&&"function"==typeof Symbol&&e.constructor===Symbol&&e!==Symbol.prototype?"symbol":typeof e},e.exports.__esModule=!0,e.exports.default=e.exports,t(r)}e.exports=t,e.exports.__esModule=!0,e.exports.default=e.exports},837(e,t,r){var o=r(72),n=r(636),u=r(691),s=r(646);function p(t){var r="function"==typeof Map?new Map:void 0;return e.exports=p=function(e){if(null===e||!u(e))return e;if("function"!=typeof e)throw new TypeError("Super expression must either be null or a function");if(void 0!==r){if(r.has(e))return r.get(e);r.set(e,t)}function t(){return s(e,arguments,o(this).constructor)}return t.prototype=Object.create(e.prototype,{constructor:{value:t,enumerable:!1,writable:!0,configurable:!0}}),n(t,e)},e.exports.__esModule=!0,e.exports.default=e.exports,p(t)}e.exports=p,e.exports.__esModule=!0,e.exports.default=e.exports}};const t={};globalThis.lib=function r(o){const n=t[o];if(void 0!==n)return n.exports;const u=t[o]={exports:{}};return e[o](u,u.exports,r),u.exports}(97)})();"#;
    let output = unpack(source, DecompileOptions::default()).expect("bundle should unpack");
    let findings = validate_output_modules(&output.modules);
    assert!(findings.is_empty(), "{findings:#?}");
    let consumer = unpacked_module(&output, "module-618.js");
    assert!(!consumer.contains("require("), "{consumer}");
    assert!(
        consumer.contains(r#"from "./module-837.js";"#),
        "{consumer}"
    );
    assert!(consumer.contains("export let AppError"), "{consumer}");
    assert!(!consumer.contains(".default"), "{consumer}");
}

#[test]
fn kept_require_default_read_of_a_self_referencing_marked_provider_stays_one_read() {
    // shape: the CommonJS shim of an `@babel/runtime` helper,
    // `module.exports = fn` with `__esModule` and a `default` that is
    // `module.exports` itself; it is recovered as `export default fn`. The
    // consumer's `.default` already meant that default.
    let source = kept_require_bundle(
        r#"1(t,e,o){var h=o(2).default;e.run=function(){return h(5)+o(3).x}},2(t){t.exports=function(e){return e},t.exports.__esModule=!0,t.exports.default=t.exports}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = unpacked_module(&output, "module-1.js");
    assert!(
        consumer.contains(r#"require("./module-2.js").default;"#),
        "{consumer}"
    );
    assert!(!consumer.contains(".default.default"), "{consumer}");
}

#[test]
fn kept_require_of_a_marked_provider_drops_the_local_interop() {
    // hypothetical: Babel's default interop returned the marked provider
    // unchanged; its namespace now has no marker to test.
    let source = kept_require_bundle(
        r#"1(t,e,o){function Z(e){return e&&e.__esModule?e:{default:e}}var Q=Z(o(2));e.run=function(){return Q.default+o(3).x}},2(t,e){"use strict";Object.defineProperty(e,"__esModule",{value:!0}),e.default="d"}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = unpacked_module(&output, "module-1.js");
    assert!(
        consumer.contains(r#"const Q = require("./module-2.js");"#),
        "{consumer}"
    );
    assert!(output.warnings.is_empty(), "{:#?}", output.warnings);
}

#[test]
fn kept_require_of_an_amd_factory_result_reads_its_default() {
    // The webpack AMD-return form the CommonJS runtime normalizer turns into
    // `export default _webpackDefault`.
    let source = kept_require_bundle(
        r#"1(t,e,o){var i=o(2);e.run=function(){return i()+o(3).x}},2(t,e,o){var r;!function(){var n=function(){return"amd"};void 0===(r=function(){return n}.apply(e,[]))||(t.exports=r)}()}"#,
    );
    let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
    let consumer = unpacked_module(&output, "module-1.js");
    assert!(
        consumer.contains(r#"require("./module-2.js").default"#),
        "{consumer}"
    );
    assert!(unpacked_module(&output, "module-2.js").contains("export default"));
}

#[test]
fn es_module_consumer_writing_a_named_only_provider_keeps_it_commonjs() {
    // hypothetical: module 2 writes to the exports object of module 1, which
    // writes named exports only. An ES module has no such object, so module
    // 1 stays CommonJS, and module 2's default import is its
    // `module.exports`.
    let source = r#"(()=>{var e={1:(e,t,r)=>{t.value=1},2:(e,t,r)=>{var p=r(1);p.value=2;console.log(p.value)}},t={};function r(o){var n=t[o];if(void 0!==n)return n.exports;var s=t[o]={exports:{}};return e[o](s,s.exports,r),s.exports}r(2)})();"#;
    // The same write to a provider whose only export is
    // `module.exports = {…}` lands on that object, which `export default`
    // still holds.
    let whole_value = source.replace("t.value=1", "e.exports={value:1}");
    for (source, kept) in [(source.to_string(), true), (whole_value, false)] {
        let output = unpack(&source, DecompileOptions::default()).expect("bundle should unpack");
        assert!(
            !output
                .warnings
                .iter()
                .any(|w| w.kind == wakaru_core::UnpackWarningKind::MissingImportedName),
            "{:#?}",
            output.warnings
        );
        let provider = unpacked_module(&output, "module-1.js");
        assert_eq!(!provider.contains("export "), kept, "{provider}");
        let consumer = unpacked_module(&output, "module-2.js");
        assert!(
            consumer.contains("import p from \"./module-1.js\""),
            "{consumer}"
        );
        assert!(consumer.contains("p.value = 2"), "{consumer}");
    }
}

#[test]
fn commonjs_consumer_writing_a_named_only_provider_keeps_it_commonjs() {
    // Module 2 keeps a require in a function, so it stays CommonJS, and
    // writes to module 1, which writes named exports only: module 1 stays
    // CommonJS, so `require` still returns the object the write lands on.
    // The direct `r(1).x = v` form is what Terser leaves of a single-use
    // binding (producer webpack@5.111.1 + Terser).
    let source = r#"(()=>{var e={1:(e,t,r)=>{t.value=1},2:(e,t,r)=>{var p=r(1);p.extra=2;t.run=function(){return r(3)}},3:(e,t,r)=>{t.x=1},4:(e,t,r)=>{"use strict";r(1).value=3;t.run=function(){return r(3)}}},t={};function r(o){var n=t[o];if(void 0!==n)return n.exports;var s=t[o]={exports:{}};return e[o](s,s.exports,r),s.exports}globalThis.a=r(2),globalThis.b=r(4)})();"#;
    let output = unpack(source, DecompileOptions::default()).expect("bundle should unpack");
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.kind == wakaru_core::UnpackWarningKind::CommonJsRequireOfEsm),
        "{:#?}",
        output.warnings
    );
    let provider = unpacked_module(&output, "module-1.js");
    assert!(provider.contains("exports.value = 1"), "{provider}");
}

#[test]
fn a_default_read_of_a_marked_provider_that_may_replace_its_exports_reads_the_default_export() {
    // Next's default-object tail replaces `module.exports` with the default
    // only when that is an object or a function; with a primitive default,
    // `require(p).default` reads the `exports` object's default getter. The
    // consumer is a webpack harmony module reading a flagged provider's
    // `.default` without interop (hypothetical).
    let source = r#"
(() => {
  var __webpack_modules__ = ({
    100: ((module, exports, __webpack_require__) => {
      "use strict";
      __webpack_require__.r(exports);
      __webpack_require__.d(exports, { run: () => run });
      var label = __webpack_require__(300);
      function run() { return label.default; }
    }),
    300: ((module, exports) => {
      "use strict";
      Object.defineProperty(exports, "__esModule", { value: true });
      Object.defineProperty(exports, "default", { enumerable: true, get: function () { return value; } });
      var value = "label";
      if ((typeof exports.default === "function" || (typeof exports.default === "object" && exports.default !== null)) && typeof exports.default.__esModule === "undefined") {
        Object.defineProperty(exports.default, "__esModule", { value: true });
        Object.assign(exports.default, exports);
        module.exports = exports.default;
      }
    })
  });
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cached = __webpack_module_cache__[moduleId];
    if (cached !== undefined) return cached.exports;
    var module = __webpack_module_cache__[moduleId] = { exports: {} };
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.r = (e) => { Object.defineProperty(e, "__esModule", { value: true }); };
  __webpack_require__.d = (e, d) => { for (var k in d) Object.defineProperty(e, k, { enumerable: true, get: d[k] }); };
  globalThis.lib = __webpack_require__(100);
})();
"#;
    let output = unpack(source, DecompileOptions::default()).expect("the bundle should unpack");
    let consumer = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-100.js")
        .map(|(_, code)| code.as_str())
        .expect("expected the consumer");
    assert!(
        consumer.contains(r#"import label from "./module-300.js";"#)
            && consumer.contains("return label;"),
        "{consumer}"
    );
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}

#[test]
fn concatenated_scripts_with_repeated_babel_helpers_split_into_valid_modules() {
    // shape: producer babel@7.29 preset-env ie11 `sourceType: "script"` run per
    // file, then terser@5.51 given both files (`terser a.js b.js`): each
    // script carries its own inline helpers, so the joined Script declares
    // every helper twice. The copies share one binding; the later body wins.
    let helpers = r#"function _typeof(e){return _typeof="function"==typeof Symbol&&"symbol"==typeof Symbol.iterator?function(e){return typeof e}:function(e){return e&&"function"==typeof Symbol&&e.constructor===Symbol&&e!==Symbol.prototype?"symbol":typeof e},_typeof(e)}function _classCallCheck(e,t){if(!(e instanceof t))throw new TypeError("Cannot call a class as a function")}function _defineProperties(e,t){for(var r=0;r<t.length;r++){var o=t[r];o.enumerable=o.enumerable||!1,o.configurable=!0,"value"in o&&(o.writable=!0),Object.defineProperty(e,_toPropertyKey(o.key),o)}}function _createClass(e,t,r){return t&&_defineProperties(e.prototype,t),r&&_defineProperties(e,r),Object.defineProperty(e,"prototype",{writable:!1}),e}function _toPropertyKey(e){var t=_toPrimitive(e,"string");return"symbol"==_typeof(t)?t:t+""}function _toPrimitive(e,t){if("object"!=_typeof(e)||!e)return e;var r=e[Symbol.toPrimitive];if(void 0!==r){var o=r.call(e,t||"default");if("object"!=_typeof(o))return o;throw new TypeError("@@toPrimitive must return a primitive value.")}return("string"===t?String:Number)(e)}"#;
    let source = format!(
        r#"{helpers}var DEFAULTS={{retries:2}};function Config(e){{this.options=Object.assign({{}},DEFAULTS,e)}}var Registry=function(){{"use strict";return _createClass(function e(){{_classCallCheck(this,e),this.items=[]}},[{{key:"add",value:function(e){{return this.items.push(e),this}}}},{{key:"size",get:function(){{return this.items.length}}}}])}}();window.makeConfig=function(e){{return new Config(e).options}},window.configRegistry=new Registry;{helpers}var BASE={{color:"red"}};var Theme=function(){{"use strict";return _createClass(function e(t){{_classCallCheck(this,e),this.values=Object.assign({{}},BASE,t)}},[{{key:"label",get:function(){{return this.values.color}}}}])}}();window.makeTheme=function(e){{return new Theme(e).label}};"#
    );

    let output = unpack(
        &source,
        DecompileOptions {
            filename: "pages.js".to_string(),
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("the concatenated scripts should unpack");

    assert!(
        output.modules.len() >= 2,
        "the scope-hoisted split should still apply: {:?}",
        module_names(&output.modules)
    );
    for helper in ["_defineProperties", "_createClass", "_toPrimitive"] {
        let declarations = output
            .modules
            .iter()
            .map(|(_, code)| code.matches(&format!("function {helper}(")).count())
            .sum::<usize>();
        assert_eq!(
            declarations, 1,
            "`{helper}` must keep one declaration:\n{:#?}",
            output.modules
        );
    }
    assert_eq!(validate_output_modules(&output.modules), vec![]);
}
