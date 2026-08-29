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
        main.contains(r#"const math = require("../utils/math.js");"#),
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
        raw[0].1.contains(r#"const dep = require("./dep.js");"#)
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
        raw[0].1.contains(r#"const mathLib = require("math-lib");"#),
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
