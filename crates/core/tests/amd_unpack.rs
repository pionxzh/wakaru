use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, DecompileOptions};

fn raw_pairs(source: &str) -> Vec<(String, String)> {
    unpack_raw(
        source,
        &DecompileOptions {
            filename: "amd.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should succeed")
    .modules
}

fn pairs(source: &str) -> Vec<(String, String)> {
    unpack(
        source,
        DecompileOptions {
            filename: "amd.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed")
    .modules
}

#[test]
fn amd_named_define_modules_unpack() {
    let source = r#"
define("utils/math", [], function() {
  function add(a, b) {
    return a + b;
  }
  return { add: add };
});

define("app/main", ["utils/math"], function(math) {
  console.log(math.add(1, 2));
});
"#;

    let raw = raw_pairs(source);
    let names: Vec<&str> = raw.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["utils/math.js", "app/main.js"]);

    let main = raw
        .iter()
        .find(|(name, _)| name == "app/main.js")
        .map(|(_, code)| code)
        .expect("expected main module");
    assert!(
        main.contains(r#"var math = require("../utils/math.js");"#),
        "dependency should become a relative require:\n{main}"
    );

    let decompiled = pairs(source);
    let main = decompiled
        .iter()
        .find(|(name, _)| name == "app/main.js")
        .map(|(_, code)| code)
        .expect("expected decompiled main module");
    assert!(
        main.contains("import ") && main.contains(r#""../utils/math.js""#),
        "decompile pipeline should recover an import:\n{main}"
    );
}

#[test]
fn amd_define_with_exports_dependency_unpack() {
    let source = r#"
define("counter", ["exports"], function(exports) {
  exports.next = function(value) {
    return value + 1;
  };
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "counter.js");
    assert!(
        raw[0].1.contains("exports.next = function"),
        "exports dependency should remain as CommonJS-style exports:\n{}",
        raw[0].1
    );
}

#[test]
fn anonymous_amd_define_unpack() {
    let source = r#"
define(["./dep"], function(dep) {
  return dep.value + 1;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains(r#"var dep = require("./dep.js");"#)
            && raw[0].1.contains("module.exports = dep.value + 1;"),
        "anonymous AMD module should become a single CommonJS module:\n{}",
        raw[0].1
    );
}

#[test]
fn terminal_return_before_hoist_only_vars_becomes_module_export() {
    let source = r#"
define([], function() {
  return buildLibrary();
  var internalState, cachedValue;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert!(
        raw[0].1.contains("module.exports = buildLibrary();")
            && raw[0].1.contains("var internalState, cachedValue;")
            && !raw[0].1.contains("return buildLibrary();"),
        "a return followed only by hoist-only vars is still terminal:\n{}",
        raw[0].1
    );
}

#[test]
fn terminal_bare_return_before_hoist_only_vars_is_removed_safely() {
    let source = r#"
define([], function() {
  initializeLibrary();
  return;
  var internalState, cachedValue;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert!(
        raw[0].1.contains("initializeLibrary();")
            && raw[0].1.contains("var internalState, cachedValue;")
            && !raw[0].1.contains("return;"),
        "a terminal bare return may be removed without executing later initializers:\n{}",
        raw[0].1
    );
}

#[test]
fn return_before_initialized_var_is_not_treated_as_terminal() {
    let source = r#"
define([], function() {
  return buildLibrary();
  var unexpectedEffect = initialize();
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert!(
        !raw[0].1.contains("define(")
            && raw[0].1.contains("return buildLibrary();")
            && raw[0].1.contains("unexpectedEffect = initialize()"),
        "the AMD detector must restore a callable factory boundary when a return cannot be lifted:\n{}",
        raw[0].1
    );
    assert_eq!(validate_output_modules(&raw), vec![]);
}

#[test]
fn early_bare_return_keeps_the_amd_factory_boundary() {
    let source = r#"
define(["polyfill/forEach"], function() {
  function deferImages() {
    document.querySelectorAll("img[data-src]").forEach(loadImage);
  }
  if (typeof IntersectionObserver === "undefined") {
    setTimeout(deferImages, 0);
    return;
  }
  const observer = new IntersectionObserver(deferImages);
  observer.observe(document.body);
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert!(
        !raw[0].1.contains("define(")
            && raw[0].1.contains("return;")
            && raw[0].1.contains("IntersectionObserver"),
        "the lifted AMD body should regain a function boundary without losing extraction:\n{}",
        raw[0].1
    );
    assert_eq!(validate_output_modules(&raw), vec![]);
}

/// A factory observing its own `arguments` cannot be lifted: an arrow
/// boundary has no `arguments`, so the restored module would parse but throw
/// at import time. The whole bundle must fall back to the original source.
#[test]
fn factory_observing_arguments_rejects_the_bundle() {
    let source = r#"
define("app/main", ["app/dep"], function(dep) {
  if (arguments.length === 0) return;
  console.log(arguments[0], dep);
});
define("app/dep", [], function() {
  return 42;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(
        raw.len(),
        1,
        "the bundle must fall back as one file: {raw:#?}"
    );
    assert!(
        raw[0].1.contains("define(\"app/main\""),
        "the original define calls must be preserved:\n{}",
        raw[0].1
    );

    let decompiled = pairs(source);
    assert_eq!(decompiled.len(), 1);
    assert!(
        decompiled[0].1.contains("define(") && !decompiled[0].1.contains("(()=>"),
        "no arrow boundary may capture the factory's arguments:\n{}",
        decompiled[0].1
    );
}

/// `new.target` is bound by the AMD factory just like `this` and
/// `arguments`. Lifting it to module scope would emit invalid JavaScript.
#[test]
fn factory_observing_new_target_rejects_the_bundle() {
    let source = r#"
define("app/main", [], function() {
  console.log(new.target);
});
define("app/dep", [], function() {
  return 42;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(
        raw.len(),
        1,
        "the bundle must fall back as one file: {raw:#?}"
    );
    assert!(
        raw[0].1.contains("define(\"app/main\"") && raw[0].1.contains("new.target"),
        "the original define calls must be preserved:\n{}",
        raw[0].1
    );
}

/// Accessor bodies bind their own `new.target`, but computed accessor keys
/// still evaluate in the surrounding AMD factory and cannot be lifted.
#[test]
fn factory_observing_new_target_in_computed_accessor_key_rejects_the_bundle() {
    let source = r#"
define("app/main", [], function() {
  const object = {
    get [new.target ? "constructed" : "called"]() { return 1; }
  };
  return object;
});
define("app/dep", [], function() {
  return 42;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(
        raw.len(),
        1,
        "the bundle must fall back as one file: {raw:#?}"
    );
    assert!(
        raw[0].1.contains("define(\"app/main\"") && raw[0].1.contains("new.target"),
        "the original define calls must be preserved:\n{}",
        raw[0].1
    );
}

#[test]
fn factory_observing_new_target_in_computed_setter_key_rejects_the_bundle() {
    let source = r#"
define("app/main", [], function() {
  return {
    set [new.target ? "constructed" : "called"](value) { consume(value); }
  };
});
define("app/dep", [], function() {
  return 42;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(
        raw.len(),
        1,
        "the bundle must fall back as one file: {raw:#?}"
    );
    assert!(
        raw[0].1.contains("define(\"app/main\"") && raw[0].1.contains("new.target"),
        "the original define calls must be preserved:\n{}",
        raw[0].1
    );
}

/// Same fail-close for a factory observing its `this` receiver.
#[test]
fn factory_observing_this_rejects_the_bundle() {
    let source = r#"
define("app/main", [], function() {
  if (this.skipBoot) return;
  this.booted = true;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert!(
        raw[0].1.contains("define(\"app/main\""),
        "the original define call must be preserved:\n{}",
        raw[0].1
    );
}

/// A guarded async factory whose await appears only as `for await` must still
/// regain an async boundary: the loop head carries no AwaitExpr node, and a
/// sync arrow around `for await` does not parse.
#[test]
fn async_factory_with_for_await_restores_an_async_boundary() {
    let source = r#"
define("app/main", [], async function() {
  if (globalThis.skip) return;
  for await (const v of globalThis.src()) console.log(v);
  return 1;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1, "the bundle should unpack: {raw:#?}");
    let main = &raw[0].1;
    assert!(
        main.contains("async") && main.contains("for await"),
        "the restored boundary must be async so `for await` stays legal:\n{main}"
    );
    assert_eq!(
        validate_output_modules(&raw),
        vec![],
        "the restored async boundary must parse:\n{main}"
    );
}

/// `arguments` inside a nested ordinary function belongs to that function and
/// must not block the boundary restoration.
#[test]
fn nested_function_arguments_does_not_block_the_lift() {
    let source = r#"
define("app/main", ["app/dep"], function(dep) {
  if (dep.skip) return;
  function tail() {
    return arguments[0];
  }
  console.log(tail(dep));
});
define("app/dep", [], function() {
  return { skip: false };
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(
        raw.len(),
        2,
        "nested-function arguments must not reject the bundle: {raw:#?}"
    );
    let main = raw
        .iter()
        .find(|(name, _)| name.contains("main"))
        .expect("main module should exist");
    assert!(
        main.1.contains("return;") && main.1.contains("function tail"),
        "the guarded body keeps its boundary and the nested function:\n{}",
        main.1
    );
    assert_eq!(validate_output_modules(&raw), vec![]);
}

#[test]
fn empty_amd_define_is_not_unpacked() {
    let source = "define();";
    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains("define();"),
        "an empty define call should remain unchanged:\n{}",
        raw[0].1
    );
}

#[test]
fn anonymous_amd_external_dependency_preserves_bare_specifier() {
    // Rollup AMD output for an external package dependency. A bare AMD module
    // ID that is not another define in the bundle must stay bare; rewriting it
    // to `./math-lib.js` changes package resolution semantics.
    let source = r#"
define(["exports", "math-lib"], function(exports, mathLib) {
  const total = mathLib.add(1, 2);
  exports.total = total;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains(r#"var mathLib = require("math-lib");"#),
        "external AMD dependency should remain bare:\n{}",
        raw[0].1
    );

    let decompiled = pairs(source);
    assert_eq!(decompiled.len(), 1);
    assert!(
        decompiled[0].1.contains(r#"from "math-lib""#),
        "decompiled import should preserve the external package specifier:\n{}",
        decompiled[0].1
    );
}

#[test]
fn swc_default_interop_assignment_does_not_write_to_recovered_import() {
    let source = r#"
define([
  "exports",
  "@swc/helpers/_/_interop_require_default",
  "react"
], function(exports, _interop_require_default, _react) {
  "use strict";
  _react = _interop_require_default(_react);
  exports.hook = _react.default.useEffect;
});
"#;

    let decompiled = pairs(source);
    assert_eq!(decompiled.len(), 1);
    let module = &decompiled[0].1;
    assert!(
        module.contains(r#"from "react""#)
            && module.contains("_react.useEffect")
            && !module.contains("_react =")
            && !module.contains("_react.default"),
        "the generated interop assignment should collapse into a legal default import:\n{module}"
    );
    assert_eq!(
        validate_output_modules(&decompiled),
        vec![],
        "recovered AMD modules must not assign to imports"
    );
}

#[test]
fn swc_namespace_member_interop_assignment_does_not_write_to_recovered_import() {
    // Real SWC AMD external-helper shape (swc issue-3067 fixture): the helper
    // dependency is a namespace object whose callable export is named `_`.
    let source = r#"
define([
  "require",
  "exports",
  "@swc/helpers/_/_interop_require_default",
  "lodash/dist/something.js"
], function(require, exports, _interop_require_default, _something) {
  "use strict";
  Object.defineProperty(exports, "__esModule", { value: true });
  Object.defineProperty(exports, "displayC", {
    enumerable: true,
    get: function() { return displayC; }
  });
  _something = _interop_require_default._(_something);
  function displayC() {
    (0, _something.default)();
    return "Display C";
  }
});
"#;

    let decompiled = pairs(source);
    assert_eq!(decompiled.len(), 1);
    let module = &decompiled[0].1;
    assert!(
        module.contains(r#"from "lodash/dist/something.js""#)
            && module.contains("_something()")
            && !module.contains("_interop_require_default")
            && !module.contains("_something =")
            && !module.contains("_something.default"),
        "the SWC namespace-member wrapper should collapse into a legal default import:\n{module}"
    );
    assert_eq!(
        validate_output_modules(&decompiled),
        vec![],
        "the real SWC AMD member form must produce a valid module graph"
    );
}

#[test]
fn rejected_swc_namespace_member_interop_keeps_executable_imports() {
    let source = r#"
define([
  "exports",
  "@swc/helpers/_/_interop_require_default",
  "react"
], function(exports, _interop_require_default, _react) {
  "use strict";
  probe();
  _react = _interop_require_default._(_react);
  exports.hook = _react.default.useEffect;
  function probe() {
    return _react.default;
  }
});
"#;

    let decompiled = pairs(source);
    assert_eq!(decompiled.len(), 1);
    let module = &decompiled[0].1;
    assert!(
        module.contains(
            "import * as _interop_require_default from \"@swc/helpers/_/_interop_require_default\""
        ) && module.contains("_react = _interop_require_default._(_react)")
            && module.contains("_react.default"),
        "a rejected member-form recovery must preserve both namespace and wrapper:\n{module}"
    );
    assert_eq!(
        validate_output_modules(&decompiled),
        vec![],
        "the fail-closed member form must remain valid ESM"
    );
}

#[test]
fn rejected_swc_default_interop_assignment_keeps_a_mutable_local() {
    let source = r#"
define([
  "exports",
  "@swc/helpers/_/_interop_require_default",
  "react"
], function(exports, _interop_require_default, _react) {
  "use strict";
  probe();
  _react = _interop_require_default(_react);
  exports.hook = _react.default.useEffect;
  function probe() {
    return _react.default;
  }
});
"#;

    let decompiled = pairs(source);
    assert_eq!(decompiled.len(), 1);
    let module = &decompiled[0].1;
    assert!(
        module.contains("from \"@swc/helpers/_/_interop_require_default\"")
            && module.contains("_react.default")
            && !module.contains("_react = _react"),
        "a rejected interop recovery must preserve the wrapper call:\n{module}"
    );
    assert_eq!(
        validate_output_modules(&decompiled),
        vec![],
        "the generated dependency local must remain mutable when the wrapper assignment stays"
    );
}

#[test]
fn object_literal_amd_define_unpack() {
    let source = r#"
define("config", {
  answer: 42
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "config.js");
    assert!(
        raw[0].1.contains("module.exports = {"),
        "object literal AMD factory should become module.exports:\n{}",
        raw[0].1
    );
}

#[test]
fn plain_umd_factory_unwraps_to_single_module() {
    let source = r#"
(function(root, factory) {
  if (typeof define === "function" && define.amd) {
    define([], factory);
  } else if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.MathLib = factory();
  }
})(this, function() {
  function add(a, b) {
    return a + b;
  }
  return { add: add };
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains("function add(a, b)") && raw[0].1.contains("module.exports = {"),
        "plain UMD wrapper should be removed:\n{}",
        raw[0].1
    );
}

#[test]
fn ordinary_two_arg_iife_is_not_plain_umd() {
    let source = r#"
(function(root, factory) {
  root.value = factory();
})(this, function() {
  return 1;
});
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains("root.value = factory();"),
        "ordinary IIFEs should stay intact:\n{}",
        raw[0].1
    );
}

#[test]
fn amd_define_with_unrelated_top_level_code_is_not_partially_unpacked() {
    let source = r#"
define("config", {
  answer: 42
});
boot();
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains("define(\"config\"") && raw[0].1.contains("boot();"),
        "mixed top-level code should stay intact instead of dropping boot():\n{}",
        raw[0].1
    );
}

#[test]
fn umd_with_unrelated_top_level_code_is_not_partially_unpacked() {
    let source = r#"
(function(root, factory) {
  if (typeof define === "function" && define.amd) {
    define([], factory);
  } else if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.MathLib = factory();
  }
})(this, function() {
  return { value: 1 };
});
boot();
"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains("root.MathLib = factory();") && raw[0].1.contains("boot();"),
        "mixed top-level UMD code should stay intact instead of dropping boot():\n{}",
        raw[0].1
    );
}

#[test]
fn named_defines_merged_into_one_sequence_unpack() {
    // shape: producer r.js@2.3.7 optimize=uglify (no loader; the bundled
    // UglifyJS merges every define into one statement). Ids synthetic.
    let source = r#"define("lib/twice",[],function(){return function(n){return 2*n}}),define("app/main",["lib/twice"],function(t){globalThis.sink=t(3)});"#;

    let raw = raw_pairs(source);
    let names: Vec<&str> = raw.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["lib/twice.js", "app/main.js"]);

    let decompiled = pairs(source);
    let main = decompiled
        .iter()
        .find(|(name, _)| name == "app/main.js")
        .map(|(_, code)| code)
        .expect("expected decompiled main module");
    assert!(
        main.contains(r#""../lib/twice.js""#),
        "the dependency should link to the split module:\n{main}"
    );
    assert!(
        validate_output_modules(&decompiled).is_empty(),
        "output must validate: {:?}",
        validate_output_modules(&decompiled)
    );
}

#[test]
fn define_merged_into_a_sequence_with_other_code_is_not_partially_unpacked() {
    // The split copy holds a call that is no define, so the input is not
    // define-only, and a split copy must not reach the single-UMD fallback
    // either. shape: hypothetical.
    let source = r#"define("config",{answer:42}),boot();"#;

    let raw = raw_pairs(source);
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].0, "module.js");
    assert!(
        raw[0].1.contains("boot()"),
        "mixed top-level code should stay intact:\n{}",
        raw[0].1
    );
}

// --- Plain UMD factory parameters ------------------------------------------
//
// A plain UMD file lifts to module scope only through the branch Node takes
// (the CommonJS one), and each factory parameter is bound to the argument that
// branch passes.

fn single_module(modules: &[(String, String)]) -> &str {
    assert_eq!(modules.len(), 1, "expected one module: {modules:?}");
    &modules[0].1
}

fn assert_valid(modules: &[(String, String)]) {
    let problems = validate_output_modules(modules);
    assert!(problems.is_empty(), "output must validate: {problems:?}");
}

fn assert_kept_whole(source: &str, marker: &str) {
    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(marker) && code.contains("typeof module"),
        "the wrapper should stay in the output:\n{code}"
    );
}

#[test]
fn umd_external_parameter_becomes_an_import() {
    // shape: producer rollup@4.63.6 format: "umd", external imports
    let source = r#"(function (global, factory) {
	typeof exports === 'object' && typeof module !== 'undefined' ? factory(exports, require('dep-alpha'), require('dep-beta')) :
	typeof define === 'function' && define.amd ? define(['exports', 'dep-alpha', 'dep-beta'], factory) :
	(global = typeof globalThis !== 'undefined' ? globalThis : global || self, factory(global.lib = {}, global.alpha, global.beta));
})(this, (function (exports, depAlpha, fmt) { 'use strict';

	function first(a) { return fmt(depAlpha.join(a, "x")); }
	const second = 2;

	exports.first = first;
	exports.second = second;

}));
"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(r#"var depAlpha = require("dep-alpha");"#)
            && code.contains(r#"var fmt = require("dep-beta");"#),
        "each external parameter should be bound to its require:\n{code}"
    );
    assert!(
        code.find("use strict") < code.find("var depAlpha"),
        "the factory's directive should stay first:\n{code}"
    );

    let decompiled = pairs(source);
    let code = single_module(&decompiled);
    assert!(
        code.contains(r#"from "dep-alpha""#) && code.contains(r#"from "dep-beta""#),
        "the bound requires should become imports:\n{code}"
    );
    assert!(
        code.contains("export { first }") || code.contains("export function first"),
        "named exports should be recovered:\n{code}"
    );
    assert_valid(&decompiled);
}

#[test]
fn minified_umd_parameters_are_bound_and_exports_renamed() {
    // shape: producer rollup@4.63.6 format: "umd" + terser@5.51.2
    let source = r#"!function(e,t){"object"==typeof exports&&"undefined"!=typeof module?t(exports,require("dep-alpha"),require("dep-beta")):"function"==typeof define&&define.amd?define(["exports","dep-alpha","dep-beta"],t):t((e="undefined"!=typeof globalThis?globalThis:e||self).lib={},e.alpha,e.beta)}(this,function(e,t,i){"use strict";e.first=function(e){return i(t.join(e,"x"))},e.second=2});"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(r#"var t = require("dep-alpha");"#)
            && code.contains(r#"var i = require("dep-beta");"#),
        "minified parameters should be bound:\n{code}"
    );
    assert!(
        code.contains("exports.first = function(e)") && !code.contains("var e"),
        "the exports parameter should take the global's name, not the inner `e`:\n{code}"
    );

    let decompiled = pairs(source);
    let code = single_module(&decompiled);
    assert!(
        code.contains(r#"from "dep-alpha""#)
            && code.contains("export const first")
            && code.contains("export const second = 2"),
        "the minified build should recover imports and exports:\n{code}"
    );
    assert_valid(&decompiled);
}

#[test]
fn umd_default_export_factory_binds_its_parameter() {
    // shape: producer rollup@4.63.6 format: "umd" + terser@5.51.2, default
    // export only (`module.exports = factory(require(...))`)
    let source = r#"!function(e,n){"object"==typeof exports&&"undefined"!=typeof module?module.exports=n(require("dep-alpha")):"function"==typeof define&&define.amd?define(["dep-alpha"],n):(e="undefined"!=typeof globalThis?globalThis:e||self).lib=n(e.alpha)}(this,function(e){"use strict";return function(n){return e.join(n,"x")}});"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(r#"var e = require("dep-alpha");"#)
            && code.contains("module.exports = function(n)"),
        "the parameter should be bound and the return become module.exports:\n{code}"
    );

    let decompiled = pairs(source);
    let code = single_module(&decompiled);
    assert!(
        code.contains(r#"from "dep-alpha""#) && code.contains("export default function"),
        "the default export should be recovered with its import:\n{code}"
    );
    assert_valid(&decompiled);
}

#[test]
fn babel_umd_exports_parameter_is_recovered() {
    // shape: producer @babel/core@7.29.7 + @babel/plugin-transform-modules-umd@7.29.7
    let source = r#"(function (global, factory) {
  if (typeof define === "function" && define.amd) {
    define(["exports", "dep-alpha", "dep-beta"], factory);
  } else if (typeof exports !== "undefined") {
    factory(exports, require("dep-alpha"), require("dep-beta"));
  } else {
    var mod = {
      exports: {}
    };
    factory(mod.exports, global.depAlpha, global.depBeta);
    global.named = mod.exports;
  }
})(typeof globalThis !== "undefined" ? globalThis : typeof self !== "undefined" ? self : this, function (_exports, _depAlpha, _depBeta) {
  "use strict";

  Object.defineProperty(_exports, "__esModule", {
    value: true
  });
  _exports.first = first;
  _exports.second = void 0;
  _depBeta = _interopRequireDefault(_depBeta);
  function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
  function first(a) {
    return (0, _depBeta.default)((0, _depAlpha.join)(a, "x"));
  }
  const second = _exports.second = 2;
});
"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains("exports.first = first;")
            && code.contains(r#"var _depBeta = require("dep-beta");"#)
            && code.contains("_depBeta = _interopRequireDefault(_depBeta);"),
        "the parameters should be bound, including the one the factory writes:\n{code}"
    );

    let decompiled = pairs(source);
    let code = single_module(&decompiled);
    assert!(
        code.contains(r#"from "dep-alpha""#) && code.contains(r#"from "dep-beta""#),
        "the bound requires should become imports:\n{code}"
    );
    assert!(
        code.contains("export const second = 2"),
        "named exports should be recovered:\n{code}"
    );
    assert_valid(&decompiled);
}

#[test]
fn umd_parameter_the_call_does_not_pass_is_undefined() {
    // shape: hypothetical
    let source = r#"(function (root, factory) {
  typeof exports === "object" && typeof module !== "undefined" ? factory(exports) : factory(root.lib = {}, root);
})(this, function (exports, host) {
  exports.hasHost = typeof host !== "undefined";
});
"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains("var host;") && code.contains("exports.hasHost"),
        "an unpassed parameter should be declared without a value:\n{code}"
    );
    assert_valid(&pairs(source));
}

#[test]
fn umd_parameter_rename_does_not_capture_a_free_exports() {
    // The body reads the CommonJS `exports` itself, so the parameter cannot
    // take that name and stays an alias. shape: hypothetical
    let source = r#"!function(e,t){"object"==typeof exports&&"undefined"!=typeof module?t(exports):t(e.lib={})}(this,function(e){e.kind=function(){return typeof exports}});"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains("var e = exports;") && code.contains("return typeof exports;"),
        "the parameter should stay an alias:\n{code}"
    );
}

#[test]
fn umd_parameter_rename_moves_an_inner_binding_out_of_the_way() {
    // An inner parameter named like the global would capture the renamed
    // reference. shape: hypothetical
    let source = r#"!function(e,t){"object"==typeof exports&&"undefined"!=typeof module?t(exports):t(e.lib={})}(this,function(e){e.same=function(exports){return e===exports}});"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        !code.contains("exports === exports"),
        "the outer parameter must not be captured by the inner one:\n{code}"
    );
}

#[test]
fn umd_parameter_named_like_a_commonjs_global_is_kept_whole() {
    // Binding `module` to a dependency would shadow the CommonJS `module` the
    // lifted code writes. shape: hypothetical
    let source = r#"(function (root, factory) {
  typeof exports === "object" && typeof module !== "undefined" ? module.exports = factory(require("dep-alpha")) : root.lib = factory(root.alpha);
})(this, function (module) {
  return module.join("a", "b");
});
"#;
    assert_kept_whole(source, "factory(require(\"dep-alpha\"))");
}

#[test]
fn umd_with_a_conditional_node_branch_is_kept_whole() {
    // The CommonJS branch picks between two calls on a property of the
    // wrapper's root argument, and passes that root to the factory.
    // shape: wild-observed (hand-written library wrapper), names synthetic
    let source = r#"!function(e,t){"object"==typeof module&&"object"==typeof module.exports?module.exports=e.document?t(e,!0):function(e){return t(e)}:t(e)}("undefined"!=typeof window?window:this,function(host,skipGlobal){var page=host.document;function lib(){return page.title}"function"==typeof define&&define.amd&&define("lib",[],function(){return lib});return typeof skipGlobal==="undefined"&&(host.lib=lib),lib});"#;
    assert_kept_whole(source, "skipGlobal");

    let decompiled = pairs(source);
    let code = single_module(&decompiled);
    assert!(
        code.contains("host.document") && code.contains("(host, skipGlobal)"),
        "the factory should keep its parameters:\n{code}"
    );
    assert_valid(&decompiled);
}

#[test]
fn umd_root_argument_read_of_a_browser_global_is_dropped() {
    // Reading `window` has no effect beyond throwing outside a browser, so
    // the lift may drop it. shape: hypothetical
    let source = r#"(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.Lib = factory();
  }
})(window, function () {
  return { value: 1 };
});
"#;
    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains("module.exports = {") && !code.contains("root.Lib"),
        "the factory should be lifted:\n{code}"
    );
}

#[test]
fn umd_root_argument_with_an_effect_is_kept_whole() {
    // The lift would drop the call. shape: hypothetical
    let source = r#"(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.Lib = factory();
  }
})(setup(), function () {
  return { value: 1 };
});
"#;
    assert_kept_whole(source, "root.Lib = factory()");
}

#[test]
fn umd_factory_without_a_return_sets_module_exports_to_undefined() {
    // `module.exports = factory(...)` with a factory that only registers
    // itself on its dependency. shape: wild-observed (hand-written plugin
    // wrapper), names synthetic
    let source = r#"(function(root,factory){if(typeof exports==='object'){module.exports=factory(require('dep-alpha'))}else if(typeof define==='function'&&define.amd){define(['dep-alpha'],factory)}else{factory(root.Alpha)}}(this,function(alpha){alpha.plugin=function(){return 1}}));"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(r#"var alpha = require("dep-alpha");"#)
            && code.trim_end().ends_with("module.exports = void 0;"),
        "the lifted module should end by clearing module.exports:\n{code}"
    );
    assert_valid(&pairs(source));
}

#[test]
fn umd_wrapper_variable_on_the_node_branch_is_followed() {
    // The wrapper keeps "undefined" in a variable and compares against it.
    // shape: wild-observed (generated SDK wrapper), names synthetic
    let source = r#"!function(n,t){var u="undefined";if("object"==typeof exports&&typeof module!=u)t(exports);else if("function"==typeof define&&define.amd)define(["exports"],t);else{var r,n=typeof globalThis!=u?globalThis:n||self;t(n.Lib={})}}(this,function(n){"use strict";n.version="1.0.0"});"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(r#"exports.version = "1.0.0""#),
        "the exports parameter should be bound:\n{code}"
    );
}

#[test]
fn umd_template_literal_typeof_comparison_is_followed() {
    // shape: wild-observed (minified wrapper with template-literal strings)
    let source = r#"!function(n,t){typeof exports==`object`&&typeof module<`u`?module.exports=t():typeof define==`function`&&define.amd?define([],t):(n=typeof globalThis<`u`?globalThis:n||self,n.Lib=t())}(this,function(){return{value:1}});"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains("module.exports = {"),
        "the factory should be lifted:\n{code}"
    );
}

#[test]
fn umd_factory_result_the_call_ignores_is_kept_whole() {
    // Node's branch discards the factory's return value, so lifting its
    // `return` into `module.exports` would change the exports.
    // shape: hypothetical
    let source = r#"(function (root, factory) {
  typeof exports === "object" && typeof module !== "undefined" ? factory(exports) : root.lib = factory({});
})(this, function (exports) {
  exports.value = 1;
  return { other: 2 };
});
"#;
    assert_kept_whole(source, "factory(exports)");
}

#[test]
fn umd_factory_declaring_a_function_named_like_a_parameter_is_kept_whole() {
    // In the factory the declaration replaces the parameter; at module scope
    // the binding would overwrite the hoisted declaration. shape: hypothetical
    let source = r#"(function (root, factory) {
  typeof module === "object" && module.exports ? module.exports = factory(require("dep-alpha")) : root.lib = factory(root.alpha);
})(this, function (dep) {
  function dep() { return 1; }
  return dep;
});
"#;
    assert_kept_whole(source, "function dep()");
}

#[test]
fn esbuild_minified_umd_typeof_comparison_is_followed() {
    // shape: producer rollup@4.63.6 format: "umd" + esbuild@0.28.0 --minify
    // (`typeof module<"u"` for `typeof module !== "undefined"`)
    let source = r#"(function(e,n){typeof exports=="object"&&typeof module<"u"?n(exports,require("dep-alpha"),require("dep-beta")):typeof define=="function"&&define.amd?define(["exports","dep-alpha","dep-beta"],n):(e=typeof globalThis<"u"?globalThis:e||self,n(e.lib={},e.alpha,e.beta))})(this,(function(e,n,i){"use strict";function t(f){return i(n.join(f,"x"))}const d=2;e.first=t,e.second=d}));"#;

    let raw = raw_pairs(source);
    let code = single_module(&raw);
    assert!(
        code.contains(r#"var n = require("dep-alpha");"#)
            && code.contains(r#"var i = require("dep-beta");"#)
            && code.contains("exports.first = t"),
        "the parameters should be bound:\n{code}"
    );
    assert_valid(&pairs(source));
}

#[test]
fn umd_wrapper_that_redeclares_the_factory_name_is_kept_whole() {
    // The hoisted declaration replaces the factory parameter, so Node's
    // branch calls the declaration, not the factory. shape: hypothetical
    let source = r#"(function (root, factory) {
  function factory() { return { swapped: true }; }
  typeof module === "object" && module.exports ? module.exports = factory() : root.lib = factory();
})(this, function () {
  return { value: 1 };
});
"#;
    assert_kept_whole(source, "swapped");
}
