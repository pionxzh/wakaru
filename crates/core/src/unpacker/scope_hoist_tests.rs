use super::*;

fn split(source: &str) -> Option<Vec<(String, String, bool)>> {
    let result = split_scope_hoisted(source)?;
    Some(
        result
            .modules
            .into_iter()
            .map(|m| (m.filename, m.code, m.is_entry))
            .collect(),
    )
}

fn count_modules(source: &str) -> usize {
    split(source).map(|m| m.len()).unwrap_or(0)
}

fn two_group_fixture(b1: &str) -> String {
    [
        r#"
            function a1() { return 1; }
            function a2() { return a1() + 1; }
            function a3() { return a2() * 2; }
            function a4() { return a3() + 3; }
            function a5() { return a4() - 1; }
            "#,
        b1,
        r#"
            function b2() { return b1() + 10; }
            function b3() { return b2() * 20; }
            function b4() { return b3() + 30; }
            function b5() { return b4() - 10; }

            const k = a5() + b5();
            console.log(k);
            "#,
    ]
    .join("\n")
}

fn assert_splits(source: &str, reason: &str) {
    let n = count_modules(source);
    assert!(n >= 2, "{reason}, got {n} modules");
}

fn assert_does_not_split(source: &str, reason: &str) {
    let n = count_modules(source);
    assert!(n < 2, "{reason}, got {n} modules");
}

#[test]
fn too_few_declarations_returns_none() {
    let input = r#"
            function a() { return 1; }
            function b() { return a(); }
            const c = 3;
        "#;
    assert!(split(input).is_none());
}

#[test]
fn splits_independent_groups() {
    // Two clearly independent groups of functions + an entry using both.
    let input = r#"
            function helperA1() { return 1; }
            function helperA2() { return helperA1() + 1; }
            function helperA3() { return helperA2() * 2; }
            function publicA() { return helperA3(); }

            function helperB1() { return 10; }
            function helperB2() { return helperB1() + 10; }
            function helperB3() { return helperB2() * 20; }
            function publicB() { return helperB3(); }

            const x = publicA();
            const y = publicB();
            console.log(x, y);
        "#;
    let n = count_modules(input);
    assert!(n >= 2, "expected at least 2 modules, got {n}");
}

#[test]
fn cluster_filename_dedup_is_case_insensitive() {
    let mut seen = HashSet::new();
    assert_eq!(
        dedup_cluster_filename("chunk_Helper.js", &mut seen),
        "chunk_Helper.js"
    );
    assert_eq!(
        dedup_cluster_filename("chunk_helper.js", &mut seen),
        "chunk_helper_2.js"
    );
    assert_eq!(
        dedup_cluster_filename("chunk_helper.js", &mut seen),
        "chunk_helper_3.js"
    );
}

#[test]
fn entry_gets_module_decls() {
    let input = r#"
            function helperA1() { return 1; }
            function helperA2() { return helperA1() + 1; }
            function helperA3() { return helperA2() * 2; }
            function helperA4() { return helperA3() + 5; }
            function publicA() { return helperA4(); }

            function helperB1() { return 10; }
            function helperB2() { return helperB1() + 10; }
            function helperB3() { return helperB2() * 20; }
            function helperB4() { return helperB3() + 50; }
            function publicB() { return helperB4(); }

            const result = publicA() + publicB();
            export { result };
        "#;
    let modules = split(input).expect("should split");
    let entry = modules.iter().find(|(_, _, is_entry)| *is_entry);
    assert!(entry.is_some(), "should have an entry module");
    let (filename, code, _) = entry.unwrap();
    assert_eq!(filename, "entry.js");
    assert!(
        code.contains("export"),
        "entry should contain export statement"
    );
}

#[test]
fn class_with_private_helpers_stays_together() {
    // A class with WeakMap helpers should cluster together.
    let input = r#"
            function utilA() { return 1; }
            function utilB() { return utilA() + 2; }
            function utilC() { return utilB() + 3; }
            function utilD() { return utilC() * 2; }
            function utilE() { return utilD() - 1; }
            function utilF() { return utilE() + 7; }

            const _data = new WeakMap();
            const _listeners = new WeakMap();
            class Store {
                constructor(initial) {
                    _data.set(this, initial);
                    _listeners.set(this, []);
                }
                get(key) { return _data.get(this)[key]; }
                set(key, value) {
                    _data.get(this)[key] = value;
                    for (const fn1 of _listeners.get(this)) fn1(key, value);
                }
            }

            const s = new Store({});
            s.set("x", utilF());
            console.log(s.get("x"));
        "#;
    let modules = split(input).expect("should split");

    // Find the module containing Store.
    let store_module = modules
        .iter()
        .find(|(_, code, _)| code.contains("class Store"));
    assert!(store_module.is_some(), "should have a Store module");
    let (_, code, _) = store_module.unwrap();
    assert!(
        code.contains("_data") && code.contains("_listeners"),
        "WeakMap helpers should be in the same module as Store"
    );
}

#[test]
fn vite_fixture_clusters() {
    let input = include_str!("../../tests/bundles/vite-gen/dist/es/bundle.mjs");
    let clusters = debug_clusters(input);
    let module_count = clusters.iter().filter(|(_, e)| !e).count();
    // Logger, Store, and API should still be recognized as logical groups. Some
    // groups may share an emitted module when separating them would create a
    // cyclic cluster graph and change eager initialization order.
    assert!(
        module_count >= 2,
        "expected at least 2 safe module clusters from vite fixture, got {module_count}"
    );

    // The algorithm should identify at least these modules:
    // - Logger module (LogLevel + Logger class)
    // - Store module (_data, _subs, CHANGE, RESET, Store)
    // - API module (BASE_URL, request, getUser, getPosts)
    let has_logger = clusters.iter().any(|(names, _)| {
        names.contains(&"LogLevel".to_string()) && names.contains(&"Logger".to_string())
    });
    let has_store = clusters
        .iter()
        .any(|(names, _)| names.contains(&"Store".to_string()));
    let has_api = clusters.iter().any(|(names, _)| {
        names.contains(&"BASE_URL".to_string()) && names.contains(&"request".to_string())
    });

    assert!(has_logger, "should cluster Logger module");
    assert!(has_store, "should cluster Store module");
    assert!(has_api, "should cluster API module");
}

#[test]
fn vite_fixture_import_export() {
    let input = include_str!("../../tests/bundles/vite-gen/dist/es/bundle.mjs");
    let modules = split(input).expect("should split vite fixture");

    // Every non-entry chunk should have an export statement.
    for (filename, code, is_entry) in &modules {
        if *is_entry {
            continue;
        }
        assert!(
            code.contains("export"),
            "{filename} should have export statement"
        );
    }

    // Entry should import from the chunks it references.
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .expect("should have entry");
    assert!(
        entry.1.contains("import"),
        "entry should have import statements"
    );
    assert!(
        entry
            .1
            .contains("import { getPosts, getUser } from \"./chunk_BASE_URL.js\";"),
        "entry imports from API chunk should be sorted, got:\n{}",
        entry.1
    );
    assert!(
        entry
            .1
            .contains("import { LogLevel, Logger } from \"./chunk_LogLevel.js\";"),
        "entry imports from Logger chunk should be sorted, got:\n{}",
        entry.1
    );
}

#[test]
fn chunk_references_to_imported_bindings_keep_imports() {
    let input = r#"
            import { constants as ky5 } from "node:os";
            import { value as zE7 } from "./dep.js";

            function groupA1() { return ky5.signals.SIGTERM; }
            function groupA2() { return zE7 + groupA1(); }
            function groupA3() { return groupA2() + 1; }
            function groupA4() { return groupA3() + 1; }
            function publicA() { return groupA4(); }

            function groupB1() { return 10; }
            function groupB2() { return groupB1() + 1; }
            function groupB3() { return groupB2() + 1; }
            function groupB4() { return groupB3() + 1; }
            function publicB() { return groupB4(); }

            const result = publicA() + publicB();
            console.log(result);
        "#;

    let modules = split(input).expect("should split");
    let imported_consumer = modules
        .iter()
        .find(|(_, code, is_entry)| {
            !*is_entry && code.contains("ky5.signals") && code.contains("zE7")
        })
        .expect("should have a non-entry chunk that consumes imported bindings");

    assert!(
        imported_consumer
            .1
            .contains("import { constants as ky5 } from \"node:os\";"),
        "chunk should copy the node:os import for ky5:\n{}",
        imported_consumer.1
    );
    assert!(
        imported_consumer
            .1
            .contains("import { value as zE7 } from \"./dep.js\";"),
        "chunk should copy the relative import for zE7:\n{}",
        imported_consumer.1
    );
}

#[test]
fn partial_var_export_preserves_declarator_order() {
    let input = r#"
            function a1() { return 1; }
            function a2() { return a1() + 1; }
            function a3() { return a2() + 1; }
            function a4() { return a3() + 1; }
            const exported = mark("exported"), kept = mark("kept");

            function b1() { return 10; }
            function b2() { return b1() + 1; }
            function b3() { return b2() + 1; }
            function b4() { return b3() + 1; }
            function b5() { return b4() + exported; }
            console.log(b5());
        "#;

    let modules = split(input).expect("should split");
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .expect("should have entry");
    let exported_pos = entry
        .1
        .find("export const exported = mark(\"exported\");")
        .expect("should export the referenced declarator inline");
    let kept_pos = entry
        .1
        .find("const kept = mark(\"kept\");")
        .expect("should keep the unreferenced declarator");
    assert!(
        exported_pos < kept_pos,
        "partial var export should preserve declarator order, got:\n{}",
        entry.1
    );
}

#[test]
fn cluster_cycle_merge_preserves_original_initialization_order() {
    // Folding small roots into the synthetic entry can create a cluster-level
    // cycle even though the original item graph is acyclic. If emitted as two
    // ESM modules, `result = make()` runs while `A` is still in its TDZ.
    let input = r#"
            class A {}
            const x1 = 1; function f1() { return x1; }
            const x2 = 2; function f2() { return x2; }
            const x3 = 3; function f3() { return x3; }
            const x4 = 4; function f4() { return x4; }
            function make() { return new A(); }
            const result = make();
            console.log(result, f1(), f2(), f3(), f4());
            export { result };
        "#;

    let modules = split(input).expect("should split");
    assert_eq!(modules.len(), 5, "entry cycle should be merged");

    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .expect("should have entry");
    let class_pos = entry.1.find("class A").expect("entry should contain A");
    let init_pos = entry
        .1
        .find("result = make()")
        .expect("entry should contain eager initialization");
    assert!(
        class_pos < init_pos,
        "merged entry must retain source initialization order:\n{}",
        entry.1
    );
    assert!(
        modules
            .iter()
            .all(|(_, code, _)| !code.contains("from \"./entry.js\"")),
        "split output must not retain the synthesized entry cycle"
    );
}

#[test]
fn vite_fixture_minified_clusters() {
    let input = include_str!("../../tests/bundles/vite-gen/dist/es-min/bundle.mjs");
    let clusters = debug_clusters(input);
    let module_count = clusters.iter().filter(|(_, e)| !e).count();
    assert!(
        module_count >= 3,
        "expected at least 3 module clusters from minified vite fixture, got {module_count}"
    );
}

#[test]
fn minified_names_still_split() {
    let input = r#"
            function a() { return 1; }
            function b() { return a() + 1; }
            function c() { return b() * 2; }
            function d() { return c() + 3; }
            function e() { return d() - 1; }

            function f() { return 10; }
            function g() { return f() + 10; }
            function h() { return g() * 20; }
            function i() { return h() + 30; }
            function j() { return i() - 10; }

            const k = d() + j();
            console.log(k);
        "#;
    let n = count_modules(input);
    assert!(
        n >= 2,
        "expected at least 2 modules with minified names, got {n}"
    );
}

#[test]
fn local_shadows_do_not_create_false_refs() {
    for (name, b1) in [
        (
            "local const shadow",
            "function b1() { const a5 = 10; return a5; }",
        ),
        (
            "nested function declaration shadow",
            "function b1() { function a5() { return 10; } return a5(); }",
        ),
        (
            "destructuring shadow",
            "function b1(o) { const { a5 } = o; return a5; }",
        ),
    ] {
        let input = two_group_fixture(b1);
        assert_splits(&input, &format!("{name} should not merge groups"));
    }
}

#[test]
fn shorthand_local_shadow_does_not_create_false_ref() {
    let input = two_group_fixture(
        r#"
            function b1() {
                const a5 = 10;
                return { a5 };
            }
            "#,
    );

    assert_splits(
        &input,
        "shorthand property should respect local binding shadows",
    );
}

#[test]
fn named_function_expression_shadow_does_not_create_false_ref() {
    let input = two_group_fixture(
        r#"
            function b1() {
                const fn = function a5() {
                    return a5;
                };
                return fn();
            }
            "#,
    );

    assert_splits(
        &input,
        "named function expression should bind its own name locally",
    );
}

#[test]
fn named_class_expression_shadow_does_not_create_false_ref() {
    let input = two_group_fixture(
        r#"
            function b1() {
                const C = class a5 {
                    method() {
                        return a5;
                    }
                };
                return C;
            }
            "#,
    );

    assert_splits(
        &input,
        "named class expression should bind its own name locally",
    );
}

#[test]
fn static_super_property_does_not_create_false_ref() {
    let input = two_group_fixture(
        r#"
            function b1() {
                return class extends Base {
                    method() {
                        return super.a5;
                    }
                };
            }
            "#,
    );

    assert_splits(&input, "static super property should not reference a5");
}

#[test]
fn jsx_member_property_does_not_create_false_ref() {
    let input = two_group_fixture(
        r#"
            function b1() {
                return <Foo.a5 />;
            }
            "#,
    );

    assert_splits(&input, "jsx member property should not reference a5");
}

#[test]
fn block_scoped_bindings_do_not_suppress_outer_refs() {
    for (name, b1) in [
        (
            "if-block const",
            "function b1(flag) { if (flag) { const a5 = 10; } return a5(); }",
        ),
        (
            "for-loop let",
            "function b1() { for (let a5 = 0; a5 < 3; a5++) {} return a5(); }",
        ),
    ] {
        let input = two_group_fixture(b1);
        assert_does_not_split(
            &input,
            &format!("{name} should leave later a5() as a top-level ref"),
        );
    }
}

#[test]
fn var_in_block_survives_block_restore() {
    let input = two_group_fixture(
        r#"
            function b1(flag) { if (flag) { var a5 = function(){ return 10; }; } return a5(); }
            "#,
    );
    assert_splits(
        &input,
        "var in block should shadow at function scope after block exit",
    );
}

#[test]
fn binding_pattern_defaults_reference_top_level() {
    for (name, b1) in [
        ("parameter default", "function b1(x = a5()) { return x; }"),
        (
            "destructured parameter default",
            "function b1({x = a5()} = {}) { return x; }",
        ),
        (
            "object binding pattern default",
            "function b1(o) { const {x = a5()} = o; return x; }",
        ),
        (
            "array binding pattern default",
            "function b1(arr) { const [x = a5()] = arr; return x; }",
        ),
    ] {
        let input = two_group_fixture(b1);
        assert_does_not_split(&input, &format!("{name} should detect top-level a5 ref"));
    }
}

#[test]
fn iife_trailing_statements_preserved() {
    // Trailing statements after the IIFE should end up in the output.
    let input = r#"(function() {
            function a1() { return 1; }
            function a2() { return a1() + 1; }
            function a3() { return a2() * 2; }
            function a4() { return a3() + 3; }
            function a5() { return a4() - 1; }

            function b1() { return 10; }
            function b2() { return b1() + 10; }
            function b3() { return b2() * 20; }
            function b4() { return b3() + 30; }
            function b5() { return b4() - 10; }

            var result = a5() + b5();
        })();
        console.log("after");
        "#;
    let modules = split(input).expect("should split IIFE bundle");
    let all_code: String = modules.iter().map(|(_, code, _)| code.as_str()).collect();
    assert!(
        all_code.contains("after"),
        "trailing statement after IIFE should be preserved"
    );
}
