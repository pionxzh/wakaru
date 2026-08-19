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
        .find("const __systemjs_export =")
        .unwrap_or_else(|| panic!("named export binding should be recovered:\n{entry}"));
    let named_export = entry
        .find("export { __systemjs_export as DerivedClass };")
        .unwrap_or_else(|| panic!("named export alias should be recovered:\n{entry}"));
    let member_assignment = entry
        .find("__systemjs_export.marker = \"derived\";")
        .unwrap_or_else(|| panic!("member assignment should be preserved:\n{entry}"));
    let after = entry
        .find("after();")
        .unwrap_or_else(|| panic!("sequence side effect should be preserved:\n{entry}"));

    assert!(
        binding < named_export && named_export < member_assignment && member_assignment < after,
        "binding, named export, member assignment, and sequence side effect should preserve order:\n{entry}"
    );
    assert!(
        !entry.lines().any(|line| line.starts_with("function (")),
        "top-level anonymous function should not be emitted:\n{entry}"
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
fn direct_declaration_export_is_not_repeated_in_trailing_export_list() {
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

    assert_eq!(
        entry.matches("export const Utils =").count(),
        1,
        "Utils should have exactly one export declaration:\n{entry}"
    );
    assert!(
        !entry.contains("export { Utils"),
        "trailing named export list must not repeat Utils:\n{entry}"
    );
    assert_eq!(
        entry.matches("export {").count(),
        0,
        "fully filtered trailing exports must not emit an empty export declaration:\n{entry}"
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
        .find("const __systemjs_export = makeValue();")
        .unwrap_or_else(|| panic!("VALUE binding should be recovered:\n{entry}"));
    let named_export = entry
        .find("export { __systemjs_export as DerivedClass };")
        .unwrap_or_else(|| panic!("named export alias should be recovered:\n{entry}"));
    let assignment = entry
        .find("__systemjs_export[getKey()] += rhs();")
        .unwrap_or_else(|| panic!("computed assignment should be preserved:\n{entry}"));
    let rest = entry
        .find("after();")
        .unwrap_or_else(|| panic!("sequence rest should be preserved:\n{entry}"));

    assert!(
        binding < named_export && named_export < assignment && assignment < rest,
        "VALUE binding, named export, computed assignment, and sequence rest should preserve order:\n{entry}"
    );
    for call in ["makeValue()", "getKey()", "rhs()", "after()"] {
        assert_eq!(
            entry.matches(call).count(),
            1,
            "{call} should appear exactly once:\n{entry}"
        );
    }
    assert!(
        entry.contains("__systemjs_export[getKey()] += rhs();"),
        "assignment operator and computed member should be preserved:\n{entry}"
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
