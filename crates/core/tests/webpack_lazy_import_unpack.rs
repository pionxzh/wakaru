//! webpack and Rspack lazy loads of another module of the bundle become
//! `import()` once unpack resolves their module id
//! (`webpack_lazy_import_source_semantics`).

use wakaru_core::driver::test_support::{unpack, unpack_files, UnpackInput};
use wakaru_core::{DecompileOptions, UnpackWarningKind};

/// A webpack 5 bundle whose entry runs module 1, with `modules` as the
/// module array from index 1.
fn bundle(modules: &str) -> String {
    format!(
        r#"
(() => {{
    var e = [, {modules}];
    var n = {{}};
    function r(o) {{
        var t = n[o];
        if (t !== undefined) return t.exports;
        var c = n[o] = {{ exports: {{}} }};
        e[o](c, c.exports, r);
        return c.exports;
    }}
    r.d = (e, t) => {{ for (var n in t) r.o(t, n) && !r.o(e, n) && Object.defineProperty(e, n, {{ enumerable: !0, get: t[n] }}); }};
    r.o = (e, t) => Object.prototype.hasOwnProperty.call(e, t);
    r.r = (e) => {{ Object.defineProperty(e, "__esModule", {{ value: !0 }}); }};
    r.t = function (e, t) {{ return e; }};
    r.e = (e) => Promise.resolve(e);
    var d = r(1);
    console.log(d);
}})();
"#
    )
}

/// The unpacked modules of `source`, and the module each `runtime_residual`
/// warning names.
fn unpacked_with_residuals(source: &str) -> (Vec<(String, String)>, Vec<String>) {
    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    let residuals = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::RuntimeResidual)
        .map(|warning| format!("{}: {}", warning.filename, warning.message))
        .collect();
    (output.modules, residuals)
}

fn unpacked(source: &str) -> Vec<(String, String)> {
    let (modules, residuals) = unpacked_with_residuals(source);
    assert!(residuals.is_empty(), "{residuals:?}");
    modules
}

fn module<'a>(modules: &'a [(String, String)], name: &str) -> &'a str {
    modules
        .iter()
        .find(|(module, _)| module == name)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| panic!("missing {name}: {modules:?}"))
}

// producer webpack@5.111.1: a CommonJS provider in the same chunk.
const COMMONJS_PROVIDER: &str = r#"(e) => { e.exports = { version: "3" }; }"#;
// producer webpack@5.111.1: an ES module provider.
const ESM_PROVIDER: &str = r#"(e, t, r) => { r.r(t); r.d(t, { label: () => o }); function o(v) { return "label:" + v; } }"#;

#[test]
fn fake_namespace_of_a_commonjs_value_becomes_import() {
    for loader in [
        "() => r.t(2, 19)",
        "r.t.bind(r, 2, 23)",
        // webpack 4 binds `null` and uses mode 7.
        "r.t.bind(null, 2, 7)",
    ] {
        let modules = unpacked(&bundle(&format!(
            r#"(e, t, r) => {{ r.r(t); r.d(t, {{ load: () => o }}); async function o() {{ const m = await Promise.resolve().then({loader}); return m.default.version; }} }}, {COMMONJS_PROVIDER}"#
        )));
        let consumer = module(&modules, "module-1.js");
        assert!(
            consumer.contains(r#"await import("./module-2.js");"#)
                && consumer.contains("export async function load")
                && !consumer.contains("require"),
            "`{loader}`:\n{consumer}"
        );
    }
}

#[test]
fn plain_loader_of_an_es_module_becomes_import() {
    for loader in ["() => r(2)", "r.bind(r, 2)"] {
        let modules = unpacked(&bundle(&format!(
            r#"(e, t, r) => {{ r.r(t); r.d(t, {{ load: () => o }}); async function o() {{ const {{ label }} = await Promise.resolve().then({loader}); return label("x"); }} }}, {ESM_PROVIDER}"#
        )));
        let consumer = module(&modules, "module-1.js");
        assert!(
            consumer.contains(r#"await import("./module-2.js");"#)
                && consumer.contains("export async function load")
                && !consumer.contains("require"),
            "`{loader}`:\n{consumer}"
        );
    }
}

// hypothetical: webpack emits the plain loader only for a provider it reads
// as a namespace; the plain loader of a `module.exports = v` provider pins
// the default form.
#[test]
fn plain_loader_of_a_commonjs_value_reads_its_default() {
    let modules = unpacked(&bundle(&format!(
        r#"(e, t, r) => {{ r.r(t); r.d(t, {{ load: () => o }}); async function o() {{ const m = await Promise.resolve().then(() => r(2)); return m.version; }} }}, {COMMONJS_PROVIDER}"#
    )));
    let consumer = module(&modules, "module-1.js");
    assert!(
        consumer.contains(r#"import("./module-2.js").then((ns)=>ns.default)"#)
            && !consumer.contains("require"),
        "{consumer}"
    );
}

#[test]
fn fake_namespace_of_named_exports_only_keeps_the_loader() {
    // The fake namespace has a `default`, the recovered module none.
    let (modules, residuals) = unpacked_with_residuals(&bundle(
        r#"(e, t, r) => { r.r(t); r.d(t, { load: () => o }); async function o() { const m = await Promise.resolve().then(() => r.t(2, 19)); return m.greet("x"); } }, (e, t) => { t.greet = function (v) { return "hi " + v; }; }"#,
    ));
    let consumer = module(&modules, "module-1.js");
    assert!(
        consumer.contains(r#"require.t("./module-2.js", 19)"#) && !consumer.contains("import("),
        "{consumer}"
    );
    // The kept loader is reported.
    assert_eq!(
        residuals,
        ["module-1.js: webpack runtime member `require.t` is kept, which the split output does not define; the lazy load throws when it runs"]
    );
}

#[test]
fn chunk_load_without_its_chunk_is_reported() {
    let (modules, residuals) = unpacked_with_residuals(&bundle(
        r#"(e, t, r) => { r.r(t); r.d(t, { load: () => o }); async function o() { return r.e(529).then(() => r(529)); } }"#,
    ));
    let consumer = module(&modules, "module-1.js");
    assert!(consumer.contains("require.e(529)"), "{consumer}");
    assert_eq!(
        residuals,
        ["module-1.js: webpack runtime member `require.e` is kept, which the split output does not define; the lazy load throws when it runs"]
    );
}

#[test]
fn chunk_load_in_another_input_is_dropped() {
    // producer webpack@5.111.1 with the provider in its own async chunk.
    let entry = bundle(
        r#"(e, t, r) => { r.r(t); r.d(t, { load: () => o }); async function o() { const { label } = await r.e(529).then(() => r(529)); return label("x"); } }"#,
    );
    let chunk = format!(
        r#"
exports.id = 529;
exports.ids = [529];
exports.modules = {{
  529: {ESM_PROVIDER}
}};
"#
    );
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "bundle.js".to_string(),
                source: entry,
            },
            UnpackInput {
                filename: "529.js".to_string(),
                source: chunk,
            },
        ],
        DecompileOptions::default(),
    )
    .expect("entry and chunk should unpack together");
    let consumer = module(&output.modules, "module-1.js");
    assert!(
        consumer.contains(r#"await import("./module-529.js");"#)
            && consumer.contains("export async function load")
            && !consumer.contains("require"),
        "{consumer}"
    );
}
