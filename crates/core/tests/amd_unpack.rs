use wakaru_core::{unpack, unpack_raw, DecompileOptions};

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
