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

fn unwraps_first_iife(source: &str) -> bool {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::super::parse_es_module(source, "bundle.js", cm)
            .expect("the IIFE fixture should parse");
        unwrap_iife(&module).is_some()
    })
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

fn pathological_entry_fixture(region_count: usize) -> String {
    let mut input = String::from("import { external } from \"./dep.js\";\n");
    for region in 0..region_count {
        input.push_str(&format!(
            r#"
                class Type{region} {{}}
                const left{region} = {region};
                function readLeft{region}() {{ return left{region} + external; }}
                const right{region} = {region};
                function readRight{region}() {{ return right{region}; }}
                function make{region}() {{ return new Type{region}(); }}
                const value{region} = make{region}();
            "#
        ));
    }
    input.push_str("export { ");
    for region in 0..region_count {
        input.push_str(&format!("value{region}, "));
    }
    input.push_str("};");
    input
}

fn cross_write_component_fixture(owner_count: usize) -> String {
    let mut input = String::new();
    for owner in 0..owner_count {
        input.push_str(&format!(
            "var state{owner} = 0;\nfunction read{owner}() {{ return state{owner}; }}\n"
        ));
    }
    input.push_str(
        r#"
            function spacer0() { return 0; }
            function spacer1() { return spacer0() + 1; }
            function spacer2() { return spacer1() + 1; }
            function spacer3() { return spacer2() + 1; }
            function mutateAll() {
        "#,
    );
    for owner in 0..owner_count {
        input.push_str(&format!("state{owner}++;\n"));
    }
    input.push_str(
        r#"
                return spacer3();
            }
            function runMutation() { return mutateAll(); }
            console.log(runMutation());
        "#,
    );
    input
}

fn cross_write_hub_with_local_writers_fixture(owner_count: usize) -> String {
    let mut input = String::new();
    for owner in 0..owner_count {
        input.push_str(&format!(
            "var state{owner} = 0;\nfunction read{owner}() {{ return state{owner}; }}\n"
        ));
    }
    input.push_str(
        r#"
            function spacer0() { return 0; }
            function spacer1() { return spacer0() + 1; }
            function spacer2() { return spacer1() + 1; }
            function spacer3() { return spacer2() + 1; }
        "#,
    );
    input.push_str("function mutateLocal0() { state0++; }\n");
    for separator in 0..4 {
        input.push_str(&format!(
            "function localSeparator{separator}() {{ return {separator}; }}\n"
        ));
    }
    input.push_str("function mutateAll() {\n");
    for owner in 0..owner_count {
        input.push_str(&format!("state{owner}++;\n"));
    }
    input.push_str(
        r#"
                return spacer3();
            }
            console.log(mutateAll());
        "#,
    );
    input
}

fn cross_write_hub_with_singleton_leaves_fixture(owner_count: usize) -> String {
    let mut input = String::new();
    for owner in 0..owner_count {
        input.push_str(&format!("var state{owner} = 0;\n"));
    }
    for owner in 0..owner_count {
        input.push_str(&format!(
            "function mutateLocal{owner}() {{ state{owner}++; }}\n"
        ));
    }
    input.push_str("function mutateAll() {\n");
    for owner in 0..owner_count {
        input.push_str(&format!("state{owner}++;\n"));
    }
    input.push_str(
        r#"
            }
            function separateA() { return 1; }
            function separateB() { return separateA() + 1; }
            console.log(mutateAll(), separateB());
        "#,
    );
    input
}

fn cross_write_hub_with_module_leaves_fixture(owner_count: usize) -> String {
    let mut input = String::new();
    for owner in 0..owner_count {
        input.push_str(&format!(
            "var state{owner} = 0;\nfunction read{owner}() {{ return state{owner}; }}\n"
        ));
    }
    for owner in 0..owner_count {
        input.push_str(&format!(
            "function mutateLocal{owner}() {{ state{owner}++; }}\nfunction runLocal{owner}() {{ mutateLocal{owner}(); }}\n"
        ));
    }
    input.push_str("function mutateAll() {\n");
    for owner in 0..owner_count {
        input.push_str(&format!("state{owner}++;\n"));
    }
    input.push_str(
        r#"
            }
            console.log(mutateAll());
        "#,
    );
    input
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
fn iife_with_function_level_return_restores_callable_entry_boundary() {
    let input = r#"
        (function() {
            if (window.disabled) return;

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

            const result = a5() + b5();
            console.log(result);
        })();
    "#;

    let modules = split(input).expect("the IIFE should remain safely splittable");
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .map(|(_, code, _)| code)
        .expect("the split should retain an entry module");
    assert!(
        entry.contains("return;") && (entry.contains("(()=>{") || entry.contains("(() =>")),
        "the entry should restore a function boundary around the lifted return:\n{entry}"
    );
    let pairs = modules
        .into_iter()
        .map(|(filename, code, _)| (filename, code))
        .collect::<Vec<_>>();
    assert_eq!(crate::validate_output_modules(&pairs), vec![]);
}

#[test]
fn iife_with_only_nested_returns_can_still_be_unwrapped() {
    let input = r#"
        (function() {
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

            const result = a5() + b5();
            console.log(result);
        })();
    "#;

    assert!(
        split(input).is_some(),
        "returns owned by nested functions must not block IIFE unwrapping"
    );
}

#[test]
fn iife_unwrap_declines_cross_scope_binding_collisions() {
    for (name, input) in [
        (
            "nested var and trailing import",
            r#"
                (function () {
                    try {
                        var shared = globalThis;
                    } catch {}
                })();
                import { value as shared } from "./dep.js";
            "#,
        ),
        (
            "inner var and trailing function",
            r#"
                (function () {
                    var shared = 1;
                })();
                function shared() { return 2; }
            "#,
        ),
        (
            "distinct direct vars",
            r#"
                (() => {
                    var shared = 1;
                })();
                var shared = 2;
            "#,
        ),
        (
            "closure capture and trailing var",
            r#"
                (function () {
                    var shared = 1;
                    globalThis.readInner = function () { return shared; };
                })();
                var shared = 2;
            "#,
        ),
    ] {
        assert!(
            !unwraps_first_iife(input),
            "{name} must retain the function boundary"
        );
    }
}

#[test]
fn iife_unwrap_declines_named_function_self_bindings() {
    let input = r#"
        (function shared() {
            globalThis.readInner = () => shared;
        })();
        var shared = 42;
    "#;

    assert!(
        !unwraps_first_iife(input),
        "a named function expression cannot lose its self-binding"
    );
}

#[test]
fn named_iife_self_binding_keeps_wrapper_in_split_entry() {
    let input = r#"
        (function shared() {
            globalThis.readInner = () => shared;
        })();
        var shared = 42;

        function a0() { return 1; }
        function a1() { return a0(); }
        function a2() { return a1(); }
        function a3() { return a2(); }
        function a4() { return a3(); }
        function a5() { return a4(); }

        function b0() { return 2; }
        function b1() { return b0(); }
        function b2() { return b1(); }
        function b3() { return b2(); }
        function b4() { return b3(); }
        function b5() { return b4(); }

        console.log(shared, a5(), b5());
    "#;

    let modules = split(input).expect("the guarded named-IIFE fixture should still split");
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .map(|(_, code, _)| code)
        .expect("the split should retain an entry module");
    assert!(
        entry.contains("function shared()")
            && entry.contains("readInner")
            && entry.contains("var shared = 42"),
        "the named IIFE self-binding needs the retained function boundary:\n{entry}"
    );
}

#[test]
fn iife_unwrap_ignores_bindings_that_keep_a_nested_scope() {
    let input = r#"
        (function () {
            if (globalThis.enabled) {
                let shared = 1;
                console.log(shared);
            }
            function readNested() {
                var other = 2;
                return other;
            }
            globalThis.readNested = readNested;
        })();
        var shared = 3;
        var other = 4;
    "#;

    assert!(
        unwraps_first_iife(input),
        "block lexical and nested-function bindings do not enter module scope"
    );
}

#[test]
fn iife_collision_keeps_wrapper_in_split_entry() {
    let input = r#"
        (function () {
            try {
                var shared = globalThis;
                globalThis.readInner = function () { return shared; };
            } catch {}
        })();
        import { value as shared } from "./dep.js";

        var a0 = () => 1;
        function a1() { return a0(); }
        function a2() { return a1(); }
        function a3() { return a2(); }
        function a4() { return a3(); }
        function a5() { return a4(); }

        function b0() { return 2; }
        function b1() { return b0(); }
        function b2() { return b1(); }
        function b3() { return b2(); }
        function b4() { return b3(); }
        function b5() { return b4(); }

        console.log(a5() + b5());
    "#;

    let modules = split(input).expect("the guarded IIFE fixture should still split");
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .map(|(_, code, _)| code)
        .expect("the split should retain an entry module");
    assert!(
        entry.contains("value as shared")
            && entry.contains("var shared = globalThis")
            && (entry.contains("(function()")
                || entry.contains("(()=>{")
                || entry.contains("(() =>")),
        "the import and captured var need the retained function boundary:\n{entry}"
    );
}

#[test]
fn recoverably_parsed_sloppy_iife_is_not_split_into_modules() {
    let input = r#"
        (function() {
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

            with (window.runtimeScope) {
                console.log(a5(), b5(), value);
            }
        })();
    "#;

    assert!(
        split(input).is_none(),
        "a module-goal recovery must not authorize ESM extraction from a sloppy script"
    );
}

#[test]
fn parenthesized_delete_identifier_is_not_split_into_modules() {
    let input = r#"
        (function() {
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

            var temporary = a5() + b5();
            delete (temporary);
            console.log(temporary);
        })();
    "#;

    assert!(
        split(input).is_none(),
        "sloppy-only delete syntax must not authorize scope-hoist extraction"
    );
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
fn writer_stays_with_mutable_top_level_binding() {
    let input = r#"
        var state = 0;
        function readState() { return state; }
        function formatState() { return `state:${readState()}`; }
        function stateIsEven() { return readState() % 2 === 0; }
        function stateLabel() { return stateIsEven() ? formatState() : "odd"; }

        function increment() { state++; }
        function incrementTwice() { increment(); increment(); }
        function updateState() { incrementTwice(); }
        function runUpdate() { updateState(); }
        function reportUpdate() { runUpdate(); return "updated"; }

        function helperA1() { return 1; }
        function helperA2() { return helperA1() + 1; }
        function helperA3() { return helperA2() * 2; }
        function helperA4() { return helperA3() + 3; }
        function publicA() { return helperA4(); }

        console.log(stateLabel(), reportUpdate(), publicA());
    "#;

    let modules = split(input).expect("the independent reader group should still allow a split");
    let writer = modules
        .iter()
        .find(|(_, code, _)| code.contains("state++"))
        .expect("one output should retain the state write");
    assert!(
        writer.1.contains("var state"),
        "the writer must stay in the module that declares the mutable binding, not import it:\n{}",
        writer.1
    );
}

#[test]
fn duplicate_top_level_var_writers_share_one_output_module() {
    let input = r#"
        var shared = false;
        function firstRead() { return shared; }
        function firstWrite() { shared = true; }
        function firstReset() { shared = false; }
        function firstRun() { firstWrite(); return firstRead(); }

        function helperA1() { return 1; }
        function helperA2() { return helperA1() + 1; }
        function helperA3() { return helperA2() * 2; }
        function helperA4() { return helperA3() + 3; }
        function publicA() { return helperA4(); }

        var shared = false;
        function secondRead() { return shared; }
        function secondWrite() { shared = true; }
        function secondReset() { shared = false; }
        function secondRun() { secondWrite(); return secondRead(); }

        console.log(firstRun(), secondRun(), publicA());
    "#;

    let modules = split(input).expect("the independent helper group should still allow a split");
    assert!(
        modules
            .iter()
            .all(|(_, code, _)| !code.contains("import { shared")),
        "a repeated top-level var is one binding and must never become an imported writer:\n{modules:#?}"
    );
    let first_owner = modules
        .iter()
        .find(|(_, code, _)| code.contains("function firstWrite"))
        .expect("one module should contain the first writer");
    assert!(
        first_owner.1.contains("function secondWrite"),
        "both writers of the repeated var binding must share one owner:\n{}",
        first_owner.1
    );
}

#[test]
fn nested_and_direct_module_var_declarations_share_one_output_module() {
    let input = r#"
        if (gate) {
            var shared = 123;
        }
        function firstRead() { return shared; }
        function firstUse() { return firstRead(); }

        function helperA1() { return 1; }
        function helperA2() { return helperA1() + 1; }
        function helperA3() { return helperA2() * 2; }
        function helperA4() { return helperA3() + 3; }
        function publicA() { return helperA4(); }

        var shared;
        function secondRead() { return shared; }
        function secondUse() { return secondRead(); }

        console.log(firstUse(), secondUse(), publicA());
    "#;

    let modules = split(input).expect("the independent helper group should still allow a split");
    let nested_declaration = modules
        .iter()
        .find(|(_, code, _)| code.contains("if (gate)"))
        .expect("one output should retain the nested var declaration");
    assert!(
        nested_declaration.1.contains("var shared = 123;")
            && nested_declaration.1.contains("var shared;"),
        "both declarations of one module-scoped var must stay together:\n{modules:#?}"
    );
    assert!(
        modules
            .iter()
            .filter(|module| module.0.as_str() != nested_declaration.0.as_str())
            .all(|(_, code, _)| !code.contains("var shared")),
        "no second module may own another copy of the shared binding:\n{modules:#?}"
    );
    let pairs = modules
        .iter()
        .map(|(filename, code, _)| (filename.clone(), code.clone()))
        .collect::<Vec<_>>();
    assert_eq!(crate::validate_output_modules(&pairs), vec![]);
}

#[test]
fn duplicate_var_declarations_follow_a_bare_entry_writer() {
    // A bare top-level writer statement is folded into the entry together
    // with everything in its write group. The write edge reaches only the
    // last declaring item, so both distant `var shared;` declarations must
    // travel with it — otherwise the entry imports the binding from the
    // cluster keeping the first declaration while also redeclaring it.
    let input = r#"
        var shared;
        function libA1() { return 1; }
        function libA2() { return libA1() + 1; }
        function libA3() { return libA2(); }
        var stateA = { n: libA3() };
        function libB1() { return 10; }
        function libB2() { return libB1() * 2; }
        function libB3() { return libB2(); }
        var stateB = { n: libB3() };
        function entryUse() { return stateA.n + stateB.n; }
        shared = entryUse();
        var shared;
        console.log(shared, stateA, stateB);
    "#;

    let modules = split(input).expect("the two helper groups should still allow a split");
    for (filename, code, _) in &modules {
        assert!(
            !code.contains("import { shared") && !code.contains("shared }"),
            "a hoisted binding written by the entry must never be imported ({filename}):\n{code}"
        );
    }
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .expect("an entry module should exist");
    assert!(
        entry.1.contains("var shared") && entry.1.contains("shared = entryUse()"),
        "the entry must own both the declarations and the bare writer:\n{}",
        entry.1
    );
}

#[test]
fn inspection_bounds_cross_item_write_components() {
    // Seven owner clusters plus the writer cluster sit exactly at the cap, so
    // the mutable bindings remain with their writer as useful same-module
    // evidence even though inspection output is not required to execute.
    let at_limit =
        cross_write_component_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS.saturating_sub(1));
    let at_limit = split_scope_hoisted_with_mode(
        &at_limit,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::NestedModule,
    )
    .expect("the at-limit fixture should split");
    let at_limit_writer = at_limit
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateAll"))
        .expect("inspection output should contain the writer");
    assert!(
        at_limit_writer.code.contains("var state0"),
        "an at-limit write component should remain merged:\n{}",
        at_limit_writer.code
    );

    // Adding one owner takes the write-connected component above the cap. In
    // Inspect mode every write edge in that component is skipped, avoiding a
    // transitive merge of otherwise independent owner clusters.
    let above_limit = cross_write_component_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS);
    let inspection = split_scope_hoisted_with_mode(
        &above_limit,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::NestedModule,
    )
    .expect("the above-limit fixture should split");
    let inspection_writer = inspection
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateAll"))
        .expect("inspection output should contain the writer");
    assert!(
        !inspection_writer.code.contains("var state0"),
        "an oversized write component should preserve its finer boundaries:\n{}",
        inspection_writer.code
    );
    assert_eq!(
        inspection
            .modules
            .iter()
            .filter(|module| module.code.contains("var state"))
            .count(),
        INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS,
        "each mutable owner cluster should remain separately visible"
    );
    let mut oversized_contexts = inspection
        .modules
        .iter()
        .filter(|module| {
            module.code.contains("var state") || module.code.contains("function mutateAll")
        })
        .map(|module| &module.inspection_context_ranges);
    let first_context = oversized_contexts
        .next()
        .expect("oversized component should expose context provenance");
    assert!(
        !first_context.is_empty(),
        "split write-component modules should retain their shared coarse context"
    );
    assert!(
        oversized_contexts.all(|context| context == first_context),
        "every fine module from one write component should share one context range set"
    );

    // Executable mode cannot import a binding and then assign to it, so its
    // correctness merge remains unconditional for the same large component.
    let executable =
        split_scope_hoisted(&above_limit).expect("the executable fixture should split");
    let executable_writer = executable
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateAll"))
        .expect("executable output should contain the writer");
    assert!(
        executable_writer.code.contains("var state0")
            && executable_writer.code.contains(&format!(
                "var state{}",
                INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS - 1
            )),
        "executable output must keep every mutable owner with the writer:\n{}",
        executable_writer.code
    );
    assert!(
        executable
            .modules
            .iter()
            .all(|module| module.inspection_context_ranges.is_empty()),
        "normal executable output must not expose Inspect-only context"
    );
}

#[test]
fn inspection_retains_bounded_leaf_writes_inside_a_hub_component() {
    let input =
        cross_write_hub_with_local_writers_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS);
    let trace = trace_scope_hoisted(&input).expect("the research trace should parse");
    assert_eq!(
        trace.leaf_candidate_output_cluster_count,
        trace.component_cap_output_cluster_count
    );
    assert!(trace.bounded_leaf_restoration_accepted);
    let inspection = split_scope_hoisted_with_mode(
        &input,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::NestedModule,
    )
    .expect("the hub fixture should split");

    let local_writer = inspection
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateLocal0"))
        .expect("inspection output should contain the local writer");
    assert!(
        local_writer.code.contains("var state0"),
        "a bounded degree-one write should retain its mutable owner:\n{}",
        local_writer.code
    );

    let hub = inspection
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateAll"))
        .expect("inspection output should contain the hub writer");
    assert!(
        !hub.code.contains("var state0"),
        "the high-degree writer must not glue the owner clusters together:\n{}",
        hub.code
    );
}

#[test]
fn inspection_backs_off_when_leaf_writes_reduce_module_count() {
    let input =
        cross_write_hub_with_module_leaves_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS);
    let trace = trace_scope_hoisted(&input).expect("the research trace should parse");

    assert!(trace.eligible);
    assert!(
        trace.leaf_candidate_output_cluster_count < trace.component_cap_output_cluster_count,
        "the fixture must exercise module-cluster contraction: {trace:#?}"
    );
    assert!(!trace.bounded_leaf_restoration_accepted);
    assert_eq!(
        trace.post_write_cluster_count, trace.signal_cluster_count,
        "every write edge belongs to the oversized component and should be skipped"
    );
}

#[test]
fn inspection_does_not_accept_offsetting_component_count_changes() {
    fn declaration(name: &str, writes: &[&str]) -> TopLevelItem {
        let written_names: HashSet<Atom> = writes.iter().map(|name| Atom::from(*name)).collect();
        TopLevelItem {
            declared_names: vec![Atom::from(name)],
            top_level_var_names: Vec::new(),
            referenced_names: written_names.clone(),
            written_names,
            is_module_decl: false,
        }
    }

    let items = vec![
        declaration("singletonOwner", &[]),
        declaration("singletonWriter", &["singletonOwner"]),
        declaration("moduleOwnerA", &[]),
        declaration("moduleOwnerB", &[]),
        declaration("moduleWriterA", &["moduleOwnerA"]),
        declaration("moduleWriterB", &[]),
        TopLevelItem {
            declared_names: Vec::new(),
            top_level_var_names: Vec::new(),
            referenced_names: HashSet::new(),
            written_names: HashSet::new(),
            is_module_decl: false,
        },
    ];
    let graph = build_reference_graph(&items);
    let mut uf = UnionFind::new(items.len());
    uf.union(2, 3);
    uf.union(4, 5);
    let before = canonical_cluster_ids(&uf, items.len());

    // With a cap of one, each two-root write component is oversized. Merging
    // the singleton pair would add one output module, while merging the two
    // established module roots would remove one. A file-level count check
    // would see a misleading net zero; component-wise checks reject both.
    let decision = merge_bounded_cross_item_writes(&items, &graph, &mut uf, 1);

    assert_eq!(decision.component_cap_output_clusters, 3);
    assert_eq!(decision.leaf_candidate_output_clusters, 3);
    assert!(!decision.bounded_leaf_restoration_accepted);
    assert!(decision.restored_components.is_empty());
    assert_eq!(canonical_cluster_ids(&uf, items.len()), before);
}

#[test]
fn inspection_backs_off_when_leaf_writes_promote_singletons_to_modules() {
    let input =
        cross_write_hub_with_singleton_leaves_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS);
    let trace = trace_scope_hoisted(&input).expect("the research trace should parse");

    assert!(trace.eligible);
    assert!(
        trace.leaf_candidate_output_cluster_count > trace.component_cap_output_cluster_count,
        "the fixture must exercise singleton promotion: {trace:#?}"
    );
    assert!(!trace.bounded_leaf_restoration_accepted);
    let local_writer_item = trace
        .items
        .iter()
        .find(|item| {
            item.declared_names
                .iter()
                .any(|name| name == "mutateLocal0")
        })
        .expect("the trace should contain the local writer")
        .index;
    let local_owner_item = trace
        .items
        .iter()
        .find(|item| item.declared_names.iter().any(|name| name == "state0"))
        .expect("the trace should contain the local owner")
        .index;
    let local_write_edge = trace
        .cross_write_edges
        .iter()
        .find(|edge| edge.writer_item == local_writer_item && edge.owner_item == local_owner_item)
        .expect("the trace should contain the local writer/owner edge");
    assert!(!local_write_edge.kept_by_inspect_policy);

    let inspection = split_scope_hoisted_with_mode(
        &input,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::NestedModule,
    )
    .expect("the independent pair should preserve a split after backoff");
    assert_eq!(
        inspection.modules.len(),
        trace.component_cap_output_cluster_count,
        "the output-count backoff should restore the component-cap partition"
    );
}

#[test]
fn scope_hoist_trace_reports_cross_write_hub_topology() {
    let input = cross_write_component_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS);
    let trace = trace_scope_hoisted(&input).expect("the research trace should parse");

    assert!(trace.eligible);
    assert!(trace.would_split);
    assert_eq!(
        trace.cross_write_edges.len(),
        INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS
    );
    assert!(trace.items.iter().all(|item| item.source_range.is_some()));
    for edge in &trace.cross_write_edges {
        assert_eq!(
            edge.writer_target_cluster_degree,
            INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS
        );
        assert_eq!(
            edge.component_cluster_count,
            INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS + 1
        );
        assert_eq!(edge.leaf_component_cluster_count, 1);
        assert!(!edge.kept_by_inspect_policy);
        assert_ne!(edge.writer_cluster, edge.owner_cluster);
    }
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
    // The b-group consumes `exported` from the entry without the entry
    // referencing the b-group back: an entry-side consumer of b5 would form
    // an entry↔chunk cycle, and the executable-order fold plus cycle merge
    // would (correctly) collapse the chunk into the entry, leaving nothing
    // for the partial var export to split.
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
            console.log(a4());
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
fn executable_partition_does_not_create_a_global_singleton_cycle() {
    const REGION_COUNT: usize = 64;
    let input = pathological_entry_fixture(REGION_COUNT);

    let inspection = split_scope_hoisted_with_mode(
        &input,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::DirectAsset,
    )
    .expect("inspection mode should split independent regions");
    let executable = split_scope_hoisted(&input).expect("executable mode should split");

    assert_eq!(
        inspection.modules.len(),
        REGION_COUNT * 3 + 1,
        "fixture should expose three clusters per region plus the entry"
    );
    assert_eq!(
        executable.modules.len(),
        inspection.modules.len(),
        "independent singleton roots must not be folded into one entry that makes their clusters cyclic"
    );

    let entry = executable
        .modules
        .iter()
        .find(|module| module.is_entry)
        .expect("export declaration should remain in the entry");
    for class_name in ["Type0", "Type63"] {
        assert!(
            !entry.code.contains(&format!("class {class_name}")),
            "singleton class {class_name} should be assigned to a local cluster:\n{}",
            entry.code
        );
    }

    let result_modules = ["value0 =", "value63 ="].map(|needle| {
        executable
            .modules
            .iter()
            .position(|module| module.code.contains(needle))
            .unwrap_or_else(|| panic!("missing result declaration {needle}"))
    });
    assert_ne!(result_modules[0], result_modules[1]);
}

#[test]
fn emission_relation_planning_scales_with_symbols_not_cluster_product() {
    const REGION_COUNT: usize = 64;
    let input = pathological_entry_fixture(REGION_COUNT);

    reset_emit_relation_symbol_probe_count();
    let result = split_scope_hoisted(&input).expect("fixture should split");
    let probes = emit_relation_symbol_probe_count();

    assert_eq!(result.modules.len(), REGION_COUNT * 3 + 1);
    assert!(
        probes <= result.modules.len() * 16,
        "emission should index cross-cluster symbols instead of probing every cluster pair; \
         observed {probes} symbol probes for {} modules",
        result.modules.len()
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
fn inspection_rendering_keeps_synthetic_clusters_separate() {
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

    let result = split_scope_hoisted_with_mode(
        input,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::DirectAsset,
    )
    .expect("inspection mode should split");
    assert_eq!(result.modules.len(), 6, "cycle should remain split");
    assert!(
        result
            .modules
            .iter()
            .any(|module| module.code.contains("from \"./entry.js\"")),
        "inspection output should retain the synthetic import cycle"
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

#[test]
fn top_level_writer_statement_folds_its_cluster_into_entry() {
    // A bare top-level write must keep its source position relative to the
    // entry statements around it. Emitting `state++` inside a lazily imported
    // chunk runs it at import time — before entry statements that preceded it
    // in the source — silently changing observable values.
    let input = r#"
        var state = 0;
        function readState() { return state; }
        function formatState() { return `state:${readState()}`; }
        function stateIsEven() { return readState() % 2 === 0; }
        function stateLabel() { return stateIsEven() ? formatState() : "odd"; }

        function helperA1() { return 1; }
        function helperA2() { return helperA1() + 1; }
        function helperA3() { return helperA2() * 2; }
        function helperA4() { return helperA3() + 3; }
        function publicA() { return helperA4(); }

        function helperB1() { return 5; }
        function helperB2() { return helperB1() + 5; }
        function helperB3() { return helperB2() * 6; }
        function helperB4() { return helperB3() + 7; }
        function publicB() { return helperB4(); }

        console.log("before-increment", readState());
        state++;
        console.log("after-increment", readState(), publicA(), publicB());
    "#;

    let modules = split(input).expect("the independent helper groups should still split");
    let entry = modules
        .iter()
        .find(|(_, _, is_entry)| *is_entry)
        .expect("should have an entry module");
    let decl_pos = entry
        .1
        .find("var state")
        .expect("the mutable binding must fold into the entry alongside its writer");
    let before_pos = entry
        .1
        .find("before-increment")
        .expect("entry should keep the preceding log statement");
    let write_pos = entry
        .1
        .find("state++")
        .expect("the top-level write must execute from the entry");
    assert!(
        decl_pos < before_pos && before_pos < write_pos,
        "entry must retain source execution order:\n{}",
        entry.1
    );
}

#[test]
fn unreachable_effectful_singleton_folds_into_entry() {
    const REGION_COUNT: usize = 64;
    let mut input = pathological_entry_fixture(REGION_COUNT);
    // An unreferenced singleton whose initializer runs code, parked mid-file
    // so the topological attachment picks an interior anchor: wherever
    // partitioning parks it, running the split output must still execute it.
    let mid_marker = "class Type32 ";
    let mid = input
        .find(mid_marker)
        .expect("fixture should have region 32");
    input.insert_str(
        mid,
        "const sideEffectProbe = globalThis.registerPolyfill();\n",
    );

    let executable = split_scope_hoisted(&input).expect("fixture should split");
    let entry = executable
        .modules
        .iter()
        .find(|module| module.is_entry)
        .expect("should have an entry module");

    // BFS over emitted import specifiers from the entry.
    let imports_of = |code: &str| -> Vec<String> {
        code.match_indices("\"./")
            .filter_map(|(start, _)| {
                let rest = &code[start + 3..];
                rest.find('"').map(|end| rest[..end].to_string())
            })
            .collect()
    };
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue = vec![entry.filename.clone()];
    while let Some(filename) = queue.pop() {
        if !reachable.insert(filename.clone()) {
            continue;
        }
        let Some(module) = executable
            .modules
            .iter()
            .find(|module| module.filename == filename)
        else {
            continue;
        };
        queue.extend(imports_of(&module.code));
    }

    let probe_host = executable
        .modules
        .iter()
        .find(|module| module.code.contains("registerPolyfill"))
        .expect("the effectful singleton must be emitted somewhere");
    assert!(
        reachable.contains(&probe_host.filename),
        "the module holding the side-effectful initializer must be reachable from the entry, \
         or the split output never runs it; it landed unreachable in {} \
         (reachable: {} of {} modules)",
        probe_host.filename,
        reachable.len(),
        executable.modules.len()
    );
}

fn distant_write_hub_fixture() -> &'static str {
    r#"
        var alpha = 0;
        function bumpAlpha() { alpha += 1; return alpha; }
        function alphaView() { return bumpAlpha(); }

        function a1() { return 1; }
        function a2() { return a1() + 1; }
        function a3() { return a2() * 2; }
        function a4() { return a3() + 3; }
        function a5() { return a4() - 1; }

        function b1() { return 7; }
        function b2() { return b1() + 10; }
        function b3() { return b2() * 20; }
        function b4() { return b3() + 30; }
        function b5() { return b4() - 10; }

        function runtimeHub() { alpha += 1; omega += 1; return alpha + omega; }
        var omega = 0;
        function omegaView() { return omega; }

        const total = alphaView() + omegaView() + runtimeHub() + a5() + b5();
        console.log(total);
    "#
}

#[test]
fn direct_inspect_skips_distant_cross_write_merges() {
    let result = split_scope_hoisted_with_mode(
        distant_write_hub_fixture(),
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::DirectAsset,
    )
    .expect("inspect mode should split the hub fixture");
    let owner = result
        .modules
        .iter()
        .find(|module| module.code.contains("var alpha"))
        .expect("one module should own the distant mutable binding");
    assert!(
        !owner.code.contains("runtimeHub"),
        "a distant runtime hub must not glue onto the mutable owner in a direct asset:\n{}",
        owner.code
    );
    assert!(
        owner.code.contains("bumpAlpha"),
        "the adjacent writer must stay with its mutable owner:\n{}",
        owner.code
    );
}

#[test]
fn direct_inspect_keeps_adjacent_cross_write_merges() {
    let result = split_scope_hoisted_with_mode(
        distant_write_hub_fixture(),
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::DirectAsset,
    )
    .expect("inspect mode should split the hub fixture");
    let hub = result
        .modules
        .iter()
        .find(|module| module.code.contains("runtimeHub"))
        .expect("one module should hold the runtime hub");
    assert!(
        hub.code.contains("var omega"),
        "a touching writer/owner pair remains same-module evidence:\n{}",
        hub.code
    );
}

#[test]
fn nested_inspect_keeps_component_cap_merges() {
    // The same small write component that the adjacency policy splits stays
    // merged on the nested path, where measured contiguity is too weak to
    // trust item order.
    let input =
        cross_write_component_fixture(INSPECT_MAX_CROSS_WRITE_COMPONENT_CLUSTERS.saturating_sub(1));
    let nested = split_scope_hoisted_with_mode(
        &input,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::NestedModule,
    )
    .expect("the fixture should split on the nested path");
    let nested_writer = nested
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateAll"))
        .expect("nested output should contain the writer");
    assert!(
        nested_writer.code.contains("var state0"),
        "an at-limit write component stays merged on the nested path:\n{}",
        nested_writer.code
    );

    let direct = split_scope_hoisted_with_mode(
        &input,
        ScopeHoistRenderMode::Inspect,
        ScopeHoistSource::DirectAsset,
    )
    .expect("the fixture should split on the direct path");
    let direct_writer = direct
        .modules
        .iter()
        .find(|module| module.code.contains("function mutateAll"))
        .expect("direct output should contain the writer");
    assert!(
        !direct_writer.code.contains("var state0"),
        "the same non-adjacent component splits on the direct path:\n{}",
        direct_writer.code
    );
}
