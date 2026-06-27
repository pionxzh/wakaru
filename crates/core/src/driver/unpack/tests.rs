use std::collections::{HashMap, HashSet};

use super::*;
use crate::unpacker::UnpackedModule;

#[test]
fn nested_scope_split_gate_requires_heuristic_split_and_aggressive() {
    assert!(!nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: false,
        level: RewriteLevel::Aggressive,
        ..Default::default()
    }));
    assert!(!nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: true,
        level: RewriteLevel::Standard,
        ..Default::default()
    }));
    assert!(!nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: true,
        level: RewriteLevel::Minimal,
        ..Default::default()
    }));
    assert!(nested_scope_split_enabled(&DecompileOptions {
        heuristic_split: true,
        level: RewriteLevel::Aggressive,
        ..Default::default()
    }));
}

#[test]
fn scan_local_import_dependencies_reads_static_imports() {
    let module_names = ["a.js".to_string(), "nested/b.js".to_string()]
        .into_iter()
        .collect();
    let deps = scan_local_import_dependencies(
        "nested/current.js",
        r#"
import { a } from "../a.js";
import {
  b
} from "./b.js";
import fs from "fs";
const value = import("./dynamic.js");
"#,
        &module_names,
    )
    .expect("static imports should scan without parsing");

    assert_eq!(deps, vec!["a.js".to_string(), "nested/b.js".to_string()]);
}

#[test]
fn scan_local_import_dependencies_ignores_import_like_body_code() {
    let module_names = ["dynamic.js".to_string()].into_iter().collect();
    let deps = scan_local_import_dependencies(
        "entry.js",
        r#"
const value = "import './dynamic.js'";
import("./dynamic.js");
"#,
        &module_names,
    )
    .expect("non-import prefix should still be a valid fast scan");

    assert!(deps.is_empty());
}

#[test]
fn scan_local_import_dependencies_ignores_nested_import_like_lines() {
    let module_names = ["nested.js".to_string()].into_iter().collect();
    let deps = scan_local_import_dependencies(
        "entry.js",
        r#"
function load() {
  import { nested } from "./nested.js";
}
"#,
        &module_names,
    )
    .expect("nested import-like code should still scan without parsing");

    assert!(deps.is_empty());
}

#[test]
fn unpack_raw_preserves_unparseable_extracted_modules() {
    let result = unpack_raw(
        "const = ;",
        &DecompileOptions {
            heuristic_split: false,
            ..Default::default()
        },
    );

    assert!(result.is_err(), "invalid top-level input should still fail");

    let modules = vec![UnpackedModule {
        id: "1".to_string(),
        is_entry: false,
        code: "const = ;".to_string(),
        filename: "module-1.js".to_string(),
        ..Default::default()
    }];
    let output = unpack_multi_module(modules, DecompileOptions::default())
        .expect("unparseable extracted modules should be preserved as raw code");
    assert_eq!(
        output.modules,
        vec![("module-1.js".to_string(), "const = ;".to_string())]
    );
    assert!(
        !output.warnings.is_empty(),
        "should warn about unparseable module"
    );
    let warning_kinds = output
        .warnings
        .iter()
        .map(|warning| {
            assert_eq!(warning.filename, "module-1.js");
            warning.kind
        })
        .collect::<Vec<_>>();
    assert_eq!(
        warning_kinds,
        vec![
            UnpackWarningKind::FactCollectionParseFailed,
            UnpackWarningKind::DecompileFailed
        ]
    );
}

#[test]
fn detector_raw_large_scope_split_skips_runnable_cleanup_merge() {
    let mut source = String::from(
        r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { a: () => a });
function a() { return b(); }
var ns_b = {};
__export(ns_b, { b: () => b });
function b() { return a(); }
"#,
    );
    for index in 0..1000 {
        source.push_str(&format!(
                "var ns_{index} = {{}};\n__export(ns_{index}, {{ v{index}: () => v{index} }});\nvar v{index} = {index};\n"
            ));
    }
    source.push_str("export { ns_a, ns_b };\n");

    let output = unpack_raw(&source, &DecompileOptions::default())
        .expect("large detector raw split should unpack");
    let filenames: HashSet<_> = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    assert!(
        filenames.contains("ns_a.js") && filenames.contains("ns_b.js"),
        "detector raw output should preserve split cycle members instead of running merge cleanup"
    );
    assert!(
        output.modules.len() > 1000,
        "fixture should exercise large synthetic raw output, got {} modules",
        output.modules.len()
    );
}

#[test]
fn unpack_propagates_invalid_input_parse_errors() {
    let err = unpack(
        "const = ;",
        DecompileOptions {
            filename: "broken.js".to_string(),
            ..Default::default()
        },
    )
    .expect_err("invalid source should fail");

    assert!(
        err.to_string().contains("broken.js"),
        "error should include input filename: {err}"
    );
}

#[test]
fn unpack_preserves_typescript_single_file_fallback() {
    let output = unpack(
        "const value: number = 1;",
        DecompileOptions {
            filename: "input.ts".to_string(),
            ..Default::default()
        },
    )
    .expect("valid TypeScript should fall back to single-file decompile");

    assert_eq!(output.modules.len(), 1);
    assert_eq!(output.modules[0].0, "module.js");
    assert!(
        output.modules[0].1.contains("const value"),
        "expected TypeScript input to decompile, got: {}",
        output.modules[0].1
    );
}

#[test]
fn import_cycle_warnings_report_local_sccs() {
    let modules = vec![
        (
            "a.js".to_string(),
            r#"import { b } from "./b.js"; export const a = b;"#.to_string(),
        ),
        (
            "b.js".to_string(),
            r#"import { a } from "./a.js"; export const b = a;"#.to_string(),
        ),
        (
            "c.js".to_string(),
            r#"import { a } from "./a.js"; export const c = a;"#.to_string(),
        ),
    ];

    let warnings = collect_import_cycle_warnings(&modules);

    assert_eq!(warnings.len(), 1, "should report one SCC: {warnings:?}");
    assert_eq!(warnings[0].kind, UnpackWarningKind::ImportCycle);
    assert!(warnings[0].message.contains("2 modules"));
    assert!(warnings[0].message.contains("a.js"));
    assert!(warnings[0].message.contains("b.js"));
}

#[test]
fn merge_import_cycles_drops_internal_imports_and_retargets_consumers() {
    let modules = vec![
        UnpackedModule {
            id: "a".to_string(),
            is_entry: false,
            code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
            filename: "a.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "b".to_string(),
            is_entry: false,
            code: r#"import { a } from "./a.js"; export const b = a + 1;"#.to_string(),
            filename: "b.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "c".to_string(),
            is_entry: false,
            code: r#"import { b } from "./b.js"; export const c = b;"#.to_string(),
            filename: "c.js".to_string(),
            ..Default::default()
        },
    ];

    let (merged, warnings) = merge_import_cycles(modules);

    assert!(
        warnings.is_empty(),
        "successful cycle repair should not surface as stderr warnings: {warnings:?}"
    );
    assert_eq!(merged.len(), 2);
    let a = merged
        .iter()
        .find(|module| module.filename == "a.js")
        .expect("cycle should merge into first module");
    assert!(
        !a.code.contains("from \"./b.js\"") && a.code.contains("export const b"),
        "merged cycle should drop internal imports and retain member code:\n{}",
        a.code
    );
    let c = merged
        .iter()
        .find(|module| module.filename == "c.js")
        .expect("consumer should remain separate");
    assert!(
        c.code.contains("from \"./a.js\""),
        "consumer should retarget imports to merged representative:\n{}",
        c.code
    );
}

#[test]
fn merge_import_cycles_does_not_reprint_unrelated_modules() {
    let untouched_code = "const untouched = 1   ;";
    let modules = vec![
        UnpackedModule {
            id: "a".to_string(),
            is_entry: false,
            code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
            filename: "a.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "b".to_string(),
            is_entry: false,
            code: r#"import { a } from "./a.js"; export const b = a + 1;"#.to_string(),
            filename: "b.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "d".to_string(),
            is_entry: false,
            code: untouched_code.to_string(),
            filename: "d.js".to_string(),
            ..Default::default()
        },
    ];

    let (merged, warnings) = merge_import_cycles(modules);

    assert!(
        warnings.is_empty(),
        "successful cycle repair should not surface as stderr warnings: {warnings:?}"
    );
    let unrelated = merged
        .iter()
        .find(|module| module.filename == "d.js")
        .expect("unrelated module should remain");
    assert_eq!(unrelated.code, untouched_code);
}

#[test]
fn merge_import_cycles_dedups_external_imports_before_safety_check() {
    let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { shared } from "./x.js"; import { b } from "./b.js"; export const a = b + shared;"#
                    .to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { shared } from "./x.js"; import { a } from "./a.js"; export const b = a + shared;"#
                    .to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
        ];

    let (merged, warnings) = merge_import_cycles(modules);

    assert_eq!(merged.len(), 1, "warnings: {warnings:?}");
    assert!(
        warnings.is_empty(),
        "duplicate external imports should not block a safe merge or emit stderr warnings: {:?}",
        warnings
    );
    let a = &merged[0];
    assert_eq!(a.filename, "a.js");
    assert_eq!(
        a.code.matches("from \"./x.js\"").count(),
        1,
        "merged cycle should deduplicate external imports:\n{}",
        a.code
    );
    assert!(
        !a.code.contains("from \"./b.js\"") && a.code.contains("export const b"),
        "merged cycle should drop internal imports and retain member code:\n{}",
        a.code
    );
}

#[test]
fn merge_import_cycles_dedups_redundant_named_exports() {
    let modules = vec![
        UnpackedModule {
            id: "a".to_string(),
            is_entry: false,
            code: r#"import { b } from "./b.js"; export function f() { return b; }"#.to_string(),
            filename: "a.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "b".to_string(),
            is_entry: false,
            code: r#"import { f } from "./a.js"; export const b = 1; export { f };"#.to_string(),
            filename: "b.js".to_string(),
            ..Default::default()
        },
    ];

    let (merged, warnings) = merge_import_cycles(modules);

    assert_eq!(merged.len(), 1, "warnings: {warnings:?}");
    let a = &merged[0];
    assert!(
        a.code.contains("export function f"),
        "merged cycle should keep the declaration export:\n{}",
        a.code
    );
    assert!(
        !a.code.contains("export { f"),
        "merged cycle should remove the redundant named export:\n{}",
        a.code
    );
}

#[test]
fn hoist_late_runtime_helpers_moves_helper_defs_before_side_effects() {
    let input = r#"
setup();
result = helper(value);
const { defineProperty } = Object;
var helper = (target) => defineProperty({}, "x", { value: target });
let cache;
function setup() {}
consumer = wrap(ns);
export var ns = {};
Object.defineProperty(ns, "value", { enumerable: true, get: () => value });
export { helper, cache };
"#;

    let output = GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(input, "fixture.js", cm.clone()).expect("input parses");
        hoist_late_runtime_helpers(&mut module);
        print_js(&module, cm).expect("output prints")
    });

    let define_property = output
        .find("const { defineProperty")
        .expect("object destructuring helper should remain");
    let helper = output
        .find("var helper")
        .expect("helper declaration should remain");
    let cache = output
        .find("let cache")
        .expect("state declaration should remain");
    let call = output.find("result = helper").expect("call should remain");
    let namespace = output
        .find("export var ns")
        .expect("namespace export should remain");
    let namespace_getter = output
        .find("Object.defineProperty(ns")
        .expect("namespace getter should remain");
    let namespace_use = output.find("consumer = wrap").expect("use should remain");

    assert!(
        define_property < call && helper < call && cache < call,
        "late helper declarations should move before side effects:\n{output}"
    );
    assert!(
        namespace < namespace_use && namespace_getter < namespace_use,
        "late namespace export setup should move before side effects:\n{output}"
    );
}

#[test]
fn merge_import_cycles_skips_duplicate_declaration_merges() {
    let modules = vec![
        UnpackedModule {
            id: "a".to_string(),
            is_entry: false,
            code: r#"import { b } from "./b.js"; const shared = 1; export const a = b + shared;"#
                .to_string(),
            filename: "a.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "b".to_string(),
            is_entry: false,
            code: r#"import { a } from "./a.js"; const shared = 2; export const b = a + shared;"#
                .to_string(),
            filename: "b.js".to_string(),
            ..Default::default()
        },
    ];

    let (merged, warnings) = merge_import_cycles(modules);

    assert_eq!(merged.len(), 2, "unsafe cycles should stay split");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].message.contains("not merged")
            && warnings[0].message.contains("duplicate declarations"),
        "warning should explain why the cycle stayed split: {:?}",
        warnings
    );
    let a = merged
        .iter()
        .find(|module| module.filename == "a.js")
        .expect("a.js should remain separate");
    assert!(
        a.code.contains("from \"./b.js\""),
        "skipped cycle should preserve original imports:\n{}",
        a.code
    );
}

#[test]
fn merge_import_cycles_skips_large_components() {
    let modules: Vec<UnpackedModule> = (0..33)
            .map(|index| {
                let next = (index + 1) % 33;
                UnpackedModule {
                    id: format!("m{index}"),
                    is_entry: false,
                    code: format!(
                        r#"import {{ v{next} }} from "./m{next}.js"; export const v{index} = v{next} + {index};"#
                    ),
                    filename: format!("m{index}.js"),
                    ..Default::default()
                }
            })
            .collect();

    let (merged, warnings) = merge_import_cycles(modules);

    assert_eq!(merged.len(), 33, "large cycles should stay split");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].message.contains("not merged")
            && warnings[0].message.contains("large-cycle merge limit"),
        "warning should explain why the large cycle stayed split: {:?}",
        warnings
    );
}

#[test]
fn fast_cycle_preflight_allows_duplicate_var_declarations() {
    let modules = [
        UnpackedModule {
            id: "a".to_string(),
            is_entry: false,
            code: r#"import { b } from "./b.js"; var shared = 1; export const a = b + shared;"#
                .to_string(),
            filename: "a.js".to_string(),
            ..Default::default()
        },
        UnpackedModule {
            id: "b".to_string(),
            is_entry: false,
            code: r#"import { a } from "./a.js"; var shared = 2; export const b = a + shared;"#
                .to_string(),
            filename: "b.js".to_string(),
            ..Default::default()
        },
    ];
    let module_by_filename: HashMap<String, &UnpackedModule> = modules
        .iter()
        .map(|module| (module.filename.clone(), module))
        .collect();
    let module_names: HashSet<String> = modules
        .iter()
        .map(|module| module.filename.clone())
        .collect();
    let members = vec!["a.js".to_string(), "b.js".to_string()];
    let member_set: HashSet<String> = members.iter().cloned().collect();

    assert!(
        unsafe_merge_member_reason(&members, &module_by_filename, &module_names, &member_set)
            .is_none(),
        "generated duplicate vars should not block the large-cycle fast preflight"
    );
}
