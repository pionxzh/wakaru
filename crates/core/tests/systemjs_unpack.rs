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
        .find("const __systemjs_export_2 =")
        .unwrap_or_else(|| panic!("default export should use a collision-free binding:\n{entry}"));
    let member_assignment = entry
        .find("__systemjs_export_2.marker = \"default\";")
        .unwrap_or_else(|| panic!("member assignment should use the binding:\n{entry}"));
    let default_export = entry
        .find("export default __systemjs_export_2;")
        .unwrap_or_else(|| panic!("default export should use the binding:\n{entry}"));

    assert!(
        binding < default_export && default_export < member_assignment,
        "binding, default export, and member assignment should preserve order:\n{entry}"
    );
    assert_eq!(
        entry.matches("function DefaultValue()").count(),
        1,
        "default export value should be evaluated once:\n{entry}"
    );
    assert!(
        !entry.contains("export default function") && !entry.contains("export default ("),
        "default export must not apply the member assignment to the IIFE result:\n{entry}"
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
        entry.contains("const __systemjs_export_2 =")
            && entry.contains("export default __systemjs_export_2;"),
        "default export binding should avoid names in the module prelude:\n{entry}"
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
            || entry.contains("export {") && entry.contains("TodayShow"),
        "nested `_export` literal must still emit the name:\n{entry}"
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
