use std::fs;

use wakaru_core::{unpack, unpack_raw, DecompileOptions};

fn fixture(path: &str) -> String {
    let full = format!("tests/bundles/systemjs-gen/dist/{path}");
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
}

fn unpack_fixture_raw(path: &str) -> Vec<(String, String)> {
    let source = fixture(path);
    let output =
        unpack_raw(&source, &DecompileOptions::default()).expect("unpack_raw should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings for {path}: {:?}",
        output.warnings
    );
    output.modules
}

fn unpack_source(source: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: "system-bundle.js".to_string(),
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

fn module_code<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(filename, _)| filename == name)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "expected module {name}, got {:?}",
                pairs
                    .iter()
                    .map(|(filename, _)| filename)
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn rollup_preserve_module_entry_raw_reconstructs_esm() {
    let raw = unpack_fixture_raw("preserve/entry.js");
    assert_eq!(raw.len(), 1);

    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains(r#"import greet, { named } from "./dep.js";"#),
        "entry should recover default + named imports:\n{entry}"
    );
    assert!(
        entry.contains(r#"import("./lazy.js")"#) || entry.contains(r#"import('./lazy.js')"#),
        "contextual dynamic import should become import():\n{entry}"
    );
    assert!(
        entry.contains("import.meta.url.length"),
        "context meta should become import.meta:\n{entry}"
    );
    assert!(
        entry.contains("export { run, value };") || entry.contains("export { value, run };"),
        "entry should recover named exports:\n{entry}"
    );
}

#[test]
fn swc_systemjs_raw_reconstructs_context_and_assignment_exports() {
    let raw = unpack_fixture_raw("swc/src/entry.js");
    assert_eq!(raw.len(), 1);

    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains(r#"import greet, { named } from "./dep.js";"#),
        "SWC setter imports should recover default + named imports:\n{entry}"
    );
    assert!(
        entry.contains(r#"import("./lazy.js")"#),
        "SWC _context.import should become import():\n{entry}"
    );
    assert!(
        entry.contains("import.meta.url.length"),
        "SWC _context.meta should become import.meta:\n{entry}"
    );
    assert!(
        entry.contains("value = named + 1;"),
        "SWC assignment export should keep the assignment:\n{entry}"
    );
    assert!(
        entry.contains("export { run, value };") || entry.contains("export { value, run };"),
        "SWC export calls should recover named exports:\n{entry}"
    );
}

#[test]
fn babel_systemjs_raw_reconstructs_outer_exports() {
    let raw = unpack_fixture_raw("babel/entry.js");
    assert_eq!(raw.len(), 1);

    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains(r#"import greet, { named } from "./dep.js";"#),
        "Babel setter imports should recover default + named imports:\n{entry}"
    );
    assert!(
        entry.contains(r#"import("./lazy.js")"#),
        "Babel _context.import should become import():\n{entry}"
    );
    assert!(
        entry.contains("import.meta.url.length"),
        "Babel _context.meta should become import.meta:\n{entry}"
    );
    assert!(
        entry.contains("export { run, value };") || entry.contains("export { value, run };"),
        "Babel outer export and execute export should both survive:\n{entry}"
    );
}

#[test]
fn tsc_systemjs_raw_reconstructs_namespace_import_and_outer_exports() {
    let raw = unpack_fixture_raw("tsc/entry.js");
    assert_eq!(raw.len(), 1);

    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains(r#"import * as dep_1 from "./dep";"#),
        "TypeScript namespace setter should recover a namespace import:\n{entry}"
    );
    assert!(
        entry.contains("value = dep_1.named + 1;"),
        "TypeScript assignment export should keep the assignment:\n{entry}"
    );
    assert!(
        entry.contains("export { run as default, value };")
            || entry.contains("export { value, run as default };"),
        "TypeScript outer default export and execute export should both survive:\n{entry}"
    );
    assert!(
        !entry.contains("exports_1")
            && !entry.contains("context_1")
            && !entry.contains("__moduleName"),
        "SystemJS runtime bindings should not leak into output:\n{entry}"
    );
}

#[test]
fn webpack_system_library_raw_recurses_into_inner_bundle() {
    let source = fixture("webpack-system/bundle.js");
    let raw = unpack_raw(&source, &DecompileOptions::default()).expect("unpack_raw should succeed");
    assert!(
        !raw.has_errors(),
        "unexpected raw warnings: {:?}",
        raw.warnings
    );
    let raw = raw.modules;
    assert_eq!(raw.len(), 2);

    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains(r#"require("./webpack-src/dep.js")"#),
        "webpack System.register wrapper should expose the inner entry module:\n{entry}"
    );
    assert!(
        entry.contains("require.r(exports);") && entry.contains("require.d(exports,"),
        "raw webpack inner entry should preserve runtime export markers:\n{entry}"
    );

    let dep = module_code(&raw, "webpack-src/dep.js");
    assert!(
        dep.contains("require.r(exports);") && dep.contains("require.d(exports,"),
        "raw webpack inner dependency should preserve runtime export markers:\n{dep}"
    );

    let modules = unpack_source(&source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains(r#"from "./webpack-src/dep.js";"#),
        "normal unpack should convert the inner entry require to import:\n{entry}"
    );
    assert!(
        entry.contains("export { value };") && entry.contains("export { run as default };"),
        "normal unpack should recover inner entry exports:\n{entry}"
    );

    let dep = module_code(&modules, "webpack-src/dep.js");
    assert!(
        dep.contains("export { named };") && dep.contains("export { double as default };"),
        "normal unpack should recover inner dependency exports:\n{dep}"
    );
}

#[test]
fn named_register_bundle_unpacks_multiple_modules() {
    let source = r#"
System.register("dep", [], function (_export) {
  return {
    execute: function () {
      _export("default", greet);
      const named = _export("named", 41);
      function greet(name) {
        return `hi ${name}`;
      }
    }
  };
});
System.register("entry", ["dep"], function (_export) {
  var greet, named;
  return {
    setters: [function (module) {
      greet = module.default;
      named = module.named;
    }],
    execute: function () {
      const value = _export("value", named + 1);
      var result = _export("default", greet(value));
    }
  };
});
"#;

    let modules = unpack_source(source);
    assert_eq!(modules.len(), 2);

    let dep = module_code(&modules, "dep.js");
    assert!(
        dep.contains("export { greet as default")
            || dep.contains("export { named, greet as default"),
        "dep should recover default export:\n{dep}"
    );

    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains(r#"import greet, { named } from "dep";"#),
        "entry should recover SystemJS setter imports:\n{entry}"
    );
    assert!(
        entry.contains("result as default") && entry.contains("value"),
        "entry should recover default and named exports:\n{entry}"
    );
}

#[test]
fn mixed_invalid_system_register_preserves_whole_input() {
    let source = r#"
System.register("dep", [], function (_export) {
  return {
    execute: function () {
      _export("value", 1);
    }
  };
});
System.register("odd", "not-an-array", function (_export) {
  return {
    execute: function () {
      _export("value", 2);
    }
  };
});
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "system-bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should preserve invalid System.register input");

    assert_eq!(
        output.modules.len(),
        1,
        "invalid System.register must not emit a partial module set: {:?}",
        output.modules
    );
    assert_eq!(output.modules[0].0, "module.js");
    assert!(
        output.modules[0].1.contains(r#"System.register("dep""#)
            && output.modules[0].1.contains(r#"System.register("odd""#),
        "fallback module should preserve both register calls:\n{}",
        output.modules[0].1
    );
    assert!(
        output.detected_formats.is_empty(),
        "invalid mixed System.register input should not be reported as a successful split"
    );
}
