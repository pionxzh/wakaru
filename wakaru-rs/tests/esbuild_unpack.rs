use wakaru_rs::unpack;
use wakaru_rs::DecompileOptions;

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
    assert!(pairs.len() >= 6, "expected ≥6 modules (5 factories + entry), got {}", pairs.len());

    let has_entry = pairs.iter().any(|(name, _)| name == "entry.js");
    assert!(has_entry, "entry.js not found: {:?}", pairs.iter().map(|(n, _)| n).collect::<Vec<_>>());

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
    assert!(pairs.iter().any(|(n, _)| n == "entry.js"), "missing entry.js");
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
    assert_eq!(pairs.len(), 1, "should not detect as esbuild bundle with only 2 factories");
}
