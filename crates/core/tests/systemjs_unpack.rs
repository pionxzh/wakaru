use std::fs;

use wakaru_core::driver::test_support::{unpack, unpack_raw};
use wakaru_core::{validate_output_modules, DecompileOptions};

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

fn unpack_source_raw(source: &str) -> Vec<(String, String)> {
    let output =
        unpack_raw(source, &DecompileOptions::default()).expect("unpack_raw should succeed");
    assert!(
        !output.has_errors(),
        "unexpected raw warnings: {:?}",
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
fn rollup_terser_lifted_object_expression_stays_valid() {
    let source = fixture("rollup-terser/entry.js");

    for (stage, modules) in [
        ("raw", unpack_source_raw(&source)),
        ("decompiled", unpack_source(&source)),
    ] {
        assert_eq!(modules.len(), 1, "unexpected {stage} modules: {modules:?}");
        let entry = &modules[0].1;
        assert_no_system_register(entry, stage);
        assert_no_leftover_export_call(entry, stage);
        assert!(
            entry.contains("Before")
                && entry.contains("After")
                && entry.contains("key()")
                && entry.contains("lookup()"),
            "{stage} output must retain the exports and object-headed call:\n{entry}"
        );
        assert_valid_unpacked_esm(&modules, &format!("Rollup + Terser {stage}"));
    }
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
fn babel_chained_member_updates_share_one_live_export() {
    let raw = unpack_fixture_raw("babel/chained-export.js");
    assert_eq!(raw.len(), 1);

    let entry = module_code(&raw, "entry.js");
    assert_eq!(
        validate_output_modules(&raw),
        vec![],
        "Babel chained updates should not create duplicate ESM exports:\n{entry}"
    );

    let first_value = entry
        .find("item = makeOne();")
        .unwrap_or_else(|| panic!("first exported assignment should survive:\n{entry}"));
    let first_member = entry
        .find("item.a = 1;")
        .unwrap_or_else(|| panic!("first member update should use the live binding:\n{entry}"));
    let second_value = entry
        .find("item = makeTwo();")
        .unwrap_or_else(|| panic!("second exported assignment should survive:\n{entry}"));
    let second_member = entry
        .find("item.b = 2;")
        .unwrap_or_else(|| panic!("second member update should use the live binding:\n{entry}"));

    assert!(
        first_value < first_member && first_member < second_value && second_value < second_member,
        "Babel chained export evaluation order should survive:\n{entry}"
    );
    assert_eq!(
        entry.matches("export { item };").count(),
        1,
        "item should have one live ESM export:\n{entry}"
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

#[test]
fn invalid_iife_system_register_preserves_whole_input() {
    let source = r#"
(function () {
  System.register("dep", [], function (_export) {
    return {
      execute: function () {
        _export("value", 1);
      }
    };
  });
  System.register("entry", "not-an-array", function (_export) {
    return {
      execute: function () {
        _export("value", 2);
      }
    };
  });
})();
"#;

    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "system-bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("raw unpack should preserve invalid IIFE System.register input");

    assert_eq!(
        output.modules.len(),
        1,
        "invalid IIFE System.register must not emit a partial module set: {:?}",
        output.modules
    );
    assert_eq!(output.modules[0].0, "module.js");
    assert!(
        output.modules[0].1.contains(r#"System.register("dep""#)
            && output.modules[0]
                .1
                .contains(r#"System.register("entry", "not-an-array""#),
        "fallback module should preserve the valid and invalid IIFE registers:\n{}",
        output.modules[0].1
    );
    assert!(
        output.detected_formats.is_empty(),
        "invalid IIFE System.register input should not be reported as a successful split"
    );
}

#[test]
fn named_iife_export_keeps_member_assignment_and_sequence_side_effect() {
    let source = r#"
System.register("entry", ["utils"], function (_export) {
  var BaseClass;
  return {
    setters: [function (Utils) {
      BaseClass = Utils.BaseClass;
    }],
    execute: function () {
      _export("DerivedClass", function (BaseClass) {
        function DerivedClass() {}
        return DerivedClass;
      }(BaseClass)).marker = "derived", after();
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    let binding = entry
        .find("export const DerivedClass =")
        .unwrap_or_else(|| panic!("named IIFE should bind the export name directly:\n{entry}"));
    let member_assignment = entry
        .find("DerivedClass.marker = \"derived\";")
        .unwrap_or_else(|| panic!("member assignment should use the export name:\n{entry}"));
    let after = entry
        .find("after();")
        .unwrap_or_else(|| panic!("sequence side effect should be preserved:\n{entry}"));

    assert!(
        binding < member_assignment && member_assignment < after,
        "export binding, member assignment, and sequence side effect should preserve order:\n{entry}"
    );
    assert!(
        !entry.contains("__systemjs_export"),
        "free export name should not use a synthetic alias:\n{entry}"
    );
    assert!(
        !entry.lines().any(|line| line.starts_with("function (")),
        "top-level anonymous function should not be emitted:\n{entry}"
    );
}

#[test]
fn nested_inner_function_iife_member_export_uses_export_name() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("WidgetUtils", function () {
        function Inner() {}
        return Inner;
      }()).tag = "WidgetUtils";
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export const WidgetUtils =")
            && entry.contains("WidgetUtils.tag = \"WidgetUtils\";"),
        "nested IIFE member export should bind the export name directly:\n{entry}"
    );
    assert!(
        !entry.contains("__systemjs_export"),
        "free export name should not use a synthetic alias:\n{entry}"
    );
}

#[test]
fn repeated_member_export_updates_one_live_binding() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("HelperUtils", makeValue());
      _export("HelperUtils", makeOther()).tag = "HelperUtils";
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export let HelperUtils;") && entry.contains("HelperUtils = makeValue()"),
        "the first value should initialize one mutable live export:\n{entry}"
    );
    assert!(
        entry.contains("(HelperUtils = makeOther()).tag = \"HelperUtils\";"),
        "the member export should update that same live binding before mutation:\n{entry}"
    );
    assert!(
        !entry.contains("export const HelperUtils =") && !entry.contains("__systemjs_export"),
        "a repeated public name must not leave the export on a stale value:\n{entry}"
    );
    assert_eq!(
        entry.matches("makeValue()").count(),
        1,
        "makeValue should be evaluated once:\n{entry}"
    );
    assert_eq!(
        entry.matches("makeOther()").count(),
        1,
        "makeOther should be evaluated once:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "the reconstructed live export should remain parseable:\n{entry}"
    );
}

#[test]
fn member_export_does_not_capture_a_free_reference() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Widget", function () {
        return Widget;
      }()).ready = true;
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export { __systemjs_export as Widget };")
            && entry.contains("return Widget;")
            && entry.contains("__systemjs_export.ready = true;"),
        "a free Widget read must keep resolving outside the recovered module binding:\n{entry}"
    );
    assert!(
        !entry.contains("export const Widget ="),
        "binding Widget around its own initializer would introduce a TDZ capture:\n{entry}"
    );
}

#[test]
fn member_export_does_not_change_direct_eval_scope() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Widget", function () {
        return eval("Widget");
      }()).ready = true;
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export { __systemjs_export as Widget };")
            && entry.contains("return eval(\"Widget\");"),
        "a direct eval must not see a newly introduced Widget binding:\n{entry}"
    );
    assert!(
        !entry.contains("export const Widget ="),
        "direct eval makes otherwise-free export names capture-sensitive:\n{entry}"
    );
}

#[test]
fn member_export_avoids_lifted_binding_collisions() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      const { Widget } = source;
      for (var Gadget of gadgets) {
        consume(Gadget);
      }
      _export("Widget", makeValue()).ready = true;
      _export("Gadget", makeGadget()).ready = true;
      consume(Widget);
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("const { Widget } = source;")
            && entry.contains("export { __systemjs_export as Widget };")
            && entry.contains("__systemjs_export.ready = true;")
            && entry.contains("export { __systemjs_export_2 as Gadget };")
            && entry.contains("__systemjs_export_2.ready = true;"),
        "destructured and nested var bindings should force collision-free export aliases:\n{entry}"
    );
    assert!(
        !entry.contains("export const Widget =") && !entry.contains("export const Gadget ="),
        "recovered exports must not redeclare lifted bindings:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "the destructuring collision fallback should remain parseable:\n{entry}"
    );
}

#[test]
fn generated_local_name_does_not_hide_same_named_public_export() {
    let source = r#"
System.register("entry", [], function (_export) {
  var X;
  return {
    execute: function () {
      _export("X", makeFirst()).a = 1;
      _export("__systemjs_export", makeSecond()).b = 2;
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export { __systemjs_export as X };")
            && entry.contains("export { __systemjs_export_2 as __systemjs_export };")
            && entry.contains("__systemjs_export_2.b = 2;"),
        "local aliases and public export names must use separate namespaces:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "both public exports should remain parseable:\n{entry}"
    );
}

#[test]
fn member_export_avoids_outer_export_name_collision() {
    let source = r#"
System.register("entry", [], function (_export) {
  var DerivedClass;
  return {
    execute: function () {
      _export("DerivedClass", makeValue()).ready = true;
    }
  };
});
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export { __systemjs_export as DerivedClass };")
            && entry.contains("__systemjs_export.ready = true;"),
        "member export should use a collision-free local alias:\n{entry}"
    );
    assert!(
        !entry.contains("const DerivedClass ="),
        "member export must not redeclare the outer binding:\n{entry}"
    );
}

#[test]
fn reserved_identifier_name_member_export_remains_parseable() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("class", makeValue()).ready = true;
    }
  };
});
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export { __systemjs_export as class };")
            && entry.contains("__systemjs_export.ready = true;"),
        "reserved IdentifierName export should use a parseable alias:\n{entry}"
    );
}

#[test]
fn member_export_value_rewrites_context_import_and_meta() {
    let source = r#"
System.register("entry", [], function (_export, _context) {
  return {
    execute: function () {
      _export("DerivedClass", makeValue(
        _context.import("./dep.js"),
        _context.meta.url
      )).ready = true;
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains(r#"import("./dep.js")"#) && entry.contains("import.meta.url"),
        "member export value should rewrite SystemJS context expressions:\n{entry}"
    );
    assert!(
        !entry.contains("_context"),
        "SystemJS context binding should not leak from the member export value:\n{entry}"
    );
}

#[test]
fn direct_static_class_iife_named_export_stays_supported() {
    let source = r#"
System.register("entry", ["utils"], function (_export) {
  var BaseClass;
  return {
    setters: [function (Utils) {
      BaseClass = Utils.BaseClass;
    }],
    execute: function () {
      _export("DerivedClass", function (BaseClass) {
        function DerivedClass() {}
        DerivedClass.marker = "derived";
        return DerivedClass;
      }(BaseClass));
    }
  };
});
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export const DerivedClass ="),
        "direct named IIFE export should remain a declaration:\n{entry}"
    );
    assert!(
        entry.contains("DerivedClass.marker = \"derived\";"),
        "static class initialization should remain inside the IIFE:\n{entry}"
    );
}

#[test]
fn default_iife_member_export_uses_one_collision_free_binding() {
    let source = r#"
System.register("entry", [], function (_export) {
  var __systemjs_export;
  var BaseClass;
  return {
    execute: function () {
      _export("default", function (BaseClass) {
        function DefaultValue() {}
        return DefaultValue;
      }(BaseClass)).marker = "default";
    }
  };
});
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");
    let binding = entry
        .find("const DefaultValue =")
        .unwrap_or_else(|| panic!("default export should reuse the IIFE return ident:\n{entry}"));
    let member_assignment = entry
        .find("DefaultValue.marker = \"default\";")
        .unwrap_or_else(|| panic!("member assignment should use the binding:\n{entry}"));
    let default_export = entry
        .find("export default DefaultValue;")
        .unwrap_or_else(|| panic!("default export should use the binding:\n{entry}"));

    assert!(
        binding < default_export && default_export < member_assignment,
        "binding, default export, and member assignment should preserve order:\n{entry}"
    );
    assert!(
        !entry.contains("__systemjs_export"),
        "prelude `__systemjs_export` must not become the default local:\n{entry}"
    );
    assert_eq!(
        entry.matches("function DefaultValue()").count(),
        1,
        "default export value should be evaluated once:\n{entry}"
    );
    // Member-assigned `_export("default", IIFE).prop =` must bind once and
    // `export default` that binding. `export default (` is the bare default
    // IIFE path (`default_iife_export_is_parenthesized`), not this shape.
    assert!(
        !entry.contains("export default function") && !entry.contains("export default ("),
        "member-assigned default must export the binding, not the IIFE:\n{entry}"
    );
}

#[test]
fn default_iife_member_export_avoids_module_prelude_names() {
    let source = r#"
!function () {
  var __systemjs_export;
  System.register("entry", [], function (_export) {
    var BaseClass;
    return {
      execute: function () {
        _export("default", function (BaseClass) {
          function DefaultValue() {}
          return DefaultValue;
        }(BaseClass)).ready = true;
      }
    };
  });
}();
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("const DefaultValue =")
            && entry.contains("export default DefaultValue;")
            && entry.contains("DefaultValue.ready = true;"),
        "default export should reuse the IIFE return ident instead of the prelude name:\n{entry}"
    );
    assert!(
        !entry.contains("const __systemjs_export") && !entry.contains("const __systemjs_export_2"),
        "prelude `__systemjs_export` must not be reused as the default local:\n{entry}"
    );
}

#[test]
fn default_iife_member_returned_ident_is_bound() {
    // Reconstruction contract is `--raw`. Decompile may later fold a
    // parameter-less IIFE into `class`, which is a different rule.
    // Member-assigned `_export("default", IIFE).prop =` should reuse the
    // unique returned Identifier when that name is free at module scope.
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function (Base) {
        function DefaultValue() {}
        return DefaultValue;
      }(Base)).marker = "default";
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const DefaultValue =")
            && entry.contains("export default DefaultValue;")
            && entry.contains("DefaultValue.marker = \"default\";"),
        "returned ident must back the default binding:\n{entry}"
    );
    assert!(
        !entry.contains("__systemjs_export")
            && !entry.contains("export default function")
            && !entry.contains("export default ("),
        "must not fall through to the synthetic alias or a default IIFE:\n{entry}"
    );
}

#[test]
fn default_iife_member_minified_return_ident_is_bound() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Named", 1);
      _export("default", function () {
        function e() {}
        return e;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const e =")
            && entry.contains("export default e;")
            && entry.contains("e.marker = true;"),
        "minified return ident must back the default binding:\n{entry}"
    );
    assert!(
        entry.contains("export const Named =") && !entry.contains("__systemjs_export"),
        "sibling named exports must stay and no synthetic alias should appear:\n{entry}"
    );
}

#[test]
fn default_iife_member_callback_name_match_is_binding_safe() {
    // The declare callback and the returned ident may share a minified name.
    // Only proven export-call callees are ignored; the inner function still
    // owns that name, so the module binding is safe.
    let source = r#"
System.register("entry", [], function (e) {
  return {
    execute: function () {
      e("default", function () {
        function e() {}
        return e;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const e =")
            && entry.contains("export default e;")
            && entry.contains("e.marker = true;")
            && !entry.contains("__systemjs_export"),
        "matching callback name must still bind the returned ident:\n{entry}"
    );
}

#[test]
fn default_iife_member_module_collision_falls_back() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      const DefaultValue = 1;
      _export("default", function () {
        function DefaultValue() {}
        return DefaultValue;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const DefaultValue = function"),
        "an existing module local must force the synthetic alias:\n{entry}"
    );
}

#[test]
fn default_iife_member_import_collision_falls_back() {
    // Returned ident `n` is already the setter local for an import.
    let source = r#"
System.register("entry", ["./dep.js"], function (_export) {
  var n;
  return {
    setters: [function (mod) {
      n = mod.legacy;
    }],
    execute: function () {
      _export("default", function () {
        var e, n;
        return (e = (n = function () {}).prototype).init = function () {}, n;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("import { legacy as n }")
            && entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const n = function"),
        "an imported local must force the synthetic alias:\n{entry}"
    );
}

#[test]
fn default_iife_member_typeof_free_name_falls_back() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      observe(typeof e);
      _export("default", function () {
        function e() {}
        return e;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const e ="),
        "a free/typeof name must not be captured by the inferred local:\n{entry}"
    );
}

#[test]
fn default_iife_member_direct_eval_falls_back() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      eval("observe()");
      _export("default", function () {
        function DefaultValue() {}
        return DefaultValue;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const DefaultValue ="),
        "direct eval must keep the synthetic alias:\n{entry}"
    );
}

#[test]
fn default_iife_member_free_arrow_return_falls_back() {
    // `() => { return e }` reads a free `e`. Inventing `const e` would make
    // that return self-referential.
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", (() => {
        return e;
      })()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const e ="),
        "a free arrow return must not become `const e`:\n{entry}"
    );
}

#[test]
fn default_iife_member_surviving_callback_use_falls_back() {
    let source = r#"
System.register("entry", [], function (e) {
  return {
    execute: function () {
      observe(e);
      e("default", function () {
        function e() {}
        return e;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const e ="),
        "a surviving callback reference must keep the name unresolved:\n{entry}"
    );
}

#[test]
fn default_iife_member_repeated_default_stays_mutable() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function () {
        function First() {}
        return First;
      }()).a = 1;
      _export("default", function () {
        function Second() {}
        return Second;
      }()).b = 2;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("let __systemjs_export;")
            && entry.contains("export { __systemjs_export as default };")
            && entry.matches(" as default").count() == 1
            && !entry.contains("const First =")
            && !entry.contains("const Second =")
            && !entry.contains("export default"),
        "repeated default member exports must keep one live binding:\n{entry}"
    );
}

#[test]
fn default_iife_member_seq_tail_ident_is_bound() {
    // Class IIFEs assign prototype methods in the return sequence, then
    // complete with the constructor ident.
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function () {
        function e() {}
        var n = e.prototype;
        return n.method = function () {}, e;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const e =")
            && entry.contains("export default e;")
            && entry.contains("e.marker = true;")
            && !entry.contains("__systemjs_export"),
        "a sequence whose completion value is the ctor ident must bind that ident:\n{entry}"
    );
}

#[test]
fn default_iife_member_comma_return_does_not_infer() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function () {
        function e() {}
        return e, make();
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const e ="),
        "a comma return whose completion value is not an Ident must not infer:\n{entry}"
    );
}

#[test]
fn default_iife_member_nested_return_does_not_infer() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function () {
        function inner() {
          function Nested() {}
          return Nested;
        }
        return inner();
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const Nested =")
            && !entry.contains("const inner ="),
        "a nested function return must not be treated as the outer return:\n{entry}"
    );
}

#[test]
fn default_iife_member_branch_return_does_not_infer() {
    // Returns in the same function, including `if` branches, must all
    // complete with the same Ident. A second Ident fails closed.
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function () {
        function DefaultValue() {}
        if (cond) return Other;
        return DefaultValue;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const __systemjs_export")
            && entry.contains("export default __systemjs_export;")
            && !entry.contains("const DefaultValue =")
            && !entry.contains("const Other ="),
        "divergent branch returns must not infer a writable binding:\n{entry}"
    );
}

#[test]
fn default_iife_member_branch_same_ident_is_bound() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", function () {
        function DefaultValue() {}
        if (cond) return DefaultValue;
        return DefaultValue;
      }()).marker = true;
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const DefaultValue =")
            && entry.contains("export default DefaultValue;")
            && entry.contains("DefaultValue.marker = true;")
            && !entry.contains("__systemjs_export"),
        "the same Ident in every own-function return must still bind:\n{entry}"
    );
}

#[test]
fn default_iife_result_returned_ident_is_bound() {
    // Ident-assign of `_export("default", IIFE())` goes through
    // `export_call_result_items`. Proof must not depend on a sibling
    // member-assign. Nested `use(_export(...))` stays on the existing
    // mutable live-binding path.
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      result = _export("default", function () {
        function DefaultValue() {}
        return DefaultValue;
      }());
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        entry.contains("const DefaultValue =")
            && entry.contains("export default DefaultValue;")
            && entry.contains("result = DefaultValue")
            && !entry.contains("__systemjs_export"),
        "an export-call result IIFE must bind the returned ident:\n{entry}"
    );
}

#[test]
fn repeated_exports_with_different_values_share_one_live_binding() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Utils", makeUtils());
      _export("Utils", Utils);
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("let __systemjs_export;")
            && entry.contains("export { __systemjs_export as Utils };")
            && entry.contains("__systemjs_export = makeUtils()")
            && entry.contains("__systemjs_export = Utils"),
        "different values should update one mutable public export without capturing global Utils:\n{entry}"
    );
    assert_eq!(
        entry.matches("export {").count(),
        1,
        "Utils should have exactly one ESM export specifier:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "the mutable alias path should remain parseable:\n{entry}"
    );
}

#[test]
fn repeated_exports_of_free_value_get_a_declared_live_binding() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Value", external);
      _export("Value", external);
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("export let Value;")
            && entry.matches("Value = external").count() == 2
            && !entry.contains("export { external as Value }"),
        "a free value cannot serve as an ESM local export binding:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "the synthesized live binding should remain parseable:\n{entry}"
    );
}

#[test]
fn repeated_default_member_exports_share_one_live_binding() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("default", makeFirst()).a = 1;
      _export("default", makeSecond()).b = 2;
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("let __systemjs_export;")
            && entry.contains("export { __systemjs_export as default };")
            && entry.contains("(__systemjs_export = makeFirst()).a = 1;")
            && entry.contains("(__systemjs_export = makeSecond()).b = 2;"),
        "default updates should share one mutable live export:\n{entry}"
    );
    assert_eq!(
        entry.matches(" as default").count(),
        1,
        "default should be exported exactly once:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "the live default export should remain parseable:\n{entry}"
    );
}

#[test]
fn typescript_namespace_emit_is_not_double_exported() {
    let source = r#"
System.register("entry", [], function (_export) {
  var Namespace;
  return {
    execute: function () {
      (function (Namespace) {
        Namespace.ready = true;
      })(Namespace || _export("Namespace", Namespace = {}));
      _export("Namespace", Namespace);
    }
  };
});
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert_eq!(
        entry.matches("export const Namespace =").count(),
        1,
        "TypeScript namespace should have one export declaration:\n{entry}"
    );
    assert!(
        entry.contains("ready: true") && !entry.contains("export { Namespace"),
        "TypeScript namespace semantics should remain intact without a trailing duplicate:\n{entry}"
    );
}

#[test]
fn member_export_preserves_computed_assignment_and_sequence_order() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("DerivedClass", makeValue())[getKey()] += rhs(), after();
    }
  };
});
"#;

    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    let binding = entry
        .find("export const DerivedClass = makeValue();")
        .unwrap_or_else(|| panic!("VALUE binding should use the export name:\n{entry}"));
    let assignment = entry
        .find("DerivedClass[getKey()] += rhs();")
        .unwrap_or_else(|| panic!("computed assignment should use the export name:\n{entry}"));
    let rest = entry
        .find("after();")
        .unwrap_or_else(|| panic!("sequence rest should be preserved:\n{entry}"));

    assert!(
        binding < assignment && assignment < rest,
        "export binding, computed assignment, and sequence rest should preserve order:\n{entry}"
    );
    for call in ["makeValue()", "getKey()", "rhs()", "after()"] {
        assert_eq!(
            entry.matches(call).count(),
            1,
            "{call} should appear exactly once:\n{entry}"
        );
    }
    assert!(
        !entry.contains("__systemjs_export"),
        "free export name should not use a synthetic alias:\n{entry}"
    );
}

#[test]
fn nested_iife_export_replacement_remains_parseable() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("DefaultValue", function () {
        function DefaultValue() {}
        return DefaultValue;
      }()).marker();
    }
  };
});
"#;

    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains(".marker();") && !entry.contains("_export"),
        "nested IIFE replacement should remain parseable without leaking the helper:\n{entry}"
    );
}

fn exports_name(entry: &str, name: &str) -> bool {
    entry.contains(&format!("export const {name}"))
        || entry.contains(&format!("export let {name}"))
        || entry.contains(&format!("export var {name}"))
        || entry.contains(&format!("export function {name}"))
        || entry.contains(&format!("export class {name}"))
        || entry.contains(&format!("export {{ {name} }}"))
        || entry.contains(&format!(" as {name}"))
        || entry.contains(&format!("export {{ {name},"))
        || entry.contains(&format!(", {name} }}"))
        || entry.contains(&format!(", {name},"))
}

#[test]
fn var_init_export_sequence_keeps_all_names() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      var last = (_export("Alpha", 1), _export("Beta", ["x"]), _export("Gamma", 2));
      use(last);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        exports_name(entry, "Alpha") && exports_name(entry, "Beta") && exports_name(entry, "Gamma"),
        "all sequence export names must survive:\n{entry}"
    );
    assert!(
        entry.contains("2") && entry.contains("last"),
        "local should keep the last value:\n{entry}"
    );
}

#[test]
fn assign_export_sequence_literal_and_iife_keeps_names() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      h = (_export("Root", "ok"), _export("Widget", function () {
        function Widget() {}
        return Widget;
      }()));
    }
  };
});
"#;
    let raw_modules = unpack_source_raw(source);
    let raw = module_code(&raw_modules, "entry.js");
    assert!(
        exports_name(raw, "Root") && exports_name(raw, "Widget"),
        "literal + IIFE sequence export names must survive:\n{raw}"
    );
    assert!(
        !raw.lines()
            .any(|line| line.trim_start().starts_with("function (")),
        "top-level anonymous function should not be emitted:\n{raw}"
    );

    let modules = unpack_source(source);
    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "decompiled sequence export should stay parseable:\n{}",
        module_code(&modules, "entry.js")
    );
    let decompiled = module_code(&modules, "entry.js");
    assert!(
        exports_name(decompiled, "Root") && exports_name(decompiled, "Widget"),
        "pipeline must keep sequence export names:\n{decompiled}"
    );
}

#[test]
fn assign_export_sequence_uninvoked_ctors_keeps_names() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      w = (_export("First", function (a, b) {
        this.a = a;
        this.b = b;
      }), _export("Second", function (i) {
        this.i = i;
      }));
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        exports_name(entry, "First") && exports_name(entry, "Second"),
        "uninvoked ctor sequence export names must survive:\n{entry}"
    );
}

#[test]
fn mixed_var_call_and_seq_export_keeps_names() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      var c = _export("one", "left"), h = (_export("two", "right"), _export("Box", function () {
        function Box() {}
        return Box;
      }()));
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        exports_name(entry, "one") && exports_name(entry, "two") && exports_name(entry, "Box"),
        "mixed Call + Seq declarators must keep all export names:\n{entry}"
    );
}

#[test]
fn var_export_sequence_preserves_declarator_evaluation_order() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      var first = before(), middle = (_export("Named", during()), finish()), last = after();
      use(first, middle, last);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    let before = entry
        .find("before()")
        .unwrap_or_else(|| panic!("first declarator should survive:\n{entry}"));
    let during = entry
        .find("during()")
        .unwrap_or_else(|| panic!("export value should survive:\n{entry}"));
    let finish = entry
        .find("finish()")
        .unwrap_or_else(|| panic!("sequence result should survive:\n{entry}"));
    let after = entry
        .find("after()")
        .unwrap_or_else(|| panic!("last declarator should survive:\n{entry}"));

    assert!(
        before < during && during < finish && finish < after,
        "splitting a sequence initializer must preserve declarator evaluation order:\n{entry}"
    );
    for call in ["before()", "during()", "finish()", "after()"] {
        assert_eq!(
            entry.matches(call).count(),
            1,
            "{call} should be evaluated exactly once:\n{entry}"
        );
    }
}

#[test]
fn assign_export_sequence_uses_last_identifier_value() {
    let source = r#"
System.register("entry", [], function (_export) {
  var result, value;
  return {
    execute: function () {
      result = (_export("First", first()), _export("Named", value));
      use(result);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains("result = value;") && !entry.contains("result = Named;"),
        "assignment must use the exported call's local value, not its public name:\n{entry}"
    );
    assert!(
        exports_name(entry, "First") && exports_name(entry, "Named"),
        "both exports should survive:\n{entry}"
    );
}

#[test]
fn assign_export_sequence_uses_last_assignment_value() {
    let source = r#"
System.register("entry", [], function (_export) {
  var result, value;
  return {
    execute: function () {
      result = (_export("First", first()), _export("Named", value = makeValue()));
      use(result);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    let first = entry
        .find("first()")
        .unwrap_or_else(|| panic!("prefix export should survive:\n{entry}"));
    let value = entry
        .find("value = makeValue();")
        .unwrap_or_else(|| panic!("exported assignment should survive:\n{entry}"));
    let result = entry
        .find("result = value;")
        .unwrap_or_else(|| panic!("outer assignment should use the assigned value:\n{entry}"));
    let use_result = entry
        .find("use(result);")
        .unwrap_or_else(|| panic!("following use should survive:\n{entry}"));

    assert!(
        first < value && value < result && result < use_result,
        "nested and outer assignments must preserve evaluation order:\n{entry}"
    );
    assert_eq!(
        entry.matches("makeValue()").count(),
        1,
        "the assigned export value should be evaluated exactly once:\n{entry}"
    );
    assert!(
        exports_name(entry, "First") && exports_name(entry, "Named"),
        "both exports should survive:\n{entry}"
    );
}

#[test]
fn assign_export_sequence_default_value_is_evaluated_once() {
    let source = r#"
System.register("entry", [], function (_export) {
  var result;
  return {
    execute: function () {
      result = (_export("First", first()), _export("default", makeValue()));
      use(result);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert_eq!(
        entry.matches("makeValue()").count(),
        1,
        "the default export and assignment must share one evaluated value:\n{entry}"
    );
    assert!(
        entry.contains("export default") && !entry.contains("_export("),
        "the default export should be reconstructed without leaking the runtime helper:\n{entry}"
    );
}

#[test]
fn assign_export_sequence_avoids_local_and_reserved_name_collisions() {
    let source = r#"
System.register("entry", [], function (_export) {
  var Named, namedResult, reservedResult;
  return {
    execute: function () {
      namedResult = (_export("First", first()), _export("Named", makeNamed()));
      reservedResult = (_export("Second", second()), _export("class", makeReserved()));
      use(namedResult, reservedResult);
    }
  };
});
"#;
    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "sequence exports must not introduce duplicate or reserved declarations:\n{entry}"
    );
    assert!(
        !entry.contains("export const Named =") && !entry.contains("export const class ="),
        "colliding and reserved export names should use local aliases:\n{entry}"
    );
    assert!(
        exports_name(entry, "Named") && exports_name(entry, "class"),
        "both public export names should survive through aliases:\n{entry}"
    );
}

#[test]
fn assign_export_sequence_prefix_avoids_local_and_reserved_name_collisions() {
    let source = r#"
System.register("entry", [], function (_export) {
  var PrefixName, namedResult, reservedResult;
  return {
    execute: function () {
      namedResult = (_export("PrefixName", makeNamed()), _export("NamedTail", namedTail()));
      reservedResult = (_export("while", makeReserved()), _export("ReservedTail", reservedTail()));
      use(namedResult, reservedResult);
    }
  };
});
"#;
    let modules = unpack_source(source);
    let entry = module_code(&modules, "entry.js");

    assert_eq!(
        validate_output_modules(&modules),
        vec![],
        "prefix sequence exports must not introduce duplicate or reserved declarations:\n{entry}"
    );
    assert!(
        !entry.contains("export const PrefixName =") && !entry.contains("export const while ="),
        "colliding and reserved prefix names should use local aliases:\n{entry}"
    );
    for name in ["PrefixName", "NamedTail", "while", "ReservedTail"] {
        assert!(
            exports_name(entry, name),
            "public export {name} should survive through an alias:\n{entry}"
        );
    }
}

#[test]
fn assign_export_sequence_prefix_rewrites_context_values() {
    let source = r#"
System.register("entry", [], function (_export, _context) {
  var result;
  return {
    execute: function () {
      result = (_export("Named", makeValue(
        _context.import("./dep.js"),
        _context.meta.url
      )), finish());
      use(result);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");

    assert!(
        entry.contains(r#"import("./dep.js")"#) && entry.contains("import.meta.url"),
        "prefix export values should rewrite SystemJS context expressions:\n{entry}"
    );
    assert!(
        !entry.contains("_context"),
        "the SystemJS context binding must not leak from a sequence export:\n{entry}"
    );
}

#[test]
fn assign_export_sequence_trailing_iife_keeps_prefix_name() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      last = (_export("Kind", function (e) {
        return e[e.Left = 1] = "Left", e;
      }({})), function (Base) {
        function Derived() {
          return Base.apply(this, arguments) || this;
        }
        return Derived;
      }(Base));
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        exports_name(entry, "Kind"),
        "prefix _export in a sequence must survive when the last item is a plain IIFE:\n{entry}"
    );
    assert!(
        entry.contains("last") && (entry.contains("Derived") || entry.contains("Base.apply")),
        "trailing IIFE should remain bound to the assign target:\n{entry}"
    );
}

#[test]
fn unrelated_comma_assign_is_not_an_export() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      var h = (1, 2, 3);
      use(h);
    }
  };
});
"#;
    let modules = unpack_source_raw(source);
    let entry = module_code(&modules, "entry.js");
    assert!(
        !entry.contains("export"),
        "plain comma values must not invent exports:\n{entry}"
    );
}

#[test]
fn statement_export_rejects_free_global_reference() {
    // `observe(Widget)` reads a global; `export const Widget` would
    // capture it. The alias path keeps the global read intact.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    observe(Widget);
    _export("Widget", make());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as Widget };"),
        "captured name must export through an alias:\n{entry}"
    );
    assert!(
        entry.contains("observe(Widget)"),
        "the global read must stay untouched:\n{entry}"
    );
}

#[test]
fn statement_export_rejects_reserved_name_binding() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("class", make());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as class };"),
        "reserved export names must use an alias binding:\n{entry}"
    );
    assert!(
        !entry.contains("const class"),
        "must not declare a reserved-word binding:\n{entry}"
    );
}

#[test]
fn statement_export_rejects_existing_local_collision() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    var Widget = 1;
    _export("Widget", make());
    use(Widget);
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as Widget };"),
        "colliding name must export through an alias:\n{entry}"
    );
    assert!(
        entry.contains("use(Widget)"),
        "the existing local must keep its uses:\n{entry}"
    );
}

#[test]
fn statement_export_binds_free_name_directly() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("Widget", make());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export const Widget = make();"),
        "a provably free name binds directly:\n{entry}"
    );
}

#[test]
fn parenthesized_direct_eval_blocks_direct_binding() {
    // `(eval)(code)` is still a direct eval; the exported name must not
    // become a module binding it could observe.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    (eval)(code);
    _export("Widget", make());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as Widget };"),
        "direct eval must force the alias path:\n{entry}"
    );
}

#[test]
fn sequence_result_export_respects_freedom_proof() {
    // The sequence path keeps `_export`'s return value; the name it binds
    // must pass the same freedom proof as every other direct binding.
    let source = r#"
System.register([], function (_export, _context) {
  var result;
  return { setters: [], execute: function () {
    eval("Widget");
    result = (_export("Widget", make()), finish());
    use(result);
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as Widget };"),
        "direct eval must force the alias path in sequences too:\n{entry}"
    );
    assert!(
        !entry.contains("export const Widget"),
        "must not bind a name a direct eval could observe:\n{entry}"
    );
}

#[test]
fn assign_of_export_literal_keeps_name_and_return_value() {
    // `S = _export("TodayShow", "TodayShow")` must emit the export and keep
    // `_export`'s return value for the assignment (not drop the name).
    let source = r#"
System.register([], function (_export, _context) {
  var S;
  return { setters: [], execute: function () {
    S = _export("TodayShow", "TodayShow");
    use(S);
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export const TodayShow = \"TodayShow\"")
            || entry.contains("export const TodayShow = 'TodayShow'"),
        "literal UNIQUE export must be emitted:\n{entry}"
    );
    assert!(
        entry.contains("S = TodayShow") || entry.contains("S=TodayShow"),
        "assignment must keep `_export`'s return value:\n{entry}"
    );
}

#[test]
fn seq_assign_export_literal_then_named_export_keeps_both_names() {
    // Activity51Popup shape: `S = _export("TodayShow", "TodayShow"), _export("Popup", ctor)`.
    let source = r#"
System.register([], function (_export, _context) {
  var S;
  return { setters: [], execute: function () {
    S = _export("TodayShow", "TodayShow"), _export("Popup", function Popup() {});
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export const TodayShow = \"TodayShow\"")
            || entry.contains("export const TodayShow = 'TodayShow'"),
        "TodayShow must be a bound export, not only a local mention:\n{entry}"
    );
    assert!(
        entry.contains("export const Popup")
            || (entry.contains("export {") && entry.contains("Popup")),
        "following named export must survive:\n{entry}"
    );
}

#[test]
fn assign_of_export_literal_respects_freedom_proof() {
    let source = r#"
System.register([], function (_export, _context) {
  var S;
  return { setters: [], execute: function () {
    eval("TodayShow");
    S = _export("TodayShow", "TodayShow");
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as TodayShow }"),
        "direct eval must force the alias path:\n{entry}"
    );
    assert!(
        !entry.contains("export const TodayShow"),
        "must not bind a name a direct eval could observe:\n{entry}"
    );
}

#[test]
fn assign_of_export_literal_rejects_reserved_name() {
    let source = r#"
System.register([], function (_export, _context) {
  var S;
  return { setters: [], execute: function () {
    S = _export("class", "class");
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as class }"),
        "reserved export names must stay on the alias path:\n{entry}"
    );
    assert!(
        !entry.contains("export const class"),
        "must not emit a reserved binding:\n{entry}"
    );
}

#[test]
fn nested_export_literal_expression_keeps_name() {
    // Expression-position `_export` (not a statement) still has to emit.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    use(_export("TodayShow", "TodayShow"));
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export const TodayShow")
            || entry.contains("export let TodayShow")
            || entry.contains("export {") && entry.contains("TodayShow"),
        "nested `_export` literal must still emit the name:\n{entry}"
    );
}

#[test]
fn expression_export_preserves_sibling_evaluation_order() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    use(before(), _export("TodayShow", make()), after());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    let use_line = entry
        .lines()
        .find(|line| line.contains("use(before()"))
        .unwrap_or_else(|| panic!("expected the original call expression:\n{entry}"));
    let before = use_line.find("before()").unwrap();
    let make = use_line
        .find("make()")
        .unwrap_or_else(|| panic!("export value must stay in the argument position:\n{entry}"));
    let after = use_line.find("after()").unwrap();
    assert!(
        before < make && make < after,
        "must preserve sibling argument evaluation order:\n{entry}"
    );
    assert!(
        !entry.contains("_export("),
        "the SystemJS helper must be fully reconstructed:\n{entry}"
    );
}

#[test]
fn babel_arbitrary_name_live_update_stays_conditional() {
    // Babel output for:
    //   let x = 0;
    //   export { x as "x-y" };
    //   cond() && x++;
    let source = r#"
System.register([], function (_export, _context) {
  "use strict";
  var x;
  return { setters: [], execute: function () {
    _export("x-y", x = 0);
    cond() && (_export("x-y", +x + 1), x++);
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    let conditional_offset = entry
        .find("cond() &&")
        .unwrap_or_else(|| panic!("expected the conditional update:\n{entry}"));
    let conditional_line = entry[conditional_offset..].lines().next().unwrap();
    assert!(
        conditional_line.contains("= +x + 1") && conditional_line.contains("x++"),
        "the live export update must remain inside the condition:\n{entry}"
    );
    assert!(
        !entry[..conditional_offset].contains("+x + 1"),
        "the export value must not be evaluated before the condition:\n{entry}"
    );
    assert_eq!(
        entry.matches("x-y").count(),
        1,
        "must emit exactly one public export for the arbitrary name:\n{entry}"
    );
    assert!(
        !entry.contains("_export("),
        "the SystemJS helper must be fully reconstructed:\n{entry}"
    );
}

#[test]
fn babel_nested_alias_exports_reconstruct_both_names() {
    // Babel output for:
    //   const x = make();
    //   export { x as a, x as b };
    let source = r#"
System.register([], function (_export, _context) {
  "use strict";
  var x;
  return { setters: [], execute: function () {
    _export("b", _export("a", x = make()));
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("x as a") && entry.contains("x as b"),
        "both producer-generated aliases must be reconstructed:\n{entry}"
    );
    assert_eq!(
        entry.matches("make()").count(),
        1,
        "the shared initializer must be evaluated exactly once:\n{entry}"
    );
    assert!(
        !entry.contains("_export("),
        "no nested SystemJS helper call may remain in ESM output:\n{entry}"
    );
}

fn has_string_export_alias(code: &str, exported: &str) -> bool {
    code.contains(&format!("as \"{exported}\"")) || code.contains(&format!("as '{exported}'"))
}

#[test]
fn expr_export_invalid_ident_name_uses_string_alias() {
    // A name that cannot be an Identifier must still appear on the export
    // list. Peeling `_export` and leaving only the value drops the name.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    use(_export("foo-bar", "x"));
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        has_string_export_alias(entry, "foo-bar") && entry.contains("__systemjs_export"),
        "invalid ident export names must use a string alias:\n{entry}"
    );
    assert!(
        !entry.contains("export const foo"),
        "must not invent a legal binding from an illegal export name:\n{entry}"
    );
    assert!(
        !entry.contains("_export("),
        "the SystemJS helper must not remain after a successful alias:\n{entry}"
    );
}

#[test]
fn statement_export_invalid_ident_name_uses_string_alias() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("foo-bar", "x");
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        has_string_export_alias(entry, "foo-bar") && entry.contains("__systemjs_export"),
        "statement `_export` with an illegal ident must still alias:\n{entry}"
    );
    assert!(
        !entry.contains("export const foo"),
        "must not invent a legal binding from an illegal export name:\n{entry}"
    );
    assert!(
        !entry.contains("_export("),
        "the SystemJS helper must not remain after a successful alias:\n{entry}"
    );
}

#[test]
fn assign_of_export_literal_rejects_existing_local() {
    let source = r#"
System.register([], function (_export, _context) {
  var S;
  return { setters: [], execute: function () {
    const TodayShow = 1;
    S = _export("TodayShow", "TodayShow");
    use(S, TodayShow);
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as TodayShow }"),
        "an existing local must force the alias path:\n{entry}"
    );
    assert!(
        !entry.contains("export const TodayShow"),
        "must not rebind an existing local as the export:\n{entry}"
    );
}

#[test]
fn assign_of_export_literal_ignores_cjs_hasownproperty_string() {
    // `exports.hasOwnProperty("TodayShow")` is a CJS surface read. SystemJS
    // `_export` does not own that object; a string key must not block a free name.
    let source = r#"
System.register([], function (_export, _context) {
  var S;
  return { setters: [], execute: function () {
    exports.hasOwnProperty("TodayShow");
    S = _export("TodayShow", "TodayShow");
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export const TodayShow = \"TodayShow\"")
            || entry.contains("export const TodayShow = 'TodayShow'"),
        "a CJS hasOwnProperty string must not force the alias path:\n{entry}"
    );
}

#[test]
fn setter_reexport_of_imported_member_becomes_named_export() {
    // `local = m.Name, _export("Name", m.Name)` used to fail the setter
    // parser and leave the whole register untouched.
    let source = r#"
System.register("dep", [], function (_export) {
  return {
    execute: function () {
      function AuctionLedgerType() {}
      _export("AuctionLedgerType", AuctionLedgerType);
    }
  };
});
System.register("entry", ["dep"], function (_export) {
  var Type;
  return {
    setters: [function (module) {
      Type = module.AuctionLedgerType, _export("AuctionLedgerType", module.AuctionLedgerType);
    }],
    execute: function () {
      _export("Entry", Type);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "setter re-export must not fail the whole module:\n{entry}"
    );
    assert!(
        entry.contains("AuctionLedgerType") && entry.contains(r#"from "dep""#),
        "the imported binding must be recovered:\n{entry}"
    );
    assert!(
        entry.contains("as AuctionLedgerType")
            || entry.contains("export { Type as AuctionLedgerType }"),
        "setter `_export` must become a live re-export:\n{entry}"
    );
}

#[test]
fn setter_reexport_of_assigned_local_becomes_named_export() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var Type;
  return {
    setters: [function (module) {
      Type = module.AuctionLedgerType;
      _export("AuctionLedgerType", Type);
    }],
    execute: function () {
      use(Type);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { Type as AuctionLedgerType }")
            || entry.contains("export {Type as AuctionLedgerType}"),
        "re-exporting the setter local must stay a live binding:\n{entry}"
    );
    assert!(
        entry.contains("use(Type)"),
        "the imported local must remain in execute:\n{entry}"
    );
}

#[test]
fn setter_reexport_only_uses_export_from_without_local() {
    // Re-export without a setter assignment must not invent `import { Name }`.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      _export("AuctionLedgerType", module.AuctionLedgerType);
    }],
    execute: function () {}
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "a proven setter re-export must unpack:\n{entry}"
    );
    assert!(
        entry.contains(r#"export { AuctionLedgerType } from "dep""#)
            || entry.contains(r#"export {AuctionLedgerType} from "dep""#),
        "assignment-free re-export must be `export {{ Name }} from`:\n{entry}"
    );
    assert!(
        !entry.contains("import {") && !entry.contains("import AuctionLedgerType"),
        "must not synthesize a local binding for the re-export:\n{entry}"
    );
}

#[test]
fn setter_unknown_export_value_preserves_whole_register() {
    let source = r#"
System.register("keep", [], function (_export) {
  return {
    execute: function () {
      _export("value", 1);
    }
  };
});
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      _export("AuctionLedgerType", module.AuctionLedgerType + 1);
    }],
    execute: function () {}
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
    .expect("unrecognized setter export must preserve the whole input");
    assert_eq!(
        output.modules.len(),
        1,
        "unrecognized setter must not emit a partial module set: {:?}",
        output.modules
    );
    assert!(
        output.modules[0].1.contains(r#"System.register("keep""#)
            && output.modules[0]
                .1
                .contains(r#"_export("AuctionLedgerType", module.AuctionLedgerType + 1)"#),
        "fallback module should preserve both register calls:\n{}",
        output.modules[0].1
    );
    assert!(
        output.detected_formats.is_empty(),
        "unrecognized setter input should not be reported as a successful split"
    );
}

#[test]
fn setter_bulk_export_preserves_whole_register() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      _export({ AuctionLedgerType: module.AuctionLedgerType });
    }],
    execute: function () {}
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
    .expect("bulk setter export must preserve the whole input");
    assert_eq!(output.modules.len(), 1);
    assert!(
        output.modules[0].1.contains("System.register"),
        "bulk setter `_export` is not a proven shape:\n{}",
        output.modules[0].1
    );
}

#[test]
fn setter_param_shadowing_export_preserves_whole_register() {
    // The setter parameter shadows `_export`, so `e(...)` is a call on the
    // module namespace, not a re-export.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (_export) {
      _export("AuctionLedgerType", _export.AuctionLedgerType);
    }],
    execute: function () {}
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
    .expect("shadowed setter `_export` must preserve the whole input");
    assert_eq!(output.modules.len(), 1);
    assert!(
        output.modules[0].1.contains("System.register"),
        "a setter that shadows `_export` must stay fail-closed:\n{}",
        output.modules[0].1
    );
}

#[test]
fn setter_reexport_only_does_not_invent_local_visible_to_eval() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      _export("AuctionLedgerType", module.AuctionLedgerType);
    }],
    execute: function () {
      eval("AuctionLedgerType");
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains(r#"from "dep""#),
        "the re-export must still unpack:\n{entry}"
    );
    assert!(
        !entry.contains("import {") && !entry.contains("import AuctionLedgerType"),
        "eval must not observe a synthesized import local:\n{entry}"
    );
    assert!(
        entry.contains("eval("),
        "direct eval in execute must remain:\n{entry}"
    );
}

#[test]
fn setter_reexport_preserves_arbitrary_export_name() {
    // Babel output for `export { foo as "foo-bar" } from "./dep.js";`.
    let source = r#"
System.register(["./dep.js"], function (_export, _context) {
  "use strict";

  return {
    setters: [function (_depJs) {
      _export("foo-bar", _depJs.foo);
    }],
    execute: function () {}
  };
});
"#;

    for (stage, modules) in [
        ("raw", unpack_source_raw(source)),
        ("decompiled", unpack_source(source)),
    ] {
        assert_eq!(modules.len(), 1, "unexpected {stage} modules: {modules:?}");
        let entry = &modules[0].1;
        let mut validation_modules = modules.clone();
        validation_modules.push(("dep.js".to_string(), "export const foo = 1;".to_string()));
        assert_eq!(
            validate_output_modules(&validation_modules),
            vec![],
            "{stage} output must remain valid ESM:\n{entry}"
        );
        assert!(
            entry.contains(r#"export { foo as "foo-bar" } from "./dep.js";"#),
            "{stage} output must preserve the arbitrary exported name:\n{entry}"
        );
    }
}

#[test]
fn setter_reexport_preserves_arbitrary_import_name() {
    // Babel output for `export { "foo-bar" as fooBar } from "./dep.js";`.
    let source = r#"
System.register(["./dep.js"], function (_export, _context) {
  "use strict";

  return {
    setters: [function (_depJs) {
      _export("fooBar", _depJs["foo-bar"]);
    }],
    execute: function () {}
  };
});
"#;

    for (stage, modules) in [
        ("raw", unpack_source_raw(source)),
        ("decompiled", unpack_source(source)),
    ] {
        assert_eq!(modules.len(), 1, "unexpected {stage} modules: {modules:?}");
        let entry = &modules[0].1;
        let mut validation_modules = modules.clone();
        validation_modules.push((
            "dep.js".to_string(),
            r#"const foo = 1; export { foo as "foo-bar" };"#.to_string(),
        ));
        assert_eq!(
            validate_output_modules(&validation_modules),
            vec![],
            "{stage} output must remain valid ESM:\n{entry}"
        );
        assert!(
            entry.contains(r#"export { "foo-bar" as fooBar } from "./dep.js";"#),
            "{stage} output must preserve the arbitrary imported name:\n{entry}"
        );
    }
}

fn unpacked_named_module<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    if pairs.len() == 1 && pairs[0].1.contains("System.register") {
        panic!(
            "named setter object must unwrap, not preserve the register:\n{}",
            pairs[0].1
        );
    }
    module_code(pairs, name)
}

fn assert_preserves_whole_system_register(source: &str, needle: &str) {
    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "system-bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unrecognized setter must preserve the whole input");
    assert_eq!(
        output.modules.len(),
        1,
        "unrecognized setter must not emit a partial module set: {:?}",
        output.modules
    );
    assert!(
        output.modules[0].1.contains("System.register") && output.modules[0].1.contains(needle),
        "fallback module should preserve the register:\n{}",
        output.modules[0].1
    );
    assert!(
        output.detected_formats.is_empty(),
        "unrecognized setter input should not be reported as a successful split"
    );
}

#[test]
fn named_setter_object_reexport_uses_export_from_without_local() {
    // Minifiers rewrite `_export("Foo", module.Foo)` into an empty object plus
    // static-key writes, then `_export(ident)`. That is the same proven named
    // re-export, not a bulk object literal.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = module.Foo;
      n["Bar"] = module.Bar;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "named setter object re-export must unpack:\n{entry}"
    );
    assert!(
        entry.contains(r#"from "dep""#) && entry.contains("Foo") && entry.contains("Bar"),
        "each static key must become a named export-from:\n{entry}"
    );
    assert!(
        !entry.contains("import {")
            && !entry.contains("import Foo")
            && !entry.contains("import Bar"),
        "must not synthesize a local binding for the re-export:\n{entry}"
    );
}

#[test]
fn named_setter_object_minified_assign_init_is_named_reexport() {
    // Minifiers emit `(n = {}).Foo = module.Foo` instead of `const n = {}; n.Foo =`.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      var n;
      (n = {}).Foo = module.Foo, n.Bar = module.Bar, _export(n);
    }],
    execute: function () {}
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "minified `(n = {{}}).Foo =` must unpack:\n{entry}"
    );
    assert!(
        entry.contains(r#"from "dep""#) && entry.contains("Foo") && entry.contains("Bar"),
        "minified static keys must stay named re-exports:\n{entry}"
    );
    assert!(
        !entry.contains("import {") && !entry.contains("import Foo"),
        "must not synthesize a local binding for the re-export:\n{entry}"
    );
}

#[test]
fn named_setter_object_minified_cjs_assign_init() {
    let source = r#"
System.register("entry", ["./dep.js", "./loader.js"], function (_export, _context) {
  var meta, loader;
  return {
    setters: [
      function (module) {
        var n;
        meta = module.__cjsMetaURL;
        (n = {}).default = module.default, n.__cjsMetaURL = module.__cjsMetaURL, _export(n);
      },
      function (module) {
        loader = module.default;
      }
    ],
    execute: function () {
      loader.require(meta, _context.meta.url);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "minified CJS static-key wrapper must unpack:\n{entry}"
    );
    assert!(
        entry.contains("default") && entry.contains("__cjsMetaURL"),
        "CJS static keys must keep their names:\n{entry}"
    );
    assert!(
        entry.contains("import.meta.url"),
        "execute `_context.meta` must become import.meta:\n{entry}"
    );
}

#[test]
fn named_setter_object_comma_assigns_are_named_reexports() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      var n = {};
      n.Foo = module.Foo, n.Bar = module.Bar;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "comma-assigned static keys must unpack:\n{entry}"
    );
    assert!(
        entry.contains("Foo") && entry.contains("Bar") && entry.contains(r#"from "dep""#),
        "comma-assigned keys must stay named re-exports:\n{entry}"
    );
}

#[test]
fn named_setter_object_mixed_with_setter_import() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var Type;
  return {
    setters: [function (module) {
      Type = module.Foo;
      const n = {};
      n.Bar = module.Bar;
      _export(n);
    }],
    execute: function () {
      use(Type);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "mixed import + named object must unpack:\n{entry}"
    );
    assert!(
        entry.contains(r#"import { Foo as Type } from "dep""#)
            || entry.contains(r#"import {Foo as Type} from "dep""#),
        "the setter assignment must stay a named import:\n{entry}"
    );
    assert!(
        entry.contains(r#"export { Bar } from "dep""#)
            || entry.contains(r#"export {Bar} from "dep""#),
        "the object key must stay `export {{ Name }} from`:\n{entry}"
    );
    assert!(
        entry.contains("use(Type)"),
        "the imported local must remain in execute:\n{entry}"
    );
}

#[test]
fn named_setter_object_reexport_of_assigned_local() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var Type;
  return {
    setters: [function (module) {
      Type = module.Foo;
      const n = {};
      n.Foo = Type;
      _export(n);
    }],
    execute: function () {
      use(Type);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        entry.contains("export { Type as Foo }") || entry.contains("export {Type as Foo}"),
        "re-exporting the setter local through the temp object must stay live:\n{entry}"
    );
    assert!(
        entry.contains("use(Type)"),
        "the imported local must remain in execute:\n{entry}"
    );
}

#[test]
fn named_setter_object_cjs_static_keys_and_context_meta() {
    let source = r#"
System.register("entry", ["./dep.js", "./loader.js"], function (_export, _context) {
  var meta, loader;
  return {
    setters: [
      function (module) {
        meta = module.__cjsMetaURL;
        const i = {};
        i.default = module.default;
        i.__cjsMetaURL = module.__cjsMetaURL;
        _export(i);
      },
      function (module) {
        loader = module.default;
      }
    ],
    execute: function () {
      if (!meta) {
        loader.throwInvalidWrapper("./dep.js", _context.meta.url);
      }
      loader.require(meta);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert!(
        !entry.contains("System.register"),
        "static-key CJS wrapper must unpack:\n{entry}"
    );
    assert!(
        entry.contains("__cjsMetaURL") && entry.contains("default"),
        "CJS static keys must be re-exported under their original names:\n{entry}"
    );
    assert!(
        entry.contains("import.meta.url"),
        "execute `_context.meta` must become import.meta:\n{entry}"
    );
    assert!(
        !entry.contains("_context"),
        "the declare context param must not leak into execute:\n{entry}"
    );
}

#[test]
fn named_setter_object_and_two_arg_export_unwrap_together() {
    let source = r#"
System.register("barrel", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = module.Foo;
      _export(n);
    }],
    execute: function () {}
  };
});
System.register("plain", [], function (_export) {
  return {
    execute: function () {
      _export("X", 1);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let barrel = unpacked_named_module(&raw, "barrel.js");
    let plain = unpacked_named_module(&raw, "plain.js");
    assert!(
        !barrel.contains("System.register") && !plain.contains("System.register"),
        "both registers must unwrap:\n{barrel}\n{plain}"
    );
    assert!(
        barrel.contains(r#"export { Foo } from "dep""#)
            || barrel.contains(r#"export {Foo} from "dep""#),
        "the named object register must become export-from:\n{barrel}"
    );
    assert!(
        plain.contains("export const X = 1") || plain.contains("export {") && plain.contains("X"),
        "the two-arg `_export` register must still reconstruct:\n{plain}"
    );
}

#[test]
fn named_setter_object_for_in_preserves_whole_register() {
    // `for-in` copies are `export *` (minus default), not proven named keys.
    let source = r#"
System.register("keep", [], function (_export) {
  return {
    execute: function () {
      _export("value", 1);
    }
  };
});
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      for (const p in module) {
        if (p !== "default") n[p] = module[p];
      }
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, r#"System.register("keep""#);
}

#[test]
fn named_setter_object_unknown_value_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = module.Foo + 1;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "module.Foo + 1");
}

#[test]
fn named_setter_object_literal_value_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = 1;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "n.Foo = 1");
}

#[test]
fn named_setter_object_without_empty_decl_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      n.Foo = module.Foo;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "_export(n)");
}

#[test]
fn named_setter_object_unused_temp_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = module.Foo;
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "const n");
}

#[test]
fn named_setter_object_use_after_export_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = module.Foo;
      _export(n);
      use(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "use(n)");
}

#[test]
fn named_setter_object_computed_key_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n[key] = module.Foo;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "n[key]");
}

#[test]
fn named_setter_object_export_spread_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      n.Foo = module.Foo;
      _export(...n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "_export(...n)");
}

#[test]
fn named_setter_object_empty_export_preserves_whole_register() {
    // `_export({})` is not a proven setter shape. An empty temp is the same call.
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "const n");
}

#[test]
fn named_setter_object_rebind_before_export_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      var n = {};
      n = module;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "n = module");
}

#[test]
fn named_setter_object_rebind_after_export_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      var n = {};
      n.Foo = module.Foo;
      _export(n);
      n = module.Bar;
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "n = module.Bar");
}

#[test]
fn named_setter_object_temp_shadows_module_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      var module = {};
      module.Foo = module.Foo;
      _export(module);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "var module");
}

#[test]
fn named_setter_object_shadowed_export_param_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (_export) {
      const n = {};
      n.Foo = _export.Foo;
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "_export(n)");
}

#[test]
fn named_setter_object_assign_spread_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      const n = {};
      Object.assign(n, module);
      _export(n);
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_system_register(source, "Object.assign");
}

#[test]
fn default_iife_export_is_parenthesized() {
    // `_export("default", function () {}())` must stay an expression. Without
    // parens, codegen prints `export default function () {}()`, which is a
    // SyntaxError (function declaration + call).
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("default", function () {
      function Util() {}
      Util.ready = true;
      return Util;
    }());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export default ("),
        "default IIFE must be parenthesized:\n{entry}"
    );
    assert!(
        !entry.contains("export default function"),
        "must not print a default function declaration that is immediately called:\n{entry}"
    );
}

#[test]
fn default_iife_member_value_keeps_expression_context() {
    // Babel 7/8 removes the source parens in `_export`'s argument because the
    // argument is already an expression context. Keep the complete value in
    // an initializer so a member suffix cannot expose a declaration-like head.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("default", function () {
      return { value: 1 };
    }().value);
  } };
});
"#;

    for (label, modules) in [
        ("raw", unpack_source_raw(source)),
        ("decompiled", unpack_source(source)),
    ] {
        let entry = module_code(&modules, "entry.js");
        assert_eq!(
            validate_output_modules(&modules),
            vec![],
            "{label} output must remain parseable:\n{entry}"
        );
        assert!(
            entry.contains("const __systemjs_export =")
                && entry.contains("export default __systemjs_export;"),
            "{label} output must evaluate the member value in an initializer:\n{entry}"
        );
        assert!(
            !entry.contains("export default function"),
            "{label} output must not reinterpret the IIFE as a declaration:\n{entry}"
        );
    }
}

#[test]
fn named_iife_export_binds_const_not_default_declaration() {
    // Named IIFE goes through `export const` (expression context). Parens are
    // required only for `export default`; codegen may drop them on const init.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("Util", function () {
      function Util() {}
      return Util;
    }());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export const Util ="),
        "named IIFE must bind the export:\n{entry}"
    );
    assert!(
        entry.contains("= function") || entry.contains("= (function"),
        "named IIFE must stay a function expression:\n{entry}"
    );
    assert!(
        !entry.contains("export default function"),
        "named IIFE must not fall through to a default function declaration:\n{entry}"
    );
}

#[test]
fn named_default_function_expression_stays_local() {
    // Babel 7/8 emits this shape from `export default (function Named() {})`.
    // The name belongs only to the function expression; emitting a declaration
    // would introduce and hoist a new module-scope `Named` binding.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    observe(typeof Named);
    observe(eval("typeof Named"));
    _export("default", function Named() {
      return 1;
    });
  } };
});
"#;

    for (label, modules) in [
        ("raw", unpack_source_raw(source)),
        ("decompiled", unpack_source(source)),
    ] {
        let entry = module_code(&modules, "entry.js");
        assert_eq!(
            validate_output_modules(&modules),
            vec![],
            "{label} output must remain parseable:\n{entry}"
        );
        assert!(
            entry.contains("const __systemjs_export = function Named")
                && entry.contains("export default __systemjs_export;"),
            "{label} output must keep the named function in an initializer:\n{entry}"
        );
        assert!(
            !entry.contains("export default function Named"),
            "{label} output must not hoist the expression name into module scope:\n{entry}"
        );
    }
}

#[test]
fn named_default_class_expression_stays_local() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    observe(typeof Named);
    observe(eval("typeof Named"));
    _export("default", class Named {
      static value = 1;
    });
  } };
});
"#;

    for (label, modules) in [
        ("raw", unpack_source_raw(source)),
        ("decompiled", unpack_source(source)),
    ] {
        let entry = module_code(&modules, "entry.js");
        assert_eq!(
            validate_output_modules(&modules),
            vec![],
            "{label} output must remain parseable:\n{entry}"
        );
        assert!(
            entry.contains("const __systemjs_export = class Named")
                && entry.contains("export default __systemjs_export;"),
            "{label} output must keep the named class in an initializer:\n{entry}"
        );
        assert!(
            !entry.contains("export default class Named"),
            "{label} output must not hoist the expression name into module scope:\n{entry}"
        );
    }
}

#[test]
fn named_iife_export_respects_direct_eval_freedom_proof() {
    // Parenthesizing the IIFE must not bypass the #208 freedom proof.
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    eval("Util");
    _export("Util", function () {
      function Util() {}
      return Util;
    }());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as Util }"),
        "direct eval must force the alias path:\n{entry}"
    );
    assert!(
        !entry.contains("export const Util"),
        "must not bind a name a direct eval could observe:\n{entry}"
    );
}

#[test]
fn named_iife_export_rejects_reserved_name_binding() {
    let source = r#"
System.register([], function (_export, _context) {
  return { setters: [], execute: function () {
    _export("class", function () {
      return 1;
    }());
  } };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("export { __systemjs_export as class }"),
        "reserved export names must stay on the alias path:\n{entry}"
    );
    assert!(
        !entry.contains("export const class"),
        "must not emit a reserved binding:\n{entry}"
    );
}

fn assert_no_system_register(code: &str, label: &str) {
    assert!(
        !code.contains("System.register"),
        "{label} must unwrap System.register:\n{code}"
    );
}

fn assert_no_invented_export(code: &str, label: &str) {
    assert!(
        !code.contains("export "),
        "{label} must not invent an export:\n{code}"
    );
}

fn assert_no_leftover_export_call(code: &str, label: &str) {
    // Only the unminified declare name. Scanning `e({` / `t({` false-positives
    // named function expressions that recurse (#209).
    assert!(
        !code.contains("_export(") && !code.contains("_export({"),
        "{label} must not leave a free `_export` call:\n{code}"
    );
}

#[test]
fn no_export_param_empty_execute_unwraps_without_export() {
    let source = r#"
System.register("entry", [], function () {
  return {
    execute: function () {}
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert_no_system_register(entry, "zero-arg function factory");
    assert_no_invented_export(entry, "zero-arg function factory");
}

#[test]
fn no_export_param_expression_arrow_empty_shell_unwraps_without_export() {
    // Rollup often emits `() => ({ execute() {} })` for empty chunk shells.
    let source = r#"
System.register("shell", [], () => ({
  execute() {}
}));
"#;
    let raw = unpack_source_raw(source);
    let shell = module_code(&raw, "shell.js");
    assert_no_system_register(shell, "expression-body arrow factory");
    assert_no_invented_export(shell, "expression-body arrow factory");
}

#[test]
fn no_export_param_null_setter_reconstructs_imports() {
    let source = r#"
System.register("entry", ["./side.js", "./dep.js"], function () {
  var component;
  return {
    setters: [
      null,
      function (module) {
        component = module.component;
      }
    ],
    execute: function () {
      use(component);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert_no_system_register(entry, "zero-arg factory with null setter");
    assert!(
        entry.contains(r#"import "./side.js""#) || entry.contains("import \"./side.js\";"),
        "null setter must become a side-effect import:\n{entry}"
    );
    assert!(
        entry.contains(r#"import { component } from "./dep.js""#),
        "named setter binding must become an import:\n{entry}"
    );
    assert!(
        entry.contains("use(component)"),
        "execute must keep using the imported binding:\n{entry}"
    );
    assert_no_invented_export(entry, "zero-arg factory with null setter");
}

#[test]
fn no_export_param_execute_body_keeps_side_effects() {
    // Shape reduced from a real module that never calls `_export` so Terser
    // drops the declare parameter, but still has a non-empty execute body.
    let source = r#"
System.register("entry", ["runtime"], function () {
  var runtime;
  return {
    setters: [function (module) {
      runtime = module.runtime;
    }],
    execute: function () {
      runtime.downloader = runtime.downloader || {};
      runtime.downloader.maxConcurrency = 6;
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert_no_system_register(entry, "zero-arg factory with execute body");
    assert!(
        entry.contains("from \"runtime\"") || entry.contains("from 'runtime'"),
        "import from the setter must be recovered:\n{entry}"
    );
    assert!(
        entry.contains("maxConcurrency = 6"),
        "execute side effects must be lifted:\n{entry}"
    );
    assert_no_invented_export(entry, "zero-arg factory with execute body");
}

#[test]
fn no_export_param_sibling_register_does_not_poison_bundle() {
    let source = r#"
System.register("shell", [], function () {
  return {
    execute: function () {}
  };
});
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("value", 1);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    assert!(
        raw.len() >= 2,
        "a zero-arg sibling must not force a whole-file fallback: {:?}",
        raw.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    let shell = module_code(&raw, "shell.js");
    let entry = module_code(&raw, "entry.js");
    assert_no_system_register(shell, "zero-arg sibling shell");
    assert_no_invented_export(shell, "zero-arg sibling shell");
    assert_no_system_register(entry, "exporting sibling");
    assert!(
        entry.contains("export") && entry.contains("value"),
        "the exporting sibling must still reconstruct:\n{entry}"
    );
}

#[test]
fn no_export_param_unknown_setter_preserves_whole_register() {
    // Unrecognized setter statements stay fail-closed even when declare has
    // no `_export` parameter. This locks `const bag = {}` assignment, not
    // the separate `e(n)` object re-export shape.
    let source = r#"
System.register("keep", [], function (_export) {
  return {
    execute: function () {
      _export("value", 1);
    }
  };
});
System.register("odd", ["dep"], function () {
  return {
    setters: [function (module) {
      const bag = {};
      bag.Name = module.Name;
      use(bag);
    }],
    execute: function () {}
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
    .expect("unrecognized setter must preserve the whole input");
    assert_eq!(
        output.modules.len(),
        1,
        "unrecognized setter must not emit a partial module set: {:?}",
        output.modules
    );
    assert!(
        output.modules[0].1.contains(r#"System.register("keep""#)
            && output.modules[0].1.contains(r#"System.register("odd""#),
        "fallback module should preserve both register calls:\n{}",
        output.modules[0].1
    );
    assert!(
        output.detected_formats.is_empty(),
        "unrecognized setter input should not be reported as a successful split"
    );
}

#[test]
fn no_export_param_does_not_invent_export_from_execute_string() {
    let source = r#"
System.register("IBattleRecord", ["cc"], function () {
  var cclegacy;
  return {
    setters: [function (module) {
      cclegacy = module.cclegacy;
    }],
    execute: function () {
      cclegacy._RF.push({}, "id", "IBattleRecord", undefined);
      cclegacy._RF.pop();
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "IBattleRecord.js");
    assert_no_system_register(entry, "filename/_RF string fixture");
    assert!(
        entry.contains("IBattleRecord"),
        "the execute string must stay a string:\n{entry}"
    );
    assert_no_invented_export(entry, "filename/_RF string fixture");
}

fn assert_preserves_whole_register(source: &str, label: &str) {
    let output = unpack_raw(
        source,
        &DecompileOptions {
            filename: "system-bundle.js".to_string(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|_| panic!("{label} must preserve the whole input"));
    assert_eq!(
        output.modules.len(),
        1,
        "{label} must not emit a partial module set: {:?}",
        output.modules
    );
    assert!(
        output.modules[0].1.contains("System.register"),
        "{label} must stay fail-closed:\n{}",
        output.modules[0].1
    );
    assert!(
        output.detected_formats.is_empty(),
        "{label} should not be reported as a successful split"
    );
}

#[test]
fn unrecognized_declare_rest_param_preserves_whole_register() {
    // A present but unreadable first param is not "no export param".
    let source = r#"
System.register("keep", [], function (_export) {
  return {
    execute: function () {
      _export("value", 1);
    }
  };
});
System.register("odd", [], function (...args) {
  return {
    execute: function () {
      args[0]("value", 1);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "rest declare param");
}

#[test]
fn unrecognized_declare_destructured_param_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function ({ e }) {
  return {
    execute: function () {
      e("value", 1);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "destructured declare param");
}

#[test]
fn unrecognized_declare_default_param_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (e = noop) {
  return {
    execute: function () {
      e("value", 1);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "defaulted declare param");
}

#[test]
fn expression_arrow_setter_stays_fail_closed() {
    // Expression-body arrows are accepted only on the declare factory.
    let source = r#"
System.register("odd", ["./dep.js"], function (_export) {
  var x;
  return {
    setters: [() => (x = m.foo)],
    execute: function () {
      _export("value", x);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "expression-body arrow setter");
}

#[test]
fn execute_object_export_of_functions_becomes_named_exports() {
    // protobuf-ts / engine chunks emit `_export({ assert: function () {} })`.
    // That is the same contract as `_export("assert", function () {})`.
    let source = r#"
System.register("assert", [], function (_export) {
  return {
    execute: function () {
      _export({
        assert: function (e, t) {
          if (!e) throw new Error(t);
        },
        assertInt32: function (t) {
          if (typeof t !== "number") throw new Error("invalid int 32");
        }
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "assert.js");
    assert_no_system_register(entry, "execute object export of functions");
    assert!(
        entry.contains("export const assert =") && entry.contains("export const assertInt32 ="),
        "function values must become export const:\n{entry}"
    );
    assert!(
        !entry.contains("export function assert") && !entry.contains("export function assertInt32"),
        "must not emit export function (Function.name, #204):\n{entry}"
    );
    assert_no_leftover_export_call(entry, "execute object export of functions");
}

#[test]
fn execute_object_export_mixes_function_and_ident() {
    // varint.js: `_export({ int64FromString: function () {}, uInt64ToString: n })`.
    let source = r#"
System.register("varint", [], function (_export) {
  return {
    execute: function () {
      _export({
        foo: function () {
          return n();
        },
        bar: n
      });
      function n() {
        return 1;
      }
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "varint.js");
    assert_no_system_register(entry, "execute object export mix");
    assert!(
        entry.contains("export const foo ="),
        "function value must become export const:\n{entry}"
    );
    assert!(
        entry.contains("export { n as bar }")
            || entry.contains("export {n as bar}")
            || entry.contains("export {n as bar};"),
        "ident value must become a named re-export:\n{entry}"
    );
}

#[test]
fn execute_object_export_named_function_value_is_the_same_shape() {
    // number.js: `_export({ num_to_s: function e(t) { ... } })`.
    let source = r#"
System.register("number", [], function (_export) {
  return {
    execute: function () {
      _export({
        num_to_s: function e(t) {
          if (t < 0) return "-" + e(-t);
          return String(t);
        },
        int_to_s: m
      });
      function m(t) {
        return String(t);
      }
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "number.js");
    assert_no_system_register(entry, "execute object export named function");
    assert!(
        entry.contains("export const num_to_s ="),
        "named function value must become export const:\n{entry}"
    );
    assert!(
        entry.contains("function e") && entry.contains("e(-t)"),
        "the inner function ident must stay in expression scope (#209):\n{entry}"
    );
    assert!(
        entry.contains("export { m as int_to_s }")
            || entry.contains("export {m as int_to_s}")
            || entry.contains("export {m as int_to_s};"),
        "ident sibling must still become a named re-export:\n{entry}"
    );
}

#[test]
fn named_function_expression_recursive_call_is_not_leftover_export() {
    // property.js: `_export("property", function e(t) { return e({ type: void 0 }); })`.
    let source = r#"
System.register("property", [], function (e) {
  return {
    execute: function () {
      e("property", function e(t) {
        if (t == null) return e({ type: void 0 });
        return t;
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "property.js");
    assert_no_system_register(entry, "named function recursive call");
    assert!(
        entry.contains("export const property ="),
        "two-arg named function export must reconstruct:\n{entry}"
    );
}

#[test]
fn named_function_expression_recursive_calls_do_not_invent_bulk_exports() {
    // Rollup + Terser can reuse the declare callback's short name for a named
    // function expression. Its recursive object arguments are ordinary calls,
    // not SystemJS bulk exports.
    let source = r#"
System.register("recurse", [], function (t) {
  return {
    execute: function () {
      t("recurse", function t(e) {
        if (e.left) return 1 + t({ step: e.step + 1, left: false });
        if (e.right) return 2 + t({ step: e.step + 2, right: false });
        return e.step;
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "recurse.js");
    assert_no_system_register(entry, "named recursive calls with object arguments");
    assert!(
        entry.contains("export const recurse ="),
        "the real two-arg export must reconstruct:\n{entry}"
    );
    assert!(
        !entry.contains("export let step") && !entry.contains("export const step"),
        "recursive calls must not invent an export named `step`:\n{entry}"
    );
    assert!(
        entry.matches("t({").count() == 2,
        "both recursive calls must remain calls to the named function:\n{entry}"
    );
}

#[test]
fn mutable_export_rewrite_ignores_named_function_recursive_calls() {
    // The two real `value` exports prepare a mutable ESM binding. A nested
    // function reusing the callback's short name must still recurse normally;
    // the mutable-export pass must not turn its call into `value = 3`.
    let source = r#"
System.register("recurse", [], function (t) {
  return {
    execute: function () {
      t("value", 1);
      t("value", 2);
      t("recurse", function t(name, value) {
        if (name === "value") return value;
        return t("value", 3);
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "recurse.js");
    assert_no_system_register(entry, "mutable export beside named recursion");
    assert!(
        entry.contains("export let value"),
        "the repeated real export must use one mutable binding:\n{entry}"
    );
    assert!(
        entry.contains("return t(\"value\", 3)") && !entry.contains("return value = 3"),
        "the shadowed recursive call must not become an export assignment:\n{entry}"
    );
}

#[test]
fn execute_local_function_shadow_is_not_export_callback() {
    // The execute function is nested under the declare callback. A local with
    // the same short name shadows that callback even for top-level initializers
    // that specialized export paths inspect before the general AST visitor.
    let source = r#"
System.register("local", [], function (t) {
  return {
    execute: function () {
      function t(name, value) {
        return value;
      }
      var result = t("value", 1);
      use(result);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "local.js");
    assert_no_system_register(entry, "execute-local callback shadow");
    assert_no_invented_export(entry, "execute-local callback shadow");
    assert!(
        entry.contains("result = t(\"value\", 1)"),
        "the call to the local function must remain intact:\n{entry}"
    );
}

#[test]
fn nested_fn_expr_param_is_not_string_export() {
    // A nested callback parameter can reuse the factory `_export` short name.
    // That inner call is an ordinary argument, not an export. A same-named
    // execute local must not be rewritten to the imported binding.
    let source = r#"
System.register("widget", ["./helpers.js"], function (e) {
  var n, t;
  return {
    setters: [function (mod) {
      n = mod.helper;
    }],
    execute: function () {
      e("Widget", function () {});
      t = {};
      t.update = function () {
        var n = "01:00";
        this.label.setByFunc(function (e) {
          return t.prefix + "\n\n" + e("MESSAGE_KEY", n);
        });
      };
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "widget.js");
    assert_no_system_register(entry, "nested fn-expr param shadow");
    assert!(
        entry.contains("export") && (entry.contains("Widget") || entry.contains("widget")),
        "the real two-arg export must reconstruct:\n{entry}"
    );
    assert!(
        !entry.contains("as MESSAGE_KEY") && !entry.contains("export const MESSAGE_KEY"),
        "the shadowed callback call must not become an export:\n{entry}"
    );
    assert!(
        entry.contains("e(\"MESSAGE_KEY\", n)") || entry.contains("e('MESSAGE_KEY', n)"),
        "the inner call must remain a call:\n{entry}"
    );
}

#[test]
fn nested_arrow_param_is_not_string_export() {
    // Same contract as a function-expression parameter: an arrow parameter that
    // reuses the factory short name is not the export callback.
    let source = r#"
System.register("widget", ["./helpers.js"], function (e) {
  var n, t;
  return {
    setters: [function (mod) {
      n = mod.helper;
    }],
    execute: function () {
      e("Widget", function () {});
      t = {};
      t.update = function () {
        var n = "01:00";
        this.label.setByFunc((e) => t.prefix + "\n\n" + e("MESSAGE_KEY", n));
      };
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "widget.js");
    assert_no_system_register(entry, "nested arrow param shadow");
    assert!(
        entry.contains("export") && (entry.contains("Widget") || entry.contains("widget")),
        "the real two-arg export must reconstruct:\n{entry}"
    );
    assert!(
        !entry.contains("as MESSAGE_KEY") && !entry.contains("export const MESSAGE_KEY"),
        "the shadowed callback call must not become an export:\n{entry}"
    );
    assert!(
        entry.contains("e(\"MESSAGE_KEY\", n)") || entry.contains("e('MESSAGE_KEY', n)"),
        "the inner call must remain a call:\n{entry}"
    );
}

#[test]
fn execute_object_export_method_shorthand_is_the_same_shape() {
    // Pretty-printers turn `assert: function () {}` into a method.
    let source = r#"
System.register("assert", [], function (_export) {
  return {
    execute: function () {
      _export({
        assert(e, t) {
          if (!e) throw new Error(t);
        }
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "assert.js");
    assert_no_system_register(entry, "execute object export method shorthand");
    assert!(
        entry.contains("export const assert ="),
        "method shorthand must become export const:\n{entry}"
    );
}

#[test]
fn execute_object_export_sibling_two_arg_still_reconstructs() {
    let source = r#"
System.register("plain", [], function (_export) {
  return {
    execute: function () {
      _export("X", 1);
    }
  };
});
System.register("fns", [], function (_export) {
  return {
    execute: function () {
      _export({
        assert: function (e, t) {
          if (!e) throw new Error(t);
        }
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let plain = module_code(&raw, "plain.js");
    let fns = module_code(&raw, "fns.js");
    assert_no_system_register(plain, "sibling two-arg export");
    assert_no_system_register(fns, "sibling object export of functions");
    assert!(
        plain.contains("export const X = 1"),
        "two-arg `_export` must still reconstruct:\n{plain}"
    );
    assert!(
        fns.contains("export const assert ="),
        "object function export must reconstruct next to a two-arg sibling:\n{fns}"
    );
}

#[test]
fn execute_object_export_void0_dummy_only_preserves_whole_register() {
    // A lone dummy object export is not a proven live binding (#206).
    let source = r#"
System.register("field", [], function (_export) {
  return {
    execute: function () {
      _export({ ScalarType: void 0 });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "void 0 dummy only");
}

#[test]
fn execute_object_export_undefined_predeclare_preserves_whole_register() {
    // Pretty-printers rewrite `void 0` to `undefined`. That ident is not a
    // local and must not become `export { undefined as ScalarType }`.
    let source = r#"
System.register("field", [], function (_export) {
  return {
    execute: function () {
      _export({ LongType: undefined, ScalarType: undefined });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "undefined dummy predeclare");
}

#[test]
fn execute_object_export_unbound_ident_preserves_whole_register() {
    // `window` is not a module local. Do not invent `export { window as bar }`.
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({ bar: window });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "unbound ident object export");
}

#[test]
fn execute_object_export_void0_dummy_drops_and_keeps_later_singles() {
    // field.js: drop the dummy object, reconstruct later `_export("Name", {})`.
    // Do not leave a free `_export({...})` and do not lift the dummy itself.
    let source = r#"
System.register("field", [], function (_export) {
  return {
    execute: function () {
      var t, n;
      _export({ LongType: void 0, ScalarType: void 0 });
      (function (e) {
        e[e.DOUBLE = 1] = "DOUBLE";
      })(t || (t = _export("ScalarType", {})));
      (function (e) {
        e[e.BIGINT = 0] = "BIGINT";
      })(n || (n = _export("LongType", {})));
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "field.js");
    assert_no_system_register(entry, "void0 dummy plus later singles");
    assert_no_leftover_export_call(entry, "void0 dummy plus later singles");
    assert!(
        entry.contains("ScalarType") && entry.contains("LongType"),
        "later two-arg fills must reconstruct:\n{entry}"
    );
}

#[test]
fn execute_object_export_comma_enum_iife_is_parenthesized() {
    // protobuf-es v1 ScalarType/LongType: dummy object + comma-operator enum
    // IIFE. The IIFE is legal in expression position; after live-binding
    // splits the sequence it must stay parenthesized as a statement.
    let source = r#"
System.register("field", [], function (_export) {
  return {
    execute: function () {
      var t, n;
      _export({ LongType: void 0, ScalarType: void 0 }),
      function (e) {
        e[e.DOUBLE = 1] = "DOUBLE";
      }(t || (t = _export("ScalarType", {}))),
      function (e) {
        e[e.BIGINT = 0] = "BIGINT";
      }(n || (n = _export("LongType", {})));
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "field.js");
    assert_no_system_register(entry, "comma enum IIFE");
    assert_no_leftover_export_call(entry, "comma enum IIFE");
    assert!(
        entry.contains("ScalarType") && entry.contains("LongType"),
        "live bindings must reconstruct:\n{entry}"
    );
    assert!(
        entry.contains("DOUBLE = 1") && entry.contains("BIGINT = 0"),
        "enum values must survive:\n{entry}"
    );
    assert!(
        entry.contains("(function"),
        "lifted enum IIFE must stay parenthesized:\n{entry}"
    );
    assert!(
        !has_bare_function_stmt(entry),
        "must not emit a statement-level `function (`:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "comma enum IIFE raw");

    let decompiled = unpack_source(source);
    assert_valid_unpacked_esm(&decompiled, "comma enum IIFE decompiled");
}

fn has_bare_function_stmt(code: &str) -> bool {
    code.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("function(") || trimmed.starts_with("function (")
    })
}

#[test]
fn lifted_object_and_assignment_heads_keep_expression_context() {
    let cases = [
        (
            "object-headed call",
            r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Before", 0),
      { [key()]: handler }[lookup()](),
      _export("After", 1);
    }
  };
});
"#,
        ),
        (
            "function-headed assignment",
            r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Before", 0),
      function () { return {}; }().value = side(),
      _export("After", 1);
    }
  };
});
"#,
        ),
        (
            "source-parenthesized object-headed call",
            r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export("Before", 0),
      ({ [key()]: handler }[lookup()]()),
      _export("After", 1);
    }
  };
});
"#,
        ),
    ];

    for (label, source) in cases {
        for (stage, modules) in [
            ("raw", unpack_source_raw(source)),
            ("decompiled", unpack_source(source)),
        ] {
            let entry = module_code(&modules, "entry.js");
            assert_no_system_register(entry, label);
            assert_no_leftover_export_call(entry, label);
            assert!(
                entry.contains("Before") && entry.contains("After"),
                "{label}/{stage} must retain both exports:\n{entry}"
            );
            assert_valid_unpacked_esm(&modules, &format!("{label}/{stage}"));
        }
    }
}

#[test]
fn lifted_string_operand_does_not_become_a_directive() {
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      "use client", _export("Value", 1);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert!(
        entry.contains("(\"use client\");") || entry.contains("('use client');"),
        "a comma operand must not become a directive after lifting:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "lifted string operand");
}

#[test]
fn lifted_iife_prefix_is_parenthesized_in_assignment_and_initializer_sequences() {
    let cases = [
        (
            "assignment sequence",
            r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      var result;
      result = (function (value) { value.hit = 1; }({}), _export("After", 1));
    }
  };
});
"#,
        ),
        (
            "initializer sequence",
            r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      var result = (function (value) { value.hit = 1; }({}), _export("After", 1));
    }
  };
});
"#,
        ),
    ];

    for (label, source) in cases {
        for (stage, modules) in [
            ("raw", unpack_source_raw(source)),
            ("decompiled", unpack_source(source)),
        ] {
            let entry = module_code(&modules, "entry.js");
            assert!(
                entry.contains("hit = 1") && entry.contains("After"),
                "{label}/{stage} must retain the IIFE and export:\n{entry}"
            );
            assert_valid_unpacked_esm(&modules, &format!("{label}/{stage}"));
        }
    }
}

#[test]
fn execute_object_export_computed_key_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({ [key]: function () {} });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "computed key object export");
}

#[test]
fn execute_object_export_getter_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        get foo() {
          return 1;
        }
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "getter object export");
}

#[test]
fn execute_object_export_quoted_name_uses_string_alias() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        "foo-bar": function () {
          return 1;
        }
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "odd.js");
    assert_no_system_register(entry, "quoted object export name");
    assert!(
        entry.contains("foo-bar") && (entry.contains("export {") || entry.contains("export{")),
        "illegal ident must use a string alias:\n{entry}"
    );
    assert!(
        !entry.contains("export {") || entry.contains("\"foo-bar\"") || entry.contains("'foo-bar'"),
        "the public name must stay quoted (#211):\n{entry}"
    );
    assert!(
        entry.contains("[\"foo-bar\"]: function") && entry.contains("}[\"foo-bar\"]"),
        "the alias initializer must preserve the anonymous function's inferred name:\n{entry}"
    );
}

#[test]
fn execute_object_export_reserved_name_uses_alias() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        class: function () {
          return 1;
        }
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "odd.js");
    assert_no_system_register(entry, "reserved object export name");
    assert!(
        !entry.contains("export const class"),
        "must not bind a reserved word:\n{entry}"
    );
    assert!(
        entry.contains("class"),
        "the public name must still be exported:\n{entry}"
    );
    assert!(
        entry.contains("[\"class\"]: function") && entry.contains("}[\"class\"]"),
        "the alias initializer must preserve the anonymous function's inferred name:\n{entry}"
    );
}

#[test]
fn execute_object_export_mixed_fn_and_void0_preserves_whole_register() {
    // One unlowerable pair fails the whole object. Do not emit half an export
    // list plus leftover `_export({ bad: void 0 })`.
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        ok: function () {
          return 1;
        },
        bad: void 0
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "mixed restorable and void 0");
}

#[test]
fn execute_object_export_mixed_ident_and_void0_preserves_whole_register() {
    // Ident pairs call `add_export` before a later void 0 fails. The whole
    // object must roll back; do not emit `export { n as bar }` alone.
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        bar: n,
        bad: void 0
      });
      function n() {
        return 1;
      }
    }
  };
});
"#;
    assert_preserves_whole_register(source, "mixed ident and void 0");
}

#[test]
fn execute_object_export_mixed_assign_and_void0_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      var n;
      _export({
        bar: (n = 1),
        bad: void 0
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "mixed assign and void 0");
}

#[test]
fn execute_object_export_same_name_bulk_then_single_uses_live_binding() {
    // Handbook §3: one public name, one live binding. A Bulk function then
    // `_export("foo", 2)` must not emit `export const` plus a second export.
    let source = r#"
System.register("entry", [], function (_export) {
  return {
    execute: function () {
      _export({
        foo: function () {
          return 1;
        }
      });
      _export("foo", 2);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "entry.js");
    assert_no_system_register(entry, "bulk then single same name");
    assert_no_leftover_export_call(entry, "bulk then single same name");
    assert!(
        entry.contains("export let foo"),
        "repeated public name must share one live binding:\n{entry}"
    );
    assert!(
        !entry.contains("export const foo"),
        "must not emit a second const export for the same name:\n{entry}"
    );
    assert_eq!(
        validate_output_modules(&raw),
        vec![],
        "same-name bulk then single must stay legal ESM:\n{entry}"
    );
}

#[test]
fn execute_object_export_in_and_expr_preserves_whole_register() {
    // Expression-position Bulk is not a top-level drop. Leaving `_export({`
    // would be illegal ESM; fail-closed instead.
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      cond && _export({
        foo: function () {
          return 1;
        }
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "expression-position object export");
}

#[test]
fn execute_object_export_assign_rhs_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      var x;
      x = _export({
        foo: function () {
          return 1;
        }
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "assignment-rhs object export");
}

#[test]
fn execute_object_export_void0_with_import_only_preserves_whole_register() {
    // Import is not an export surface. Dropping the dummy must not unwrap
    // a module that never reconstructed a public name (#206 dual).
    let source = r#"
System.register("desc", ["./message.js"], function (_export) {
  var Message;
  return {
    setters: [
      function (m) {
        Message = m.Message;
      }
    ],
    execute: function () {
      _export({ Edition: void 0 });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "void0 dummy with import only");
}

#[test]
fn execute_object_export_arrow_is_the_same_shape() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        foo: () => 1
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "odd.js");
    assert_no_system_register(entry, "execute object export of arrow");
    assert!(
        entry.contains("export const foo ="),
        "arrow value must become export const:\n{entry}"
    );
    assert_no_leftover_export_call(entry, "execute object export of arrow");
}

#[test]
fn execute_object_export_class_is_the_same_shape() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        Foo: class {
          m() {
            return 1;
          }
        }
      });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = module_code(&raw, "odd.js");
    assert_no_system_register(entry, "execute object export of class");
    assert!(
        entry.contains("export const Foo ="),
        "class value must become export const:\n{entry}"
    );
    assert!(
        entry.contains("function m") || entry.contains("m()"),
        "the class method must stay on the expression:\n{entry}"
    );
    assert_no_leftover_export_call(entry, "execute object export of class");
}

#[test]
fn execute_object_export_number_literal_preserves_whole_register() {
    // `_export("foo", 1)` reconstructs; `{ foo: 1 }` is not that proven shape.
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({ foo: 1 });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "object number literal");
}

#[test]
fn execute_object_export_default_key_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        default: function () {
          return 1;
        }
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "object default key");
}

#[test]
fn execute_object_export_empty_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({});
    }
  };
});
"#;
    assert_preserves_whole_register(source, "empty execute object export");
}

#[test]
fn execute_object_export_spread_preserves_whole_register() {
    let source = r#"
System.register("odd", [], function (_export) {
  return {
    execute: function () {
      _export({
        foo: function () {},
        ...other
      });
    }
  };
});
"#;
    assert_preserves_whole_register(source, "spread execute object export");
}

#[test]
fn execute_object_export_void0_does_not_fail_sibling_registers() {
    // Engine bundles mix many registers. One unlowerable `_export({ Name: void 0 })`
    // must not fail-closed the siblings (leftover-scan used to do that).
    let source = r#"
System.register("plain", [], function (_export) {
  return {
    execute: function () {
      _export("X", 1);
    }
  };
});
System.register("field", [], function (_export) {
  return {
    execute: function () {
      _export({ ScalarType: void 0 });
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let plain = module_code(&raw, "plain.js");
    let field = module_code(&raw, "field.js");
    assert_no_system_register(plain, "sibling of unlowerable object export");
    assert_no_leftover_export_call(plain, "sibling of unlowerable object export");
    assert!(
        plain.contains("export const X = 1"),
        "the reconstructable sibling must stay ESM:\n{plain}"
    );
    assert!(
        field.contains("System.register") && field.contains("void 0"),
        "the dummy sibling must keep its own register, not leftover ESM:\n{field}"
    );
}

#[test]
fn execute_object_export_void0_with_unrelated_export_preserves_whole_register() {
    // Babel emits dummy bulk exports beside unrelated two-arg exports. Dropping
    // only the dummy would silently remove a public namespace key.
    let source = r#"
System.register("desc", ["./message.js"], function (_export) {
  var Message;
  return {
    setters: [
      function (m) {
        Message = m.Message;
      }
    ],
    execute: function () {
      _export({ Edition: void 0 });
      _export("FileDescriptorProto", Message);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "void0 bulk beside unrelated export");
}

#[test]
fn execute_object_export_default_bulk_with_later_single_preserves_whole_register() {
    // Rollup + Terser can group default/named functions into a bulk call and
    // emit a later primitive as a two-arg export. The unsupported default pair
    // must not disappear merely because the later export is reconstructable.
    let source = r#"
System.register("entry", [], function (t) {
  return {
    execute: function () {
      t({
        default: function () { return 1; },
        other: function () { return 2; }
      });
      t("value", 3);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "default bulk beside unrelated export");
}

fn assert_named_import_from(code: &str, spec: &str, source: &str, label: &str) {
    let spaced = format!(r#"import {{ {spec} }} from "{source}""#);
    let compact = format!(r#"import {{{spec}}} from "{source}""#);
    assert!(
        code.contains(&spaced) || code.contains(&compact),
        "{label} must reconstruct `{spaced}`:\n{code}"
    );
}

fn assert_valid_unpacked_esm(modules: &[(String, String)], label: &str) {
    assert_eq!(
        validate_output_modules(modules),
        vec![],
        "{label} output must remain valid ESM:\n{}",
        modules
            .iter()
            .map(|(name, code)| format!("// {name}\n{code}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn unused_setter_member_read_becomes_named_import() {
    // Terser drops an unused `a = module.native` left-hand side but keeps the
    // getter (`pure_getters: false`). That is still `import { native }`.
    let source = r#"
System.register("herobdc", ["cc"], function (_export) {
  var n;
  return {
    setters: [function (module) {
      n = module.cclegacy, module.native;
    }],
    execute: function () {
      n._RF.push({}, "id", "herobdc", undefined);
      _export("HeroBDC", {});
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "herobdc.js");
    assert_no_system_register(entry, "unused setter member + assigned sibling");
    assert_named_import_from(
        entry,
        "cclegacy as n, native",
        "cc",
        "unused setter member + assigned sibling",
    );
    assert!(
        entry.contains("export const HeroBDC") || entry.contains("as HeroBDC"),
        "the two-arg execute export must still reconstruct:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "unused setter member + assigned sibling");
}

#[test]
fn unused_setter_member_read_alone_becomes_named_import() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module.foo;
    }],
    execute: function () {}
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "standalone unused setter member");
    assert_named_import_from(entry, "foo", "dep", "standalone unused setter member");
    assert_valid_unpacked_esm(&raw, "standalone unused setter member");
}

#[test]
fn unused_setter_member_quoted_key_is_the_same_shape() {
    let source = r#"
System.register("entry", ["cc"], function (_export) {
  var n;
  return {
    setters: [function (module) {
      n = module.cclegacy;
      module["native"];
    }],
    execute: function () {
      n._RF.push();
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "quoted unused setter member");
    assert_named_import_from(
        entry,
        "cclegacy as n, native",
        "cc",
        "quoted unused setter member",
    );
    assert_valid_unpacked_esm(&raw, "quoted unused setter member");
}

#[test]
fn unused_setter_member_after_same_name_assignment_keeps_assigned_local() {
    // The unused get is the DCE leftover of `a = module.native`. Keep `a`.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var a;
  return {
    setters: [function (module) {
      a = module.native, module.native;
    }],
    execute: function () {
      use(a);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "assigned then unused same name");
    assert_named_import_from(
        entry,
        "native as a",
        "dep",
        "assigned then unused same name",
    );
    assert!(
        !entry.contains("native as a, native") && !entry.contains("native, native"),
        "must not emit a second specifier for the unused get:\n{entry}"
    );
    assert!(
        entry.contains("use(a)"),
        "execute must keep the assigned local:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "assigned then unused same name");
}

#[test]
fn unused_setter_member_without_export_param_still_imports() {
    let source = r#"
System.register("entry", ["cc"], function () {
  var n;
  return {
    setters: [function (module) {
      n = module.cclegacy, module.native;
    }],
    execute: function () {
      n._RF.push();
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "unused member without export param");
    assert_named_import_from(
        entry,
        "cclegacy as n, native",
        "cc",
        "unused member without export param",
    );
    assert_no_invented_export(entry, "unused member without export param");
    assert_valid_unpacked_esm(&raw, "unused member without export param");
}

#[test]
fn unused_setter_member_call_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module.foo();
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter member call");
}

#[test]
fn unused_setter_member_computed_key_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module[k];
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter computed member");
}

#[test]
fn unused_setter_member_optional_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module?.foo;
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter optional member");
}

#[test]
fn unused_setter_member_default_preserves_whole_register() {
    // Unused `module.default` would be a default import, not this named shape.
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module.default;
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter default member");
}

#[test]
fn unused_setter_member_before_same_name_assignment_keeps_assigned_local() {
    // Same leftover as assigned-then-unused, opposite comma order.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var a;
  return {
    setters: [function (module) {
      module.native, a = module.native;
    }],
    execute: function () {
      use(a);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "unused then assigned same name");
    assert_named_import_from(
        entry,
        "native as a",
        "dep",
        "unused then assigned same name",
    );
    assert!(
        !entry.contains("native as a, native") && !entry.contains("{ native, native as a }"),
        "must not emit a second specifier for the unused get:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "unused then assigned same name");
}

#[test]
fn unused_setter_member_eval_still_imports_the_source_local() {
    // Source was `import { native }`. Restoring that local is the contract,
    // unlike `export { X } from` which never had a local (#211).
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module.native;
    }],
    execute: function () {
      eval("native");
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "unused member visible to eval");
    assert_named_import_from(entry, "native", "dep", "unused member visible to eval");
    assert!(
        entry.contains("eval("),
        "direct eval in execute must remain:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "unused member visible to eval");
}

#[test]
fn two_assigned_locals_for_same_imported_both_stay() {
    // Dedup is only for the unused leftover get, not a second assigned alias.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var a, b;
  return {
    setters: [function (module) {
      a = module.native, b = module.native;
    }],
    execute: function () {
      use(a, b);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "two assigned locals same imported");
    assert_named_import_from(
        entry,
        "native as a, native as b",
        "dep",
        "two assigned locals same imported",
    );
    assert!(
        entry.contains("use(a, b)") || entry.contains("use(a,b)"),
        "both assigned locals must remain in execute:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "two assigned locals same imported");
}

#[test]
fn assigned_identity_alias_beside_renamed_alias_both_stay() {
    // `native = module.native` is an assigned import, not a leftover get.
    // Dedup must not treat `local == imported` as unused.
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var a, native;
  return {
    setters: [function (module) {
      a = module.native, native = module.native;
    }],
    execute: function () {
      use(a, native);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "identity assign after renamed alias");
    assert_named_import_from(
        entry,
        "native as a, native",
        "dep",
        "identity assign after renamed alias",
    );
    assert!(
        entry.contains("use(a, native)") || entry.contains("use(a,native)"),
        "both assigned locals must remain in execute:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "identity assign after renamed alias");
}

#[test]
fn assigned_identity_alias_before_renamed_alias_both_stay() {
    let source = r#"
System.register("entry", ["dep"], function (_export) {
  var a, native;
  return {
    setters: [function (module) {
      native = module.native, a = module.native;
    }],
    execute: function () {
      use(a, native);
    }
  };
});
"#;
    let raw = unpack_source_raw(source);
    let entry = unpacked_named_module(&raw, "entry.js");
    assert_no_system_register(entry, "identity assign before renamed alias");
    assert_named_import_from(
        entry,
        "native, native as a",
        "dep",
        "identity assign before renamed alias",
    );
    assert!(
        entry.contains("use(a, native)") || entry.contains("use(a,native)"),
        "both assigned locals must remain in execute:\n{entry}"
    );
    assert_valid_unpacked_esm(&raw, "identity assign before renamed alias");
}

#[test]
fn unused_setter_member_reserved_key_preserves_whole_register() {
    // Member `class` is an IdentifierName. Binding it as `import { class }`
    // (or a sanitized local with Ident imported) is illegal ESM.
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module.class;
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter reserved member");
}

#[test]
fn unused_setter_member_invalid_quoted_key_preserves_whole_register() {
    let source = r#"
System.register("odd", ["dep"], function (_export) {
  return {
    setters: [function (module) {
      module["foo-bar"];
    }],
    execute: function () {}
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter invalid quoted member");
}

#[test]
fn unused_setter_member_colliding_local_preserves_whole_register() {
    // Inventing `import { n }` beside `import { cclegacy as n }` is illegal.
    let source = r#"
System.register("odd", ["cc"], function (_export) {
  var n;
  return {
    setters: [function (module) {
      n = module.cclegacy, module.n;
    }],
    execute: function () {
      n._RF.push();
    }
  };
});
"#;
    assert_preserves_whole_register(source, "unused setter colliding invented local");
}

#[test]
fn unused_setter_member_cross_dependency_collision_preserves_whole_register() {
    // Babel SystemJS + Terser `pure_getters: false` produces this from:
    //   import { foo as kept } from "./a.js";
    //   import { kept as unused } from "./b.js";
    //   export const result = kept;
    // The second import has lost its local alias, so `kept` is not free.
    let source = r#"
System.register("entry", ["./a.js", "./b.js"], function (_export, _context) {
  "use strict";
  var kept;
  return {
    setters: [
      function (_aJs) {
        kept = _aJs.foo;
      },
      function (_bJs) {
        _bJs.kept;
      }
    ],
    execute: function () {
      _export("result", kept);
    }
  };
});
"#;
    assert_preserves_whole_register(source, "cross-dependency inferred import collision");
}

#[test]
fn unused_setter_member_lifted_local_collision_preserves_whole_register() {
    // Babel SystemJS + Terser `pure_getters: false` produces this from:
    //   import { foo as unused } from "./dep.js";
    //   let foo = 0;
    //   globalThis.inc = () => ++foo;
    // Treating the getter as `import { foo }` would turn `foo = 0` into an
    // assignment to a read-only import binding.
    let source = r#"
System.register("entry", ["./dep.js"], function (_export, _context) {
  "use strict";
  var foo;
  return {
    setters: [function (_depJs) {
      _depJs.foo;
    }],
    execute: function () {
      foo = 0, globalThis.inc = () => ++foo;
    }
  };
});
"#;
    assert_preserves_whole_register(source, "lifted-local inferred import collision");
}

#[test]
fn unused_setter_member_free_name_capture_preserves_whole_register() {
    // The erased local alias is unknowable. Introducing `import { foo }`
    // must not capture an execute-body reference that was still global.
    let source = r#"
System.register("entry", ["./dep.js"], function (_export, _context) {
  "use strict";
  return {
    setters: [function (_depJs) {
      _depJs.foo;
    }],
    execute: function () {
      globalThis.result = foo;
    }
  };
});
"#;
    assert_preserves_whole_register(source, "free-name inferred import capture");
}
