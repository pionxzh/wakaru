use wakaru_core::unpack;
use wakaru_core::unpack_raw;
use wakaru_core::DecompileOptions;

fn expect_unpack(source: &str, filename: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: filename.to_string(),
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

fn expect_unpack_raw(source: &str) -> Vec<(String, String)> {
    let output =
        unpack_raw(source, &DecompileOptions::default()).expect("unpack_raw should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

/// Minimal synthetic esbuild bundle: two lazy modules + entry code.
fn make_bundle(helper: &str, helper_name: &str) -> String {
    format!(
        r#"
var {helper_name} = {helper};
var mod_a = {helper_name}(() => {{
    mod_a_val = 42;
}});
var mod_b = {helper_name}(() => {{
    mod_b_val = "hello";
}});
var mod_c = {helper_name}(() => {{ mod_c_val = true; }});
var mod_d = {helper_name}(() => {{ mod_d_val = null; }});
var mod_e = {helper_name}(() => {{ mod_e_val = undefined; }});
// entry
mod_a();
mod_b();
mod_c();
mod_d();
mod_e();
console.log(mod_a_val);
"#
    )
}

#[test]
fn esbuild_detects_minified_lazy_helper() {
    // esbuild's minified __esm: (q, K) => () => (q && (K = q(q = 0)), K)
    let bundle = make_bundle("(q,K)=>()=>(q&&(K=q(q=0)),K)", "y");
    let pairs = expect_unpack(&bundle, "bundle.js");

    // Should split into factory modules + entry
    assert!(
        pairs.len() >= 6,
        "expected ≥6 modules (5 factories + entry), got {}",
        pairs.len()
    );

    let has_entry = pairs.iter().any(|(name, _)| name == "entry.js");
    assert!(
        has_entry,
        "entry.js not found: {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );

    let has_mod_a = pairs.iter().any(|(name, _)| name == "mod_a.js");
    assert!(has_mod_a, "mod_a.js not found");
}

#[test]
fn esbuild_detects_minified_cjs_helper() {
    // esbuild's minified __commonJS: (q, K) => () => (K || q((K = {exports:{}}).exports, K), K.exports)
    let bundle = make_bundle(
        "(q,K)=>()=>(K||q((K={exports:{}}).exports,K),K.exports)",
        "m",
    );
    let pairs = expect_unpack(&bundle, "bundle.js");

    assert!(pairs.len() >= 6, "expected ≥6 modules, got {}", pairs.len());
    assert!(
        pairs.iter().any(|(n, _)| n == "entry.js"),
        "missing entry.js"
    );
}

#[test]
fn esbuild_factory_code_is_non_empty() {
    let bundle = make_bundle("(q,K)=>()=>(q&&(K=q(q=0)),K)", "y");
    let pairs = expect_unpack(&bundle, "bundle.js");

    for (name, code) in &pairs {
        assert!(!code.trim().is_empty(), "module {name} has empty code");
    }
}

#[test]
fn esbuild_scope_hoisted_modules_are_extracted() {
    // Synthetic bundle: 5 factory modules + scope-hoisted entry containing
    // an __export helper and 2 scope-hoisted modules.
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
var ns_b = {};
__export(ns_b, { add: () => add, PI: () => PI });
var PI = 3.14;
function add(a, b) { return a + b; }
console.log(greet(), add(1, 2));
export { ns_a, ns_b };
"#;
    // Use raw unpack first to verify extraction without pipeline interference
    let raw_pairs = expect_unpack_raw(bundle);
    let raw_names: Vec<&str> = raw_pairs.iter().map(|(n, _)| n.as_str()).collect();

    // 5 factory modules + 2 scope-hoisted modules + entry.js
    assert!(
        raw_pairs.len() >= 8,
        "expected ≥8 modules (5 factories + 2 scope-hoisted + entry), got {}: {raw_names:?}",
        raw_pairs.len()
    );

    assert!(
        raw_names.contains(&"ns_a.js"),
        "ns_a.js not found in {raw_names:?}"
    );
    assert!(
        raw_names.contains(&"ns_b.js"),
        "ns_b.js not found in {raw_names:?}"
    );

    // ns_a should contain the greet function
    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        ns_a_code.contains("greet"),
        "ns_a.js should contain 'greet': {ns_a_code}"
    );

    // ns_b should contain add, PI, and the console.log side effect
    let ns_b_code = &raw_pairs.iter().find(|(n, _)| n == "ns_b.js").unwrap().1;
    assert!(
        ns_b_code.contains("add") && ns_b_code.contains("3.14"),
        "ns_b.js should contain 'add' and PI: {ns_b_code}"
    );

    // console.log(greet(), add(1, 2)) references `add` from ns_b's exports,
    // so it stays with ns_b as a module-level side effect.
    assert!(
        ns_b_code.contains("console.log"),
        "ns_b.js should contain console.log (references module binding `add`): {ns_b_code}"
    );
}

/// Known limitation: entry expressions referencing a last-module binding
/// are absorbed into that module because minified bundles have no
/// structural marker distinguishing them from module-level side effects.
/// See the KNOWN LIMITATION comment in esbuild.rs.
#[test]
fn esbuild_last_module_entry_ref_absorbed() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
var ns_b = {};
__export(ns_b, { value: () => value });
var value = 42;
console.log("entry", value);
export { ns_a, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // The entry `console.log("entry", value)` references `value` from ns_b.
    // In minified output there is no structural marker to distinguish it
    // from a module-level side effect. It currently stays with ns_b.
    let ns_b_code = &raw_pairs.iter().find(|(n, _)| n == "ns_b.js").unwrap().1;
    assert!(
        ns_b_code.contains("console.log"),
        "known limitation: entry expression referencing last-module binding is absorbed into ns_b"
    );
}

#[test]
fn esbuild_member_property_does_not_extend_scope_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { read: () => read });
function read(obj) { return obj.value; }
var value = "entry";
export { ns_a, value };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        !ns_a_code.contains("value = \"entry\""),
        "member property name should not pull unrelated value binding into ns_a: {ns_a_code}"
    );
}

#[test]
fn esbuild_export_decl_extends_last_scope_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { value: () => value });
export var value = read();
function read() { return "module"; }
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        ns_a_code.contains("export var value") && ns_a_code.contains("function read"),
        "export declarations should count as module declarations and include referenced helpers: {ns_a_code}"
    );
}

#[test]
fn esbuild_shadowed_name_does_not_extend_last_scope_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { read: () => read });
function read() {
    function inner() {
        let helper = "local";
        return helper;
    }
    return inner();
}
function helper() { return "entry"; }
export { ns_a, helper };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        !ns_a_code.contains("function helper"),
        "shadowed local helper should not pull top-level helper into ns_a: {ns_a_code}"
    );
}

#[test]
fn esbuild_shadowed_trailing_expr_does_not_extend_last_scope_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { read: () => read });
function read() { return "module"; }
(function() {
    let read = "entry";
    console.log(read);
})();
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        !ns_a_code.contains("console.log"),
        "shadowed trailing expression should stay out of ns_a: {ns_a_code}"
    );
}

#[test]
fn esbuild_destructured_export_decl_extends_last_scope_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { value: () => value });
var source = { value: "module" };
export var { value } = source;
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        ns_a_code.contains("export var { value }"),
        "destructured export declarations should count as module declarations: {ns_a_code}"
    );
}

#[test]
fn esbuild_scope_hoisted_duplicate_namespace_names() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var a = {};
__export(a, { x: () => x });
var x = 1;
var a = {};
__export(a, { y: () => y2 });
var y2 = 2;
console.log(x, y2);
"#;
    let raw_pairs = expect_unpack_raw(bundle);
    let raw_names: Vec<&str> = raw_pairs.iter().map(|(n, _)| n.as_str()).collect();

    assert!(
        raw_names.contains(&"a.js"),
        "a.js not found in {raw_names:?}"
    );
    assert!(
        raw_names.contains(&"a_2.js"),
        "a_2.js (deduped) not found in {raw_names:?}"
    );
}

#[test]
fn scope_module_skips_factory_owned_suffixes() {
    // Factories own `a.js` (from `var a = y(...)`) and the scope boundary
    // also uses namespace atom `a`.  The counter-based dedup used to assign
    // `a_2.js` which shadows the CLI-deduped factory `a_2.js` when the
    // CLI separately deduplicates `a.js` → `a_2.js`.  The probe loop must
    // skip all taken names.
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var a = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var a = {};
__export(a, { greet: () => greet });
function greet() { return "hello"; }
export { a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);
    let raw_names: Vec<&str> = raw_pairs.iter().map(|(n, _)| n.as_str()).collect();

    // Factory gets `a.js`. Scope module must NOT also claim `a.js`;
    // it should get `a_2.js` (or higher if `a_2.js` is also taken).
    let scope_module = raw_pairs
        .iter()
        .find(|(_, code)| code.contains("greet"))
        .expect("should find scope module containing greet");
    assert_ne!(
        scope_module.0, "a.js",
        "scope module should not shadow factory a.js: {raw_names:?}"
    );

    // The scope module's filename must exist in the raw output (not orphaned).
    assert!(
        raw_names.contains(&scope_module.0.as_str()),
        "scope module filename {} should be in output: {raw_names:?}",
        scope_module.0
    );
}

#[test]
fn scope_module_skips_cli_deduped_factory_suffixes() {
    // Factories `a` and `A` produce raw filenames `a.js` and `A.js`.
    // The CLI case-dedup turns them into `a.js` and `A_2.js`.
    // A scope module with namespace `A_2` must not claim `A_2.js`;
    // it should probe past it to `A_2_2.js`.
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var a = y(() => { v1 = 1; });
var A = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var c = {};
__export(c, { use: () => use });
function use() { return greet(); }
var A_2 = {};
__export(A_2, { greet: () => greet });
function greet() { return "hello"; }
export { c, A_2 };
"#;
    let raw_pairs = expect_unpack_raw(bundle);
    let raw_names: Vec<&str> = raw_pairs.iter().map(|(n, _)| n.as_str()).collect();

    // The scope module exporting `greet` must not shadow the CLI-deduped
    // factory `A_2.js`.
    let greet_module = raw_pairs
        .iter()
        .find(|(_, code)| code.contains("function greet()"))
        .expect("should find scope module declaring greet");

    assert_ne!(
        greet_module.0.to_ascii_lowercase(),
        "a_2.js",
        "scope module should not shadow CLI-deduped factory A_2.js: {raw_names:?}"
    );

    // The `c` module imports greet — verify its import target matches greet's actual filename.
    let c_module = raw_pairs
        .iter()
        .find(|(_, code)| code.contains("function use()"))
        .expect("should find scope module declaring use");
    let import_target = c_module
        .1
        .lines()
        .find(|l| l.contains("import") && l.contains("greet"))
        .unwrap_or("");
    assert!(
        import_target.contains(&greet_module.0),
        "c module should import greet from {}, but import line is: {import_target}",
        greet_module.0
    );
}

#[test]
fn small_file_with_few_factories_is_not_detected() {
    // Only 2 factories — below the threshold of 5, so we should NOT detect as esbuild.
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var a = y(() => { x = 1; });
var b = y(() => { z = 2; });
a(); b();
"#;
    let pairs = expect_unpack(bundle, "bundle.js");

    // Falls through to single-module path
    assert_eq!(
        pairs.len(),
        1,
        "should not detect as esbuild bundle with only 2 factories"
    );
}

#[test]
fn single_boundary_without_esm_export_is_not_detected() {
    // One __export boundary but the namespace is NOT re-exported via ESM.
    // This pattern can appear in non-bundled code — it should NOT be
    // treated as an esbuild bundle.
    let code = r#"
var defProp = Object.defineProperty;
var helper = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var api = {};
helper(api, { greet: () => greet });
function greet() { return "hello"; }
console.log(api.greet());
"#;
    let pairs = expect_unpack(code, "app.js");

    assert_eq!(
        pairs.len(),
        1,
        "single boundary without ESM export should not split: {:?}",
        pairs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
    );
}

/// Factory modules that reference bindings from scope-hoisted modules should
/// get synthesized imports, and the scope-hoisted modules should export those
/// bindings.  Without this, Dead Code Elimination incorrectly removes code
/// that appears unreferenced within its own module but is used by factories.
#[test]
fn factory_referencing_scope_hoisted_binding_gets_import() {
    // Synthetic bundle: 5 factory modules (f1..f4 are trivial, f5 references
    // `helperFn` which is declared in scope-hoisted module ns_a).
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { var result = helperFn(42); });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
function helperFn(x) { return x + 1; }
var ns_b = {};
__export(ns_b, { value: () => value });
var value = 99;
export { ns_a, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);
    let raw_names: Vec<&str> = raw_pairs.iter().map(|(n, _)| n.as_str()).collect();

    // Verify we got the expected modules.
    assert!(
        raw_pairs.len() >= 7,
        "expected factories + scope modules + entry, got {}: {raw_names:?}",
        raw_pairs.len()
    );

    // f5 references helperFn from ns_a — it should have an import statement.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("helperFn"),
        "f5.js should import helperFn from the scope-hoisted module:\n{f5_code}"
    );

    // ns_a should export helperFn (it's referenced by f5, a factory module).
    let ns_a_code = &raw_pairs
        .iter()
        .find(|(_, code)| code.contains("function greet"))
        .unwrap()
        .1;
    assert!(
        ns_a_code.contains("export") && ns_a_code.contains("helperFn"),
        "ns_a should export helperFn (referenced by factory f5):\n{ns_a_code}"
    );
}

/// When multiple factory modules reference different bindings from the same
/// scope-hoisted module, all referenced bindings should be exported.
#[test]
fn multiple_factories_referencing_scope_hoisted_bindings() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { var x = add(1, 2); });
var f5 = y(() => { var p = PI; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var math_exports = {};
__export(math_exports, { add: () => add, PI: () => PI });
var PI = 3.14;
function add(a, b) { return a + b; }
export { math_exports as math };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // f4 should import add.
    let f4_code = &raw_pairs.iter().find(|(n, _)| n == "f4.js").unwrap().1;
    assert!(
        f4_code.contains("import") && f4_code.contains("add"),
        "f4.js should import add:\n{f4_code}"
    );

    // f5 should import PI.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("PI"),
        "f5.js should import PI:\n{f5_code}"
    );

    // The math module should export both add and PI (they were in the original
    // __export map, plus referenced by factories).
    let math_code = &raw_pairs
        .iter()
        .find(|(_, code)| code.contains("3.14"))
        .unwrap()
        .1;
    assert!(
        math_code.contains("export"),
        "math module should have exports:\n{math_code}"
    );
}

/// Factory modules that don't reference any scope-hoisted bindings should not
/// get any spurious import statements.
#[test]
fn factory_without_scope_hoisted_refs_gets_no_imports() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // All factory modules should have no import statements.
    for (name, code) in &raw_pairs {
        if name.starts_with("f") && name.ends_with(".js") {
            assert!(
                !code.contains("import"),
                "{name} should not have import statements (no scope-hoisted refs):\n{code}"
            );
        }
    }
}

/// Factory modules that reference a scope-hoisted namespace object (e.g.
/// `ns_a.greet()`) should import it from entry.js, where the namespace
/// declaration and __export call are restored.  The scope-hoisted module
/// itself should NOT export the undeclared namespace binding.
#[test]
fn factory_referencing_namespace_imports_from_entry() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { var result = ns_a.greet(); });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // f5 should import ns_a from entry.js.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("ns_a"),
        "f5.js should import ns_a:\n{f5_code}"
    );
    assert!(
        f5_code.contains("entry.js"),
        "f5.js should import ns_a from entry.js:\n{f5_code}"
    );

    // The scope-hoisted module should NOT export `ns_a` (it doesn't declare it).
    let ns_a_module = &raw_pairs
        .iter()
        .find(|(_, code)| code.contains("function greet"))
        .unwrap()
        .1;
    assert!(
        !ns_a_module.contains("export { ns_a") && !ns_a_module.contains("export { ns_a,"),
        "scope module should NOT export undeclared ns_a:\n{ns_a_module}"
    );
}

/// When the bundle has no ESM `export { ns_a }` but a factory references the
/// namespace, entry.js must still export it so the factory import resolves.
#[test]
fn factory_namespace_ref_without_entry_export_adds_export() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { var result = ns_a.greet(); });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
var ns_b = {};
__export(ns_b, { value: () => value });
var value = 99;
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // f5 should import ns_a from entry.js.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("ns_a") && f5_code.contains("entry.js"),
        "f5.js should import ns_a from entry.js:\n{f5_code}"
    );

    // entry.js must have an ESM `export { ns_a }` even though the bundle
    // had no such declaration.  Check for the actual export statement, not
    // the `__export` helper function name.
    let entry_code = &raw_pairs.iter().find(|(n, _)| n == "entry.js").unwrap().1;
    assert!(
        entry_code.contains("export { ns_a"),
        "entry.js should have `export {{ ns_a }}` for the factory import:\n{entry_code}"
    );
}

/// When the entry has an aliased export like `export { math_exports as math }`,
/// `math_exports` is NOT directly importable by that name.  A factory that
/// references `math_exports` still needs a synthesized `export { math_exports }`.
#[test]
fn factory_namespace_ref_with_aliased_entry_export_adds_export() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { var result = math_exports.add(1, 2); });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var math_exports = {};
__export(math_exports, { add: () => add });
function add(a, b) { return a + b; }
export { math_exports as math };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // f5 should import math_exports from entry.js.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("math_exports"),
        "f5.js should import math_exports:\n{f5_code}"
    );

    // entry.js must have an unaliased `export { math_exports }` in addition to
    // the existing `export { math_exports as math }`.
    let entry_code = &raw_pairs.iter().find(|(n, _)| n == "entry.js").unwrap().1;
    // Check there's an `export { math_exports }` (unaliased) — not just
    // `export { math_exports as math }`.
    let has_unaliased = entry_code.lines().any(|line| {
        line.contains("export {") && line.contains("math_exports") && !line.contains(" as ")
    });
    assert!(
        has_unaliased,
        "entry.js needs unaliased `export {{ math_exports }}`:\n{entry_code}"
    );
}

/// `export { ns_a as ns_a }` is semantically identical to `export { ns_a }` —
/// both make the binding importable as `ns_a`. No duplicate export should be
/// synthesized.
#[test]
fn same_name_alias_export_does_not_duplicate() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { var result = ns_a.greet(); });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
export { ns_a as ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let entry_code = &raw_pairs.iter().find(|(n, _)| n == "entry.js").unwrap().1;
    // Count how many `export {` lines mention ns_a — should be exactly one.
    let export_ns_a_count = entry_code
        .lines()
        .filter(|line| line.contains("export {") && line.contains("ns_a"))
        .count();
    assert_eq!(
        export_ns_a_count, 1,
        "entry.js should have exactly one export of ns_a (no duplicate):\n{entry_code}"
    );
}

/// Private helpers only referenced by factory modules (not by other scope-
/// hoisted code) should still be absorbed into the last scope-hoisted module,
/// not left in entry.js.
#[test]
fn factory_only_ref_extends_last_scope_module() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { var x = privateHelper(1); });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
function privateHelper(x) { return x + 1; }
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // privateHelper should be in the scope-hoisted module (ns_a), not entry.
    let ns_a_code = &raw_pairs
        .iter()
        .find(|(_, code)| code.contains("function greet"))
        .unwrap()
        .1;
    assert!(
        ns_a_code.contains("privateHelper"),
        "privateHelper should be in the scope-hoisted module, not entry:\n{ns_a_code}"
    );

    // f5 should import privateHelper.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("privateHelper"),
        "f5.js should import privateHelper:\n{f5_code}"
    );

    // entry should NOT contain privateHelper.
    let entry_code = &raw_pairs.iter().find(|(n, _)| n == "entry.js").unwrap().1;
    assert!(
        !entry_code.contains("privateHelper"),
        "entry.js should NOT contain privateHelper:\n{entry_code}"
    );
}

/// Init factory that reads a binding from another scope module must synthesize
/// an import in the target module after merging.
#[test]
fn merged_init_factory_imports_cross_module_reads() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var init_a = y(() => { target = source + 1; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var a_exports = {};
__export(a_exports, { target: () => target });
var target;
var b_exports = {};
__export(b_exports, { source: () => source });
var source = 42;
export { a_exports, b_exports };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let a_code = &raw_pairs
        .iter()
        .find(|(n, _)| n.starts_with("a_exports"))
        .expect("a_exports module should exist")
        .1;
    // The merged init body uses `source` from b_exports — must have an import.
    assert!(
        a_code.contains("import") && a_code.contains("source"),
        "a_exports should import `source` from b_exports:\n{a_code}"
    );
    assert!(
        a_code.contains("target = source + 1"),
        "a_exports should contain the merged assignment:\n{a_code}"
    );
}

/// Update expressions (count++) that write to a scope-hoisted binding should
/// be merged into the target module, not emitted standalone with an import.
#[test]
fn update_expr_write_merges_into_target_module() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var init_counter = y(() => { count++; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var counter_exports = {};
__export(counter_exports, { count: () => count });
var count = 0;
export { counter_exports };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // init_counter should NOT exist as a separate module.
    assert!(
        !raw_pairs.iter().any(|(n, _)| n.contains("init_counter")),
        "init_counter should be merged, not standalone"
    );

    let counter_code = &raw_pairs
        .iter()
        .find(|(n, _)| n.starts_with("counter_exports"))
        .expect("counter_exports module should exist")
        .1;
    assert!(
        counter_code.contains("count++"),
        "counter_exports should contain the merged count++:\n{counter_code}"
    );
}

/// Factory files in subdirectories should get correct relative import paths.
#[test]
fn factory_in_subdirectory_gets_correct_import_path() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y({"src/consumer.js"(exports, module) { var x = helperFn(1); }});
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { greet: () => greet });
function greet() { return "hello"; }
function helperFn(x) { return x + 1; }
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    // The factory at src/consumer.js should import with ../ns_a.js, not ./ns_a.js.
    let consumer_code = &raw_pairs
        .iter()
        .find(|(n, _)| n.contains("consumer"))
        .unwrap()
        .1;
    assert!(
        consumer_code.contains("../"),
        "src/consumer.js should use ../ to reach the root-level scope module:\n{consumer_code}"
    );
    assert!(
        !consumer_code.contains("./../"),
        "should not have double-prefixed ./../ path:\n{consumer_code}"
    );
    assert!(
        consumer_code.contains("helperFn"),
        "src/consumer.js should import helperFn:\n{consumer_code}"
    );
}
