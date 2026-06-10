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

fn expect_heuristic_unpack(source: &str, filename: &str) -> Vec<(String, String)> {
    let output = unpack(
        source,
        DecompileOptions {
            filename: filename.to_string(),
            heuristic_split: true,
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

fn expect_heuristic_unpack_raw(source: &str) -> Vec<(String, String)> {
    let output = unpack_raw(
        source,
        &DecompileOptions {
            heuristic_split: true,
            ..Default::default()
        },
    )
    .expect("unpack_raw should succeed");
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
fn commonjs_factory_emits_callable_module_exports_wrapper() {
    let bundle = r#"
var m = (q, K) => () => (K || q((K = { exports: {} }).exports, K), K.exports);
var f1 = m((exports, module) => { module.exports = function() { return 1; }; });
var f2 = m((exports, module) => { module.exports = 2; });
var f3 = m((exports) => { exports.value = 3; });
var f4 = m((exports, module) => { module.exports = 4; });
var f5 = m((exports, module) => { module.exports = 5; });
console.log(f1()());
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let f1_code = &raw_pairs.iter().find(|(n, _)| n == "f1.js").unwrap().1;
    assert!(
        f1_code.contains("export function f1()"),
        "CommonJS factory should emit a callable export:\n{f1_code}"
    );
    assert!(
        f1_code.contains("var __wakaru_f1_cache")
            && f1_code.contains("if (__wakaru_f1_cache)")
            && f1_code.contains("return __wakaru_f1_cache.exports")
            && f1_code.contains("var exports = {}")
            && f1_code.contains("var module =")
            && f1_code.contains("exports: exports")
            && f1_code.contains("__wakaru_f1_cache = module")
            && f1_code.contains("return module.exports"),
        "CommonJS factory should cache module state before running the body:\n{f1_code}"
    );

    let f3_code = &raw_pairs.iter().find(|(n, _)| n == "f3.js").unwrap().1;
    assert!(
        f3_code.contains("export function f3()")
            && f3_code.contains("var __wakaru_f3_cache")
            && f3_code.contains("if (__wakaru_f3_cache)")
            && f3_code.contains("return __wakaru_f3_cache")
            && f3_code.contains("var exports = {}")
            && f3_code.contains("__wakaru_f3_cache = exports")
            && f3_code.contains("exports.value = 3")
            && f3_code.contains("return exports"),
        "one-param CommonJS factory should cache and return the exports object:\n{f3_code}"
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

#[test]
fn heuristic_scope_hoist_restores_esbuild_dynamic_require_imports() {
    let bundle = r#"
(()=>{
var H=Object.create;
var P=Object.defineProperty;
var q=Object.getOwnPropertyDescriptor;
var G=Object.getOwnPropertyNames;
var Q=Object.getPrototypeOf,W=Object.prototype.hasOwnProperty;
var r=(e=>typeof require<"u"?require:typeof Proxy<"u"?new Proxy(e,{get:(t,o)=>(typeof require<"u"?require:t)[o]}):e)(function(e){if(typeof require<"u")return require.apply(this,arguments);throw Error('Dynamic require of "'+e+'" is not supported')});
var X=(e,t,o,a)=>{if(t&&typeof t=="object"||typeof t=="function")for(let l of G(t))!W.call(e,l)&&l!==o&&P(e,l,{get:()=>t[l],enumerable:!(a=q(t,l))||a.enumerable});return e};
var Y=(e,t,o)=>(o=e!=null?H(Q(e)):{},X(t||!e||!e.__esModule?P(o,"default",{value:e,enumerable:!0}):o,e));
var Z=Math.max;
var react=r("react"),jsx=r("react/jsx-runtime"),D=(0,react.createContext)(null);
var wrapped=Y(r("react"));
function App(){return (0,jsx.jsx)(wrapped.default.Fragment,{children:(0,jsx.jsx)(D.Provider,{value:null,children:"ok"})})}
var Root=App;
console.log(Root);
})();
"#;

    let raw_pairs = expect_heuristic_unpack_raw(bundle);
    let raw_app = raw_pairs
        .iter()
        .find(|(_, code)| code.contains("createContext"))
        .expect("should split app chunk");
    assert!(
        !raw_app.1.contains("import { r }"),
        "dynamic require helper should not be synthesized as an import:\n{}",
        raw_app.1
    );
    assert!(
        (raw_app.1.contains("require(\"react\")")
            && raw_app.1.contains("require(\"react/jsx-runtime\")"))
            || (raw_app.1.contains("from \"react\"")
                && raw_app.1.contains("from \"react/jsx-runtime\"")),
        "dynamic require helper calls should be restored to direct module references:\n{}",
        raw_app.1
    );
    assert!(
        !raw_app.1.contains("import { Y }") && !raw_app.1.contains("Y(require("),
        "esbuild __toESM helper should not be synthesized as an import:\n{}",
        raw_app.1
    );
    assert!(
        raw_app.1.contains("wrapped.Fragment") && !raw_app.1.contains("wrapped.default"),
        "default interop member access should be unwrapped with the helper call:\n{}",
        raw_app.1
    );

    let pairs = expect_heuristic_unpack(bundle, "bundle.js");
    let app = pairs
        .iter()
        .find(|(_, code)| code.contains("createContext"))
        .expect("should decompile app chunk");
    assert!(
        app.1.contains("from \"react\"") && app.1.contains("from \"react/jsx-runtime\""),
        "require() calls should be restored to imports after UnEsm:\n{}",
        app.1
    );
    assert!(
        !app.1.contains("r(\"react\")"),
        "dynamic require alias should not survive decompilation:\n{}",
        app.1
    );
    assert!(
        !app.1.contains("Y(require(") && !app.1.contains(".default.Fragment"),
        "esbuild __toESM wrapper should not survive decompilation:\n{}",
        app.1
    );
}

#[test]
fn scope_module_imports_bindings_referenced_only_by_export_getters() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { source: () => source });
var source = 42;
var ns_b = {};
__export(ns_b, { reexported: () => source });
function local() { return "local"; }
var ns_c = {};
__export(ns_c, { tail: () => tail });
var tail = "tail";
export { ns_a, ns_b, ns_c };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        ns_a_code.contains("export") && ns_a_code.contains("source"),
        "ns_a.js should contain and export source:\n{ns_a_code}"
    );

    let ns_b_code = &raw_pairs.iter().find(|(n, _)| n == "ns_b.js").unwrap().1;
    assert!(
        ns_b_code.contains("import { source }") && ns_b_code.contains("./ns_a.js"),
        "ns_b.js should import source because its export getter references it:\n{ns_b_code}"
    );
    assert!(
        ns_b_code.contains("export { source"),
        "ns_b.js should re-export the imported source binding without dangling exports:\n{ns_b_code}"
    );
}

#[test]
fn scope_module_exports_namespace_object_when_referenced_by_another_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { renamed: () => value });
function value() { return 1; }
var ns_b = {};
__export(ns_b, { read: () => read });
function read() { return ns_a.renamed(); }
export { ns_a, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        ns_a_code.contains("export var ns_a = {}")
            && ns_a_code.contains("\"renamed\"")
            && ns_a_code.contains("get: () => value"),
        "provider module should export its namespace object with getters:\n{ns_a_code}"
    );

    let ns_b_code = &raw_pairs.iter().find(|(n, _)| n == "ns_b.js").unwrap().1;
    assert!(
        ns_b_code.contains("import { ns_a }") && ns_b_code.contains("./ns_a.js"),
        "consumer module should import the namespace from the provider module, not entry:\n{ns_b_code}"
    );
}

#[test]
fn restored_entry_namespace_does_not_restore_late_export_helper() {
    let bundle = r#"
var ns_a = {};
T8(ns_a, { value: () => value });
function value() { return 1; }
var { defineProperty } = Object;
function KK5(target, name, getter) {
    defineProperty(target, name, { enumerable: true, get: getter });
}
var T8 = (target, all) => {
    for (var name in all)
        KK5(target, name, all[name]);
};
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let entry_code = &raw_pairs.iter().find(|(n, _)| n == "entry.js").unwrap().1;
    assert!(
        entry_code.contains("Object.defineProperty(ns_a")
            && entry_code.contains("\"value\"")
            && entry_code.contains("get: ()=>value"),
        "entry namespace should be restored with direct getters:\n{entry_code}"
    );
    assert!(
        !entry_code.contains("T8(ns_a")
            && !entry_code.contains("var T8")
            && !entry_code.contains("function KK5")
            && !entry_code.contains("defineProperty } = Object"),
        "entry should not keep the late export helper prelude:\n{entry_code}"
    );
}

#[test]
fn scope_module_imports_external_bindings_used_in_body() {
    let bundle = r#"
import { randomUUID as uid } from "crypto";
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { make: () => make });
function make() { return uid(); }
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    assert!(
        ns_a_code.contains("import { randomUUID as uid } from \"crypto\""),
        "scope module should preserve external imports referenced by its body:\n{ns_a_code}"
    );
    assert!(
        ns_a_code.contains("return uid()"),
        "scope module should keep the imported binding reference:\n{ns_a_code}"
    );
}

#[test]
fn scope_module_does_not_duplicate_external_import_already_in_body() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { make: () => make });
import { randomUUID as uid } from "crypto";
function make() { return uid(); }
var ns_b = {};
__export(ns_b, { value: () => value });
var value = 1;
export { ns_a, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs.iter().find(|(n, _)| n == "ns_a.js").unwrap().1;
    let import_count = ns_a_code
        .matches("import { randomUUID as uid } from \"crypto\"")
        .count();
    assert_eq!(
        import_count, 1,
        "scope module should not duplicate package imports already present in its body:\n{ns_a_code}"
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
/// `ns_a.greet()`) should import it from the scope module that owns the
/// namespace getters, avoiding an entry.js cycle.
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

    // f5 should import ns_a from the restored namespace module.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("ns_a"),
        "f5.js should import ns_a:\n{f5_code}"
    );
    assert!(
        f5_code.contains("./ns_a.js"),
        "f5.js should import ns_a from the namespace module:\n{f5_code}"
    );

    // The scope-hoisted module should export the synthesized namespace object.
    let ns_a_module = &raw_pairs
        .iter()
        .find(|(_, code)| code.contains("function greet"))
        .unwrap()
        .1;
    assert!(
        ns_a_module.contains("export var ns_a") && ns_a_module.contains("Object.defineProperty"),
        "scope module should export synthesized ns_a namespace:\n{ns_a_module}"
    );
}

/// When the bundle has no ESM `export { ns_a }` but a factory references the
/// namespace, the owning scope module must still export it so the factory
/// import resolves.
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

    // f5 should import ns_a from the restored namespace module.
    let f5_code = &raw_pairs.iter().find(|(n, _)| n == "f5.js").unwrap().1;
    assert!(
        f5_code.contains("import") && f5_code.contains("ns_a") && f5_code.contains("./ns_a.js"),
        "f5.js should import ns_a from the namespace module:\n{f5_code}"
    );

    // The scope module must synthesize an ESM namespace export even though
    // the bundle had no direct `export { ns_a }` declaration.
    let ns_a_module = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns_a.js")
        .expect("ns_a module should exist")
        .1;
    assert!(
        ns_a_module.contains("export var ns_a") && ns_a_module.contains("Object.defineProperty"),
        "ns_a.js should synthesize a namespace export for the factory import:\n{ns_a_module}"
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
/// an import in the standalone lazy factory module.
#[test]
fn standalone_init_factory_imports_cross_module_reads() {
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

    let init_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_a.js")
        .expect("init_a module should exist")
        .1;
    // The init body uses `source` from b_exports — must have an import.
    assert!(
        init_code.contains("import { source } from \"./b_exports.js\""),
        "init_a should import `source` from b_exports:\n{init_code}"
    );
    assert!(
        init_code.contains("export function init_a")
            && init_code.contains("target = source + 1")
            && init_code.contains("__wakaru_init_a_initialized")
            && init_code.contains("export { target"),
        "init_a should contain the guarded lazy body and written export:\n{init_code}"
    );
}

/// Split init factories remain callable guarded functions so other modules can
/// trigger the original lazy initializer without running the body at ESM
/// evaluation time.
#[test]
fn standalone_init_factory_preserves_callable_symbol() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var init_target = y(() => { target = 1; });
var init_other = y(() => { init_target(); other = target + 1; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var mod_exports = {};
__export(mod_exports, { target: () => target, other: () => other });
var target, other;
export { mod_exports };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let target_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_target.js")
        .expect("init_target module should exist")
        .1;
    assert!(
        target_code.contains("export function init_target"),
        "init_target should remain callable/exported:\n{target_code}"
    );
    assert!(
        target_code.contains("__wakaru_init_target_initialized")
            && target_code.contains("target = 1")
            && target_code.contains("export { target"),
        "init_target should remain guarded and export written state:\n{target_code}"
    );

    let other_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_other.js")
        .expect("init_other module should exist")
        .1;
    assert!(
        other_code.contains("import { init_target, target } from \"./init_target.js\"")
            && other_code.contains("init_target()")
            && other_code.contains("other = target + 1"),
        "init_other should import and call init_target:\n{other_code}"
    );
}

#[test]
fn standalone_init_factory_preserves_lazy_body() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var init_lazy = y(() => { value = compute(); });
function compute() { return 1; }
init_lazy();
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let lazy_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_lazy.js")
        .expect("init_lazy module should exist")
        .1;
    assert!(
        lazy_code.contains("var __wakaru_init_lazy_initialized = false")
            && lazy_code.contains("export function init_lazy")
            && lazy_code.contains("value = compute()"),
        "standalone init factory should preserve a guarded lazy body:\n{lazy_code}"
    );
}

/// Update expressions (count++) that write to a scope-hoisted binding should
/// stay in a standalone lazy factory and export the state they mutate.
#[test]
fn update_expr_write_stays_in_standalone_factory() {
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

    let counter_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_counter.js")
        .expect("init_counter module should exist")
        .1;
    assert!(
        counter_code.contains("count++")
            && counter_code.contains("var count = 0")
            && counter_code.contains("export { count }")
            && counter_code.contains("export function init_counter"),
        "init_counter should contain the guarded count++ and exported state:\n{counter_code}"
    );
}

/// Standalone lazy factories own the top-level state they initialize.  If the
/// state stays in entry.js, the emitted factory body assigns an undeclared ESM
/// binding and Node throws before the bundle can run.
#[test]
fn standalone_factory_exports_written_state_for_later_factories() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var a, b;
var init_a = y(() => { a = 1; });
var init_b = y(() => { init_a(); b = a + 1; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_b();
console.log(b);
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let init_a_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_a.js")
        .expect("init_a module should exist")
        .1;
    assert!(
        init_a_code.contains("var a") && init_a_code.contains("export { a"),
        "init_a should export its written state:\n{init_a_code}"
    );

    let init_b_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_b.js")
        .expect("init_b module should exist")
        .1;
    assert!(
        init_b_code.contains("import { a, init_a }") && init_b_code.contains("./init_a.js"),
        "init_b should import init_a and state a from init_a:\n{init_b_code}"
    );
    assert!(
        init_b_code.contains("var b") && init_b_code.contains("export { b"),
        "init_b should export its written state:\n{init_b_code}"
    );
}

#[test]
fn standalone_init_factory_decompiles_storage_before_callable_wrapper() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var a;
var init_a = y(() => { a = 1; });
var init_b = y(() => { init_a(); });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_b();
console.log(a);
"#;
    let pairs = expect_unpack(bundle, "bundle.js");

    let init_a_code = &pairs
        .iter()
        .find(|(n, _)| n == "init_a.js")
        .expect("init_a module should exist")
        .1;
    let storage_pos = init_a_code
        .find("let a;")
        .or_else(|| init_a_code.find("var a;"))
        .expect("init_a should declare its written state");
    let wrapper_pos = init_a_code
        .find("export function init_a")
        .expect("init_a should export its callable wrapper");
    assert!(
        storage_pos < wrapper_pos,
        "written state must be declared before the callable init wrapper:\n{init_a_code}"
    );
}

#[test]
fn scope_module_imports_factory_owned_binding_from_mixed_declaration() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
function memoize(fn) { return function() { return fn.apply(this, arguments); }; }
var label = "ok", memo;
var init_memo = y(() => { memo = memoize; });
var init_user = y(() => { init_memo(); cached = useMemo; });
var cached;
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns = {};
__export(ns, { useMemo: () => useMemo });
function useMemo(fn) { return memo(fn); }
var ns_b = {};
__export(ns_b, { other: () => other });
function other() { return label; }
init_user();
console.log(cached(() => label)());
export { ns, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns.js")
        .expect("scope module should exist")
        .1;
    assert!(
        ns_code.contains("import { memo } from \"./init_memo.js\""),
        "scope module should import factory-owned memo binding:\n{ns_code}"
    );
    assert!(
        !ns_code.contains("memo;") && !ns_code.contains("memo,"),
        "scope module must not redeclare factory-owned memo from mixed var decl:\n{ns_code}"
    );
}

#[test]
fn scope_module_imports_callable_commonjs_factory_by_atom() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var m = (q, K) => () => (K || q((K = { exports: {} }).exports, K), K.exports);
var runtime = m((exports, module) => { module.exports = function() { return 1; }; });
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns = {};
__export(ns, { read: () => read });
function read() { return runtime()(); }
var ns_b = {};
__export(ns_b, { other: () => other });
function other() { return 2; }
console.log(ns.read());
export { ns, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns.js")
        .expect("scope module should exist")
        .1;
    assert!(
        ns_code.contains("import { runtime } from \"./runtime.js\""),
        "scope module should import callable CommonJS factory wrapper:\n{ns_code}"
    );
}

#[test]
fn scope_module_imports_factory_owned_support_binding() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
function dispose(value) { return value; }
var reader;
var init_reader = y(() => {
    reader = function(value) { return dispose(value); };
});
var f1 = y(() => { v1 = 1; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns = {};
__export(ns, { read: () => read });
function read(value) {
    init_reader();
    return dispose(reader(value));
}
var ns_b = {};
__export(ns_b, { other: () => other });
function other() { return 2; }
console.log(ns.read(1));
export { ns, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns.js")
        .expect("scope module should exist")
        .1;
    assert!(
        ns_code.contains("import { dispose, init_reader, reader } from \"./init_reader.js\""),
        "scope module should import support binding owned by init factory:\n{ns_code}"
    );
}

#[test]
fn scope_module_owns_local_support_function_refs() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
function cache(fn) { return fn; }
function readSlot(def) { return def.slot; }
var ns_b = {};
__export(ns_b, { b: () => b, value: () => value });
var value, b;
value = cache(() => 1);
b = readSlot({ get slot() { return value(); } });
export { ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_b_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns_b.js")
        .expect("scope module should exist")
        .1;
    assert!(
        ns_b_code.contains("function cache") && ns_b_code.contains("function readSlot"),
        "scope module should own local support functions it calls:\n{ns_b_code}"
    );
    let entry_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "entry.js")
        .expect("entry module should exist")
        .1;
    assert!(
        !entry_code.contains("function cache") && !entry_code.contains("function readSlot"),
        "owned support functions should not remain in entry.js:\n{entry_code}"
    );
}

#[test]
fn scope_module_owns_sibling_helper_from_mixed_lazy_declaration() {
    let bundle = r#"
var wrap = (value) => ({ default: value }), y = (q,K) => () => (q && (K = q(q = 0)), K);
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
var wrapped_exports = {};
__export(wrapped_exports, { wrapped: () => wrapped });
var wrapped = wrap("value");
export { wrapped_exports };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let wrapped_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "wrapped_exports.js")
        .expect("scope module should exist")
        .1;
    assert!(
        wrapped_code.contains("wrap =")
            && wrapped_code.contains("wrapped = wrap")
            && !wrapped_code.contains("y ="),
        "scope module should keep the sibling helper without re-emitting the lazy helper:\n{wrapped_code}"
    );
}

#[test]
fn unowned_mixed_helper_siblings_do_not_leak_to_entry() {
    let bundle = r#"
var wrap = (value) => ({ default: value }), unused = (value) => value, y = (q,K) => () => (q && (K = q(q = 0)), K);
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
var wrapped_exports = {};
__export(wrapped_exports, { wrapped: () => wrapped });
var wrapped = wrap("value");
console.log(wrapped);
export { wrapped_exports };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let wrapped_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "wrapped_exports.js")
        .expect("scope module should exist")
        .1;
    assert!(
        wrapped_code.contains("wrap =") && wrapped_code.contains("wrapped = wrap"),
        "owned mixed-declaration sibling should still move with the scope module:\n{wrapped_code}"
    );

    let entry_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "entry.js")
        .expect("entry module should exist")
        .1;
    assert!(
        !entry_code.contains("unused ="),
        "unowned helper siblings from mixed helper declarations should not leak into entry.js:\n{entry_code}"
    );
}

#[test]
fn exported_noop_support_binding_stays_with_scope_module() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var noop = () => {};
var ns_a = {};
__export(ns_a, { run: () => run, noop: () => noop });
function run() { return "a"; }
var ns_b = {};
__export(ns_b, { value: () => value });
noop();
var value = run();
export { ns_a, ns_b };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns_a.js")
        .expect("ns_a module should exist")
        .1;
    assert!(
        ns_a_code.contains("function noop") || ns_a_code.contains("noop ="),
        "exported no-op support binding should be available in ns_a.js:\n{ns_a_code}"
    );

    let ns_b_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns_b.js")
        .expect("ns_b module should exist")
        .1;
    assert!(
        !ns_b_code.contains("noop();") || ns_b_code.contains("import { noop"),
        "a module that still calls noop should import it instead of referencing an invisible entry binding:\n{ns_b_code}"
    );
}

#[test]
fn scope_module_imports_top_level_computed_property_refs() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var keys = {};
__export(keys, { keyFor: () => keyFor });
function keyFor(name) { return "prefix:" + name; }
var prices = {};
__export(prices, { table: () => table });
var table = { [keyFor("sonnet")]: 1 };
export { keys, prices };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let prices_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "prices.js")
        .expect("prices module should exist")
        .1;
    assert!(
        prices_code.contains("import { keyFor } from \"./keys.js\"")
            && prices_code.contains("[keyFor(\"sonnet\")]"),
        "top-level computed property references should import their owner:\n{prices_code}"
    );
}

#[test]
fn scope_module_imports_one_param_commonjs_factory() {
    let bundle = r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var __commonJS = (cb, mod) => function __require() {
    return mod || (0, cb[Object.keys(cb)[0]])((mod = { exports: {} }).exports), mod.exports;
};
var require_value = __commonJS({
    "value.js"(exports) { exports.value = "ok"; }
});
var ns_a = {};
__export(ns_a, { value: () => value });
var value = require_value().value;
export { ns_a };
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let ns_a_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "ns_a.js")
        .expect("scope module should exist")
        .1;
    assert!(
        ns_a_code.contains("import { require_value } from \"./value.js\""),
        "scope module should import one-param CommonJS factory wrapper:\n{ns_a_code}"
    );

    let value_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "value.js")
        .expect("CommonJS module should exist")
        .1;
    assert!(
        value_code.contains("export function require_value()")
            && value_code.contains("var exports = {}")
            && value_code.contains("return exports"),
        "one-param CommonJS factory should remain callable:\n{value_code}"
    );
}

#[test]
fn standalone_factory_exports_destructuring_assignment_writes() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var source = { value: 1 };
var value;
var init_value = y(() => { ({ value } = source); });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_value();
console.log(value);
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let init_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_value.js")
        .expect("init_value module should exist")
        .1;
    assert!(
        init_code.contains("export var value") || init_code.contains("value }"),
        "init_value should export destructuring assignment writes:\n{init_code}"
    );
}

/// Copied support declarations can have their own top-level dependencies. The
/// ownership pass must close over those references so copied functions don't
/// read undeclared constants at runtime.
#[test]
fn standalone_factory_closes_over_support_declaration_refs() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var FLAG = "ok";
function helper() { return FLAG; }
var value;
var init_value = y(() => { value = helper; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_value();
console.log(value());
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let init_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_value.js")
        .expect("init_value module should exist")
        .1;
    assert!(
        init_code.contains("function helper") && init_code.contains("FLAG = \"ok\""),
        "init_value should include helper and its referenced constant:\n{init_code}"
    );
}

#[test]
fn standalone_factory_keeps_support_declaration_siblings() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var cache_a, cache_b, helper = (value) => {
    var cache = value ? cache_a ??= new WeakMap : cache_b ??= new WeakMap;
    return cache.get(value);
};
var value;
var init_value = y(() => { value = helper({}); });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_value();
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let init_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_value.js")
        .expect("init_value module should exist")
        .1;
    assert!(
        init_code.contains("cache_a") && init_code.contains("cache_b"),
        "init_value should keep sibling cache declarators closed over by helper:\n{init_code}"
    );
}

#[test]
fn standalone_factory_keeps_owned_destructuring_declarations() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
var { defineProperty: defProp, getOwnPropertyNames: names } = Object;
function helper(target) { return names(target).map((name) => defProp({}, name, { value: true })); }
var value;
var init_value = y(() => { value = helper; });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_value();
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let init_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_value.js")
        .expect("init_value module should exist")
        .1;
    assert!(
        init_code.contains("defineProperty") && init_code.contains("getOwnPropertyNames"),
        "init_value should keep the destructuring declaration used by helper:\n{init_code}"
    );
    assert!(
        !init_code.contains("Export '"),
        "output should not contain a dangling export diagnostic:\n{init_code}"
    );
}

#[test]
fn arrow_returning_function_without_factories_is_not_helper() {
    let bundle = r#"
var y = (q,K) => () => (q && (K = q(q = 0)), K);
function encode(q) { return q; }
var cache, template = (q = encode) => function(strings, ...values) { return strings[0]; }, tag;
var init_tag = y(() => { cache = Object.create(null); tag = template(encode); });
var f2 = y(() => { v2 = 2; });
var f3 = y(() => { v3 = 3; });
var f4 = y(() => { v4 = 4; });
var f5 = y(() => { v5 = 5; });
init_tag();
"#;
    let raw_pairs = expect_unpack_raw(bundle);

    let init_code = &raw_pairs
        .iter()
        .find(|(n, _)| n == "init_tag.js")
        .expect("init_tag module should exist")
        .1;
    assert!(
        init_code.contains("template") && init_code.contains("strings"),
        "template helper should stay with the init module instead of being filtered as a lazy helper:\n{init_code}"
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

#[test]
fn plain_code_not_detected_as_esbuild() {
    // No esbuild markers (no lazy-helper arrows, no __export helper).
    // The detector should reject this via the cheap pre-check without
    // cloning or resolving the AST.
    let code = r#"
function greet(name) { return "Hello, " + name + "!"; }
var result = greet("world");
console.log(result);
"#;
    let pairs = expect_unpack(code, "app.js");
    assert_eq!(
        pairs.len(),
        1,
        "plain code should not be detected as esbuild: {:?}",
        pairs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn webpack_style_bundle_not_detected_as_esbuild() {
    // A webpack-shaped IIFE with __webpack_require__ — no esbuild markers.
    // The esbuild detector's pre-check should reject this immediately.
    let code = r#"
(function(modules) {
    function __webpack_require__(moduleId) {
        var module = { exports: {} };
        modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
        return module.exports;
    }
    __webpack_require__(0);
})([
    function(module, exports, __webpack_require__) {
        var dep = __webpack_require__(1);
        console.log(dep.value);
    },
    function(module, exports) {
        exports.value = 42;
    }
]);
"#;
    let pairs = expect_unpack(code, "bundle.js");
    // Should be detected as webpack4, not esbuild.
    assert!(
        pairs.len() >= 2,
        "webpack4 bundle should be split by webpack4 detector, not esbuild"
    );
}
