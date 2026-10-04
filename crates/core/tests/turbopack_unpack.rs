use wakaru_core::driver::test_support::{
    unpack, unpack_files, unpack_raw, UnpackInput, UnpackOutput,
};
use wakaru_core::{BundleFormat, DecompileOptions, UnpackWarningKind};

fn unpack_chunk(source: &str) -> UnpackOutput {
    unpack(
        source,
        DecompileOptions {
            filename: "chunk.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed")
}

fn module<'a>(output: &'a UnpackOutput, filename: &str) -> &'a str {
    output
        .modules
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| panic!("{filename} missing from {:?}", output.modules))
}

fn assert_clean(output: &UnpackOutput) {
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    assert!(
        output.warnings.is_empty(),
        "unexpected warnings: {:?}",
        output.warnings
    );
}

const CLIENT_PREFIX: &str = r#"(globalThis.TURBOPACK || (globalThis.TURBOPACK = [])).push(["object" == typeof document ? document.currentScript : void 0,"#;

fn client_chunk(payload: &str) -> String {
    format!("{CLIENT_PREFIX}{payload}]);")
}

#[test]
fn value_and_getter_exports_become_esm_exports() {
    // Next 16 encoding: `name, 0, value` binds a value; `name, getter` binds
    // a live getter.
    let source = client_chunk(
        r#"
101, t => {
  "use strict";
  var e = t.i(202);
  t.s(["default", 0, function () { return e.helper(1); }, "count", () => n], 101);
  let n = 2;
},
202, t => {
  "use strict";
  t.s(["helper", () => r]);
  function r(v) { return v + 1; }
}
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);

    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"import { helper } from "./module-202.js";
export default function() {
    return helper(1);
};
export let count = 2;"#
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"export function helper(v) {
    return v + 1;
}"#
    );
}

#[test]
fn getter_only_exports_from_next_15_5_are_recovered() {
    let source = client_chunk(
        r#"
101, e => {
  "use strict";
  e.s(["default", () => l, "named", () => m], 101);
  function l() { return "alpha"; }
  const m = "beta";
}
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"function l() {
    return "alpha";
}
export { l as default };
export const named = "beta";"#
    );
}

#[test]
fn commonjs_factories_keep_module_and_exports() {
    let source = client_chunk(
        r#"
101, (e, t, r) => {
  t.exports = { value: 1 };
},
202, (e, t, r) => {
  "use strict";
  var n = e.r(101);
  r.read = function () { return n.value; };
}
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        "export default {\n    value: 1\n};"
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"import n from "./module-101.js";
export const read = function() {
    return n.value;
};"#
    );
}

#[test]
fn value_and_namespace_exports_assign_module_exports() {
    let source = client_chunk(
        r#"
101, t => { t.v("alpha"); },
202, t => { t.n({ beta: 1 }); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export default "alpha";"#
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        "export default {\n    beta: 1\n};"
    );
}

#[test]
fn alias_ids_resolve_to_the_first_id_of_their_factory() {
    let source = client_chunk(
        r#"
101, 102, t => { t.v("shared"); },
202, t => { "use strict"; var e = t.r(102); t.s(["default", 0, e]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert!(!output
        .modules
        .iter()
        .any(|(name, _)| name == "module-102.js"));
    assert_eq!(
        module(&output, "module-202.js").trim(),
        "import e from \"./module-101.js\";\nexport default e;"
    );
}

#[test]
fn async_loader_modules_become_deferred_requires_of_their_target() {
    let source = client_chunk(
        r#"
101, t => {
  "use strict";
  t.s(["load", 0, function () { return t.A(301).then(e => e.message); }]);
},
301, t => {
  t.v(e => Promise.all(["static/chunks/lazy-beta.js"].map(e => t.l(e))).then(() => e(302)));
},
302, t => { "use strict"; t.s(["message", 0, "lazy"]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    // The loader module stays available to consumers in other inputs.
    assert_eq!(
        module(&output, "module-301.js").trim(),
        r#"export default (()=>Promise.resolve().then(()=>require("./module-302.js")));"#
    );
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export const load = function() {
    return Promise.resolve().then(()=>require("./module-302.js")).then((e)=>e.message);
};"#
    );
}

#[test]
fn async_loader_targets_in_another_input_are_rewritten_across_inputs() {
    let consumer = client_chunk(
        r#"
101, t => { "use strict"; t.s(["load", 0, () => t.A(301)]); },
301, t => { t.v(e => Promise.all(["static/chunks/lazy-beta.js"].map(e => t.l(e))).then(() => e(302))); }
"#,
    );
    let lazy = client_chunk(r#"302, t => { "use strict"; t.s(["message", 0, "lazy"]); }"#);
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".into(),
                source: consumer,
            },
            UnpackInput {
                filename: "lazy-beta.js".into(),
                source: lazy,
            },
        ],
        DecompileOptions::default(),
    )
    .expect("multi-input unpack should succeed");
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    let consumer = module(&output, "module-101.js");
    assert!(consumer.contains("./module-302.js"), "{consumer}");
}

#[test]
fn shadowed_context_names_inside_nested_functions_are_not_translated() {
    let source = client_chunk(
        r#"
101, t => {
  "use strict";
  t.s(["read", 0, function (t) { return t.i; }]);
}
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        "export const read = function(t) {\n    return t.i;\n};"
    );
}

#[test]
fn computed_global_names_are_detected() {
    let source = r#"(globalThis["TURBOPACK_remote_chunk_loading_global_example-app"] || (globalThis["TURBOPACK_remote_chunk_loading_global_example-app"] = [])).push([document.currentScript, 101, t => { t.v("alpha"); }]);"#;
    let output = unpack_chunk(source);
    assert_clean(&output);
    assert!(module(&output, "module-101.js").contains("\"alpha\""));
}

#[test]
fn server_chunks_and_externals_are_recovered() {
    let source = r#"module.exports = [
101, (a, b, c) => { b.exports = a.x("node:crypto", () => require("node:crypto")); },
202, a => { "use strict"; var b = a.i(101); a.s(["hash", 0, () => b.randomUUID()]); }
];"#;
    let output = unpack_chunk(source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export default require("node:crypto");"#
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"import * as b from "node:crypto";
export const hash = ()=>b.randomUUID();"#
    );
}

#[test]
fn unsupported_context_members_keep_only_that_factory_opaque() {
    let source = client_chunk(
        r#"
101, t => { t.v(t.U("./a.js")); },
202, t => { t.v("alpha"); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-101.js");
    assert_eq!(warning.kind, UnpackWarningKind::DecompileFailed);
    assert!(warning.message.contains("`U`"), "{}", warning.message);
    assert!(module(&output, "module-101.js").contains("t.U("));
    assert!(module(&output, "module-202.js").contains("export default \"alpha\""));
}

#[test]
fn chunk_loading_members_stay_as_runtime_residuals() {
    let source = client_chunk(
        r#"
101, t => {
  "use strict";
  t.s(["preload", 0, function (url) { return t.L(url); }, "load", 0, function (chunks) { return Promise.all(chunks.map(t.l)); }]);
},
202, t => { t.v("alpha"); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-101.js");
    assert_eq!(warning.kind, UnpackWarningKind::RuntimeResidual);
    assert!(!warning.kind.is_error());
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export const preload = function(url) {
    return __turbopack_context__.L(url);
};
export const load = function(chunks) {
    return Promise.all(chunks.map(__turbopack_context__.l));
};"#
    );
}

#[test]
fn chunk_loading_residuals_are_not_captured_by_a_local_binding() {
    let source = client_chunk(
        r#"
101, t => { var __turbopack_context__ = 1; t.v(t.L("static/chunks/other.js")); },
202, t => { t.v("alpha"); }
"#,
    );
    let output = unpack_chunk(&source);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-101.js");
    assert_eq!(warning.kind, UnpackWarningKind::DecompileFailed);
    assert!(warning.message.contains("`L`"), "{}", warning.message);
    assert!(module(&output, "module-101.js").contains("t.L("));
}

#[test]
fn path_and_host_require_members_stay_as_runtime_residuals() {
    // `import.meta` emulation (`P` before 16.3, `F` since), the host
    // `require` probed by next/dynamic, and the throwing require stub.
    let source = client_chunk(
        r#"
101, e => {
  "use strict";
  let a = { get url() { return `file://${e.P("node_modules/a/index.mjs")}`; } };
  let b = { get url() { return e.F("node_modules/b/index.mjs"); } };
  var weak = "function" == typeof e.t.resolveWeak;
  var stub = e.z;
  e.s(["a", 0, a, "b", 0, b, "weak", 0, weak, "stub", 0, stub]);
},
202, t => { t.v("alpha"); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-101.js");
    assert_eq!(warning.kind, UnpackWarningKind::RuntimeResidual);
    assert!(warning.message.contains("`P`"), "{}", warning.message);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"let a = {
    get url () {
        return `file://${__turbopack_context__.P("node_modules/a/index.mjs")}`;
    }
};
let b = {
    get url () {
        return __turbopack_context__.F("node_modules/b/index.mjs");
    }
};
const weak = typeof __turbopack_context__.t.resolveWeak === "function";
const stub = __turbopack_context__.z;
export { a };
export { b };
export { weak };
export { stub };"#
    );
}

#[test]
fn discarded_require_reads_in_amd_branches_are_dropped() {
    // A UMD wrapper's AMD branch reads `ctx.r` for its discarded `define`
    // dependency before registering the value.
    let source = client_chunk(
        r#"
101, (e, t, r) => { !function () { var o = { alpha: 1 }; if ("function" == typeof define && define.amd) e.r, void 0 !== o && e.v(o); else t.exports = o; }(); },
202, (e, t, r) => { var o = 1; "function" == typeof define && define.amd && (e.r, e.v(o)); },
303, e => { e.r, e.s(["x", 0, 1]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"(()=>{
    const o = {
        alpha: 1
    };
    if (typeof define === "function" && define.amd) {
        if (o !== undefined) {
            module.exports = o;
        }
    } else {
        module.exports = o;
    }
})();"#
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"const o = 1;
if (typeof define === "function" && define.amd) {
    module.exports = o;
}"#
    );
    assert_eq!(
        module(&output, "module-303.js").trim(),
        "export const x = 1;"
    );
}

#[test]
fn amd_factories_receive_require_and_their_discarded_result_is_exported() {
    // Turbopack's `define(factory)` wrapper: an arrow IIFE that calls the
    // factory with `(ctx.r, exports, module)` and registers its result.
    let source = client_chunk(
        r#"
101, (e, t, r) => {
  var s = function (req) { return { alpha: 1 }; };
  "function" == typeof define && define.amd ? ((n, a = "function" != typeof n ? n : n(e.r, r, t)) => void 0 !== a && e.v(a))(s) : window.lib = s();
},
202, (e, t, r) => { var x = ((a) => e.v(a))(1); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-202.js");
    assert!(warning.message.contains("`v`"), "{}", warning.message);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"const s = (req)=>({
        alpha: 1
    });
if (typeof define === "function" && define.amd) {
    ((n, a = typeof n !== "function" ? n : n(require, exports, module))=>a !== undefined && (module.exports = a))(s);
} else {
    window.lib = s();
}"#
    );
}

#[test]
fn bound_runtime_functions_become_require_and_bound_residuals() {
    // An App Router server page entry passes runtime functions as values.
    let source = client_chunk(
        r#"
101, a => { "use strict"; let k = a.r.bind(a), l = a.l.bind(a); a.s(["routeModule", 0, { require: k, loadChunk: l }]); },
202, a => { "use strict"; let m = a.U.bind(a); a.s(["m", 0, m]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [failure, residual] = output.warnings.as_slice() else {
        panic!("expected two warnings: {:?}", output.warnings);
    };
    assert_eq!(failure.filename, "module-202.js");
    assert!(failure.message.contains("`U`"), "{}", failure.message);
    assert_eq!(residual.filename, "module-101.js");
    assert_eq!(residual.kind, UnpackWarningKind::RuntimeResidual);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"let require_1 = require;
let loadChunk = __turbopack_context__.l.bind(__turbopack_context__);
export const routeModule = {
    require: require_1,
    loadChunk
};"#
    );
}

#[test]
fn a_factory_that_cannot_take_webpack_parameter_names_stays_opaque_alone() {
    // Turbopack's AMD wrapper leaves `exports` and `module` free; renaming
    // the translated parameters to those names would capture them.
    let source = client_chunk(
        r#"
101, e => { "function" == typeof define && define.amd ? (e.r, exports, module, e.v(1)) : window.x = 1; },
202, t => { t.v("alpha"); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-101.js");
    assert_eq!(
        warning.kind,
        UnpackWarningKind::WebpackFactoryRecoveryFailed
    );
    assert!(module(&output, "module-101.js").contains("e.r, exports, module"));
    assert!(module(&output, "module-202.js").contains("export default \"alpha\""));
}

#[test]
fn free_require_calls_in_translated_factories_share_the_require_name() {
    // Next's app-page template calls
    // `require("path").join(/* turbopackIgnore: true */ process.cwd(), dir)`,
    // which Turbopack leaves as a free host `require`.
    let source = client_chunk(
        r#"
101, e => { var d = e.r(202); e.s(["dir", 0, require("path").join(process.cwd(), d.dir)]); },
202, (e, r, t) => { t.dir = "beta"; }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    let code = module(&output, "module-101.js");
    assert!(
        code.contains(r#"require("path").join(process.cwd()"#),
        "{code}"
    );
    assert!(code.contains("./module-202.js"), "{code}");
}

#[test]
fn module_contexts_called_with_a_listed_constant_become_that_entry() {
    // Next.js resolves its instrumentation hook through a one-entry context.
    let source = client_chunk(
        r#"
101, (e, t, r) => { "use strict"; t.exports = e.f({ "private-next-instrumentation-client": { id: () => 303, module: () => e.r(303) } })("private-next-instrumentation-client"); },
303, e => { "use strict"; e.s(["onRouterTransitionStart", 0, function () {}]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export default require("./module-303.js");"#
    );
}

#[test]
fn module_contexts_with_dynamic_requests_use_a_local_runtime_copy() {
    // `import(`../icons/${name}`)` compiles to a context map of lazy entries.
    let source = client_chunk(
        r#"
101, e => {
  "use strict";
  e.s(["load", 0, function (n) {
    return e.f({ "../icons/a.js": { id: () => 301, module: () => e.A(301) }, "../icons/b.js": { id: () => 404, module: () => e.r(404) } }).import(`../icons/${n}.js`);
  }, "missing", 0, function () { return e.f({ "./x.js": { id: () => 404, module: () => e.r(404) } })("./y.js"); }]);
},
301, t => {
  t.v(e => Promise.all(["static/chunks/lazy-a.js"].map(e => t.l(e))).then(() => e(302)));
},
302, t => { "use strict"; t.s(["icon", 0, "a"]); },
404, t => { "use strict"; t.s(["icon", 0, "b"]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    // The requires stay inside their entry thunks, so loading stays lazy.
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r##"function moduleContext(map) {
    function request(id) {
        const hash = id.indexOf("#");
        if (hash !== -1) {
            id = id.substring(0, hash);
        }
        const query = id.indexOf("?");
        if (query !== -1) {
            id = id.substring(0, query);
        }
        return id;
    }
    function context(id) {
        id = request(id);
        if (Object.prototype.hasOwnProperty.call(map, id)) {
            return map[id].module();
        }
        const error = new Error(`Cannot find module '${id}'`);
        error.code = "MODULE_NOT_FOUND";
        throw error;
    }
    context.keys = ()=>Object.keys(map);
    context.resolve = (id)=>{
        id = request(id);
        if (Object.prototype.hasOwnProperty.call(map, id)) {
            return map[id].id();
        }
        const error = new Error(`Cannot find module '${id}'`);
        error.code = "MODULE_NOT_FOUND";
        throw error;
    };
    context.import = async (id)=>await context(id);
    return context;
}
export const load = function(n) {
    return moduleContext({
        "../icons/a.js": {
            id: ()=>301,
            module: ()=>Promise.resolve().then(()=>require("./module-302.js"))
        },
        "../icons/b.js": {
            id: ()=>404,
            module: ()=>require("./module-404.js")
        }
    }).import(`../icons/${n}.js`);
};
export const missing = function() {
    return moduleContext({
        "./x.js": {
            id: ()=>404,
            module: ()=>require("./module-404.js")
        }
    })("./y.js");
};"##
    );
}

#[test]
fn module_contexts_in_next_15_object_containers_keep_the_request_unchanged() {
    // Before 16.1 the runtime looks the request up without dropping a query
    // or fragment.
    let source = format!(
        "{LEGACY_CLIENT_PREFIX}{}}}]);",
        r#"
101: e => { "use strict"; e.v(function (n) { return e.f({ "./a.js": { id: () => 202, module: () => e.r(202) } })(n); }); },
202: e => { "use strict"; e.v("alpha"); }
"#
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    let helper = module(&output, "module-101.js");
    assert!(helper.contains("function moduleContext(map)"), "{helper}");
    assert!(!helper.contains("indexOf"), "{helper}");
}

#[test]
fn module_contexts_stay_opaque_when_a_runtime_global_is_shadowed() {
    let source = client_chunk(
        r#"
101, e => { "use strict"; var Object = 1; e.v(e.f({ "./a.js": { id: () => 202, module: () => e.r(202) } })(name)); },
202, t => { t.v("alpha"); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-101.js");
    assert!(warning.message.contains("`f`"), "{}", warning.message);
}

#[test]
fn setters_and_foreign_export_targets_are_not_translated() {
    for payload in [
        // getter followed by a setter
        r#"101, t => { t.s(["value", () => n, e => { n = e; }]); let n = 1; }, 202, t => { t.v(1); }"#,
        // a listed member without registrations
        r#"101, 102, t => { t.s(["a", 0, 1], 101); t.s(["b", 0, 2], 999); }, 202, t => { t.v(1); }"#,
        // an unlisted member that another factory defines
        r#"101, t => { t.s(["a", 0, 1], 101); t.s(["b", 0, 2], 202); }, 202, t => { t.v(1); }"#,
        // a registration without an id inside a merged group
        r#"101, t => { t.s(["a", 0, 1]); t.s(["b", 0, 2], 999); }, 202, t => { t.v(1); }"#,
    ] {
        let output = unpack_chunk(&client_chunk(payload));
        assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| warning.filename == "module-101.js"
                    && warning.kind == UnpackWarningKind::DecompileFailed),
            "{payload}: {:?}",
            output.warnings
        );
    }
}

#[test]
fn a_local_global_this_binding_blocks_the_global_translation() {
    let source = client_chunk(
        r#"
101, t => { var globalThis = {}; t.v(t.g.location); },
202, t => { t.v(1); }
"#,
    );
    let output = unpack_chunk(&source);
    assert!(output
        .warnings
        .iter()
        .any(|warning| warning.filename == "module-101.js"));
}

#[test]
fn non_container_shapes_are_not_detected() {
    for source in [
        // runtime registration only
        format!(
            r#"{CLIENT_PREFIX}{{ otherChunks: ["static/chunks/a.js"], runtimeModuleIds: [101] }}]);"#
        ),
        // strict-mode factory group from unreleased builds
        client_chunk(
            r#"(() => { "use strict"; return [101, t => { t.v(1); }]; })(), 202, t => { t.v(2); }"#,
        ),
        // string module ids
        client_chunk(r#""[project]/app/page.js", t => { t.v(1); }"#),
        // trailing id without a factory
        client_chunk(r#"101, t => { t.v(1); }, 202"#),
        // another top-level statement beside the container
        format!(
            "var globalThis = {{}};\n{}",
            client_chunk(r#"101, t => { t.v(1); }"#)
        ),
        // an ordinary CommonJS array export
        r#"module.exports = [1, function (a) { return a + 1; }];"#.to_string(),
    ] {
        let output = unpack_raw(&source, &DecompileOptions::default()).expect("unpack_raw");
        assert!(
            !output.detected_formats.contains(&BundleFormat::Turbopack),
            "detected Turbopack in:\n{source}"
        );
    }
}

#[test]
fn dynamic_requires_stay_runtime_requires() {
    let source = client_chunk(r#"101, t => { t.v(function (n) { return t.r(n); }); }"#);
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        "export default function(n) {\n    return require(n);\n};"
    );
}

const LEGACY_CLIENT_PREFIX: &str = r#"(globalThis.TURBOPACK = globalThis.TURBOPACK || []).push(["object" == typeof document ? document.currentScript : void 0, {"#;

#[test]
fn next_15_3_object_containers_are_recovered() {
    // 15.3–15.4: an object keyed by id, a context preamble, a block-wrapped
    // body, object-form getters, and an inline async loader call.
    let source = format!(
        "{LEGACY_CLIENT_PREFIX}{}}}]);",
        r#"
101: e => { var { g: t, __dirname: l } = e; { "use strict"; e.s({ label: () => t }); let t = "alpha"; } },
202: e => { "use strict"; var { g: t, __dirname: l } = e; e.s({ default: () => c }); var r = e.i(101); function c() { return e.r(301)(e.i).then(e => r.label + e.message); } },
301: e => { var { g: t, __dirname: l } = e; e.v(t => Promise.all(["static/chunks/lazy-beta.js"].map(t => e.l(t))).then(() => t(302))); },
302: e => { var { g: t, __dirname: l } = e; { e.s({ message: () => t }); let t = "beta"; } }
"#
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export let label = "alpha";"#
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"import { label } from "./module-101.js";
function c() {
    return Promise.resolve().then(()=>require("./module-302.js")).then((e)=>label + e.message);
}
export { c as default };"#
    );
}

#[test]
fn next_15_preamble_bindings_survive_redeclared_spellings() {
    // The block redeclares both the context parameter (`e`) and the
    // module binding's spelling (`r`); 15.4 lists alias ids beside the
    // factory; a minifier merged another declarator into the preamble.
    let source = format!(
        "{LEGACY_CLIENT_PREFIX}{}}}]);",
        r#"
403: [e => { var { g: t, __dirname: n, m: r, e: o } = e; { "use strict"; let e = 1, r = 2; o.value = e + r; } }, [404]],
505: e => { var a, { g: t, __dirname: n, m: r, e: o } = e; a = e.r(404); r.exports = a.value; }
"#
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-403.js").trim(),
        "let e = 1;\nlet r = 2;\nexport const value = e + r;"
    );
    assert_eq!(
        module(&output, "module-505.js").trim(),
        "let a;\na = require(\"./module-403.js\");\nexport default a.value;"
    );
}

#[test]
fn next_15_3_server_objects_recover_and_dirname_reads_stay_opaque() {
    let source = r#"module.exports = {
101: function (a) { var { g: b, __dirname: c, m: d, e: e } = a; d.exports = a.x("node:path", () => require("node:path")); },
202: function (a) { var { g: b, __dirname: c, m: d, e: e } = a; "use strict"; d.exports = a.r(101).join(c, "x"); }
};"#;
    let output = unpack_chunk(source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export default require("node:path");"#
    );
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-202.js");
    assert!(warning.message.contains("__dirname"), "{}", warning.message);
}

#[test]
fn next_15_3_evaluate_registrations_are_not_modules() {
    let source = format!(
        r#"{LEGACY_CLIENT_PREFIX}}}, {{ otherChunks: ["static/chunks/a.js"], runtimeModuleIds: [101] }}]);"#
    );
    let output = unpack_raw(&source, &DecompileOptions::default()).expect("unpack_raw");
    assert!(!output.detected_formats.contains(&BundleFormat::Turbopack));
}

#[test]
fn loaders_in_another_input_are_called_through_their_module() {
    let consumer = client_chunk(r#"101, t => { "use strict"; t.s(["load", 0, () => t.A(301)]); }"#);
    let lazy = client_chunk(
        r#"
301, t => { t.v(e => Promise.all(["static/chunks/lazy-beta.js"].map(e => t.l(e))).then(() => e(302))); },
302, t => { "use strict"; t.s(["message", 0, "lazy"]); }
"#,
    );
    let output = unpack_files(
        vec![
            UnpackInput {
                filename: "consumer.js".into(),
                source: consumer,
            },
            UnpackInput {
                filename: "lazy-beta.js".into(),
                source: lazy,
            },
        ],
        DecompileOptions::default(),
    )
    .expect("multi-input unpack should succeed");
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export const load = ()=>require("./module-301.js")();"#
    );
}

#[test]
fn exports_module_and_asset_url_members_are_translated() {
    let source = client_chunk(
        r#"
101, t => { t.e.value = 1; t.m.exports.other = 2; },
202, t => { t.q("/static/media/alpha.png", 202); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        "export const value = 1;\nexport const other = 2;"
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"export default "/static/media/alpha.png";"#
    );
}

#[test]
fn typescript_helper_guards_on_top_level_this_are_recognized() {
    // Turbopack compiles TypeScript's `(this && this.__importDefault) || …`
    // guard to `ctx.e && ctx.e.__importDefault`.
    let source = client_chunk(
        r#"
101, (e, r, t) => { "use strict"; var n = e.e && e.e.__importDefault || function (m) { return m && m.__esModule ? m : { default: m }; }; Object.defineProperty(t, "__esModule", { value: !0 }); var d = n(e.r(202)); t.value = d.default.label; },
202, (e, r, t) => { t.label = "beta"; }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    let code = module(&output, "module-101.js");
    assert!(!code.contains("__importDefault"), "{code}");
    assert!(!code.contains("exports"), "{code}");
    assert!(code.contains("export const value"), "{code}");
}

#[test]
fn helper_guards_inside_functions_keep_the_exports_object() {
    // Inside a non-arrow function `this` is that function's receiver, so only
    // a top-level guard can name the module's `this`.
    let source = client_chunk(
        r#"
101, (e, r, t) => { t.read = function () { return e.e && e.e.__helper; }; }
"#,
    );
    let output = unpack_chunk(&source);
    let code = module(&output, "module-101.js");
    assert!(code.contains("exports && exports.__helper"), "{code}");
    assert!(!code.contains("this"), "{code}");
}

#[test]
fn value_exports_whose_result_is_discarded_become_assignments() {
    // A UMD wrapper registers its value conditionally.
    let source = client_chunk(
        r#"
101, t => { !function () { var o = { alpha: 1 }; if ("object" == typeof t.m) t.v(o); else window.alpha = o; }(); },
202, t => { var x = t.v(1); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_eq!(output.detected_formats, [BundleFormat::Turbopack]);
    let [warning] = output.warnings.as_slice() else {
        panic!("expected one warning: {:?}", output.warnings);
    };
    assert_eq!(warning.filename, "module-202.js");
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"(()=>{
    const o = {
        alpha: 1
    };
    if (typeof module === "object") {
        module.exports = o;
    } else {
        window.alpha = o;
    }
})();"#
    );
}

#[test]
fn expression_statements_beside_containers_become_a_prelude_module() {
    let source = format!(
        ";self.__APP_STARTED = Date.now();\n{}\nself.__APP_FLAGS = {{ beta: true }};",
        client_chunk(r#"101, t => { t.v("alpha"); }"#)
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export default "alpha";"#
    );
    let prelude = module(&output, "prelude.js");
    assert!(
        prelude.contains("self.__APP_STARTED = Date.now()"),
        "{prelude}"
    );
    assert!(prelude.contains("self.__APP_FLAGS = {"), "{prelude}");
    assert!(!prelude.contains("TURBOPACK"), "{prelude}");
}

/// The polyfill `turbopack.debugIds` prepends to every chunk.
fn debug_id_polyfill(id: &str) -> String {
    format!(
        r#";!function(){{try {{ var e="undefined"!=typeof globalThis?globalThis:"undefined"!=typeof global?global:"undefined"!=typeof window?window:"undefined"!=typeof self?self:{{}},n=(new e.Error).stack;n&&((e._debugIds|| (e._debugIds={{}}))[n]="{id}")}}catch(e){{}}}}();"#
    )
}

#[test]
fn debug_id_polyfills_are_dropped() {
    let source = format!(
        "{}\n{}",
        debug_id_polyfill("7cc192c9-cac3-6e49-0ca9-31643f48e4ad"),
        client_chunk(r#"101, t => { t.v("alpha"); }"#)
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(output.modules.len(), 1, "{:?}", output.modules);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"export default "alpha";"#
    );
}

#[test]
fn server_chunks_after_a_debug_id_polyfill_are_detected() {
    let source = format!(
        "{}\nmodule.exports = [101, a => {{ \"use strict\"; a.s([\"value\", 0, 1]); }}];",
        debug_id_polyfill("5bb7027f-323d-ee4e-de51-1994ab129d95")
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(output.modules.len(), 1, "{:?}", output.modules);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        "export const value = 1;"
    );
}

#[test]
fn edited_debug_id_polyfills_stay_in_the_prelude() {
    let source = format!(
        "{}\n{}",
        debug_id_polyfill("not-an-id\"),self.__APP_FLAGS=(\"x"),
        client_chunk(r#"101, t => { t.v("alpha"); }"#)
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert!(module(&output, "prelude.js").contains("__APP_FLAGS"));
}

#[test]
fn merged_group_members_become_facades_of_the_primary_module() {
    // One factory defines modules 101 and 102 (listed) and 999 (registered
    // only), and reads 999 back through the module cache.
    let source = client_chunk(
        r#"
101, 102, t => {
  "use strict";
  function helper(v) { return v + 1; }
  t.s(["helper", 0, helper], 101);
  const FLAG = 1;
  t.s(["FLAG", 0, FLAG], 999);
  var flags = t.i(999);
  function main() { return helper(flags.FLAG); }
  t.s(["default", 0, main, "helper", () => helper], 102);
},
202, t => { "use strict"; var e = t.i(102), f = t.i(999); t.s(["value", 0, (0, e.helper)(1) + f.FLAG]); }
"#,
    );
    let output = unpack_chunk(&source);
    assert_clean(&output);
    assert_eq!(
        module(&output, "module-101.js").trim(),
        r#"import { FLAG as FLAG_1 } from "./module-999.js";
export function helper_102(v) {
    return v + 1;
}
export { helper_102 as helper };
export const FLAG = 1;
export function default_102() {
    return helper_102(FLAG_1);
}"#
    );
    assert_eq!(
        module(&output, "module-102.js").trim(),
        r#"export { default_102 as default } from "./module-101.js";
export { helper_102 as helper } from "./module-101.js";"#
    );
    assert_eq!(
        module(&output, "module-999.js").trim(),
        r#"export { FLAG } from "./module-101.js";"#
    );
    assert_eq!(
        module(&output, "module-202.js").trim(),
        r#"import { helper } from "./module-102.js";
import { FLAG } from "./module-999.js";
export const value = helper(1) + FLAG;"#
    );
}
