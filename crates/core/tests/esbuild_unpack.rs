use wakaru_core::unpack;
use wakaru_core::unpack_raw;
use wakaru_core::DecompileOptions;

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
    let pairs = unpack(
        &bundle,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

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
    let pairs = unpack(
        &bundle,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

    assert!(pairs.len() >= 6, "expected ≥6 modules, got {}", pairs.len());
    assert!(
        pairs.iter().any(|(n, _)| n == "entry.js"),
        "missing entry.js"
    );
}

#[test]
fn esbuild_factory_code_is_non_empty() {
    let bundle = make_bundle("(q,K)=>()=>(q&&(K=q(q=0)),K)", "y");
    let pairs = unpack(
        &bundle,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");
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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");

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
    let raw_pairs =
        unpack_raw(bundle, &DecompileOptions::default()).expect("unpack_raw should succeed");
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
fn small_file_with_few_factories_is_not_detected() {
    // Only 2 factories — below the threshold of 5, so we should NOT detect as esbuild.
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var a = y(() => { x = 1; });
var b = y(() => { z = 2; });
a(); b();
"#;
    let pairs = unpack(
        bundle,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

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
    let pairs = unpack(
        code,
        DecompileOptions {
            filename: "app.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

    assert_eq!(
        pairs.len(),
        1,
        "single boundary without ESM export should not split: {:?}",
        pairs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
    );
}
