use super::*;

#[test]
fn primes_compiled_script_setup_render_context() {
    let ctx = primed_context(
        r#"
const component = {
  setup(__props, { emit: fire }) {
    const returned = {};
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock("div");
}
component.render = render;
export default component;
"#,
    );

    assert_eq!(ctx.render_context, Some(Atom::from("_ctx")));
    assert!(ctx.has_compiled_script_setup);
    assert_eq!(ctx.render_props_context, Some(Atom::from("$props")));
    assert_eq!(ctx.render_setup_context, Some(Atom::from("$setup")));
    assert_eq!(ctx.setup_props_context, Some(Atom::from("__props")));
    assert!(ctx.setup_props_context_ctxt.is_some());
    assert_eq!(ctx.setup_emit_context, Some(Atom::from("fire")));
}

#[test]
fn primes_inline_setup_render_context() {
    let ctx = primed_context(
        r#"
export default {
  setup(props, { emit: fire }) {
    return (view, cache) => (
      openBlock(), createElementBlock("button", { onClick: () => fire("save") })
    );
  }
};
"#,
    );

    assert_eq!(ctx.render_context, Some(Atom::from("view")));
    assert!(!ctx.has_compiled_script_setup);
    assert_eq!(ctx.render_props_context, None);
    assert_eq!(ctx.render_setup_context, None);
    assert_eq!(ctx.setup_props_context, Some(Atom::from("props")));
    assert!(ctx.setup_props_context_ctxt.is_some());
    assert_eq!(ctx.setup_emit_context, Some(Atom::from("fire")));
}

#[test]
fn stmt_ident_refs_reports_sibling_scope_free_references() {
    // `resolver()` assigns one context per scope, so `sibling` and `handler`
    // (both top level) share a context. The declared-binding set must key on
    // (name, ctxt), not ctxt alone — otherwise `handler` treats the sibling
    // reference as one of its own declarations and the dependency is dropped.
    let stmts = resolved_stmts("const sibling = 1; function handler() { return sibling; }");
    let handler = stmts.into_iter().nth(1).expect("handler statement");
    let refs = stmt_ident_refs(&handler);
    assert!(refs.contains(&Atom::from("sibling")));
    assert!(!refs.contains(&Atom::from("handler")));
}

#[test]
fn stmt_ident_refs_excludes_shadowing_locals() {
    // A nested arrow param `outer` shadows any outer binding of that name; its
    // uses must not be reported as free references, while a genuine free
    // reference (`external`) still is. Guards the ScopeStack -> (name, ctxt)
    // conversion of the cleaned-AST reference collectors.
    let stmts = resolved_stmts("const f = (outer) => outer.method(external);");
    let refs = stmt_ident_refs(&stmts[0]);
    assert!(refs.contains(&Atom::from("external")));
    assert!(!refs.contains(&Atom::from("outer")));
    assert!(!refs.contains(&Atom::from("f")));
}

#[test]
fn binding_table_lists_ref_cleanup_bindings_by_context() {
    let mut table = VueBindingTable::default();
    table.refs.insert(Atom::from("count"));
    table.template_refs.insert(Atom::from("el"));
    table
        .aliases
        .insert(Atom::from("count"), Atom::from("countAlias"));
    table
        .aliases
        .insert(Atom::from("el"), Atom::from("elAlias"));
    table
        .aliases
        .insert(Atom::from("plainAlias"), Atom::from("plain"));

    assert_eq!(table.ref_value_cleanup_bindings(false), vec!["count"]);
    assert_eq!(
        table.ref_value_cleanup_bindings(true),
        vec!["count", "countAlias", "el", "elAlias"]
    );
}

#[test]
fn ignores_plain_render_function_without_vue_signal() {
    let input = r#"
export function render() {
  return "not a Vue render";
}
"#;

    assert!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn ignores_marker_only_recovered_template() {
    let input = r#"
import { openBlock } from "vue";
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.node;
}
"#;

    assert!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn ignores_vue_import_without_render_helper_call() {
    let input = r#"
import { ref } from "vue";
const __sfc__ = { props: { msg: String } };
export function render() {
  return "not a Vue render";
}
"#;

    assert!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn detects_likely_vue_sfc_render_sources() {
    let plain_render = r#"
export function render() {
  return "not a Vue render";
}
"#;
    let vue_import_without_helper = r#"
import { ref } from "vue";
export function render() {
  return "not a Vue render";
}
"#;
    let vue_render = r#"
import { openBlock as o, createElementBlock as h } from "vue";
export function render(_ctx, _cache) {
  return o(), h("main", null, "Aliased");
}
"#;

    assert!(!is_likely_vue_sfc_source(plain_render).unwrap());
    assert!(!is_likely_vue_sfc_source(vue_import_without_helper).unwrap());
    assert!(is_likely_vue_sfc_source(vue_render).unwrap());
}

#[test]
fn recovers_aliased_vue_helper_signal() {
    let input = r#"
import { openBlock as o, createElementBlock as h } from "vue";
export function render(_ctx, _cache) {
  return o(), h("main", null, "Aliased");
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <main>Aliased</main>\n</template>\n"
    );
}

#[test]
fn recovers_webpack_namespace_vue_helpers() {
    let input = r#"
import * as Vue from "vue";
const _hoisted_1 = { class: "notice" };
export function render(_ctx, _cache) {
  return Vue.openBlock(), Vue.createElementBlock("section", _hoisted_1, Vue.toDisplayString(_ctx.message), 3);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <section class=\"notice\">{{ message }}</section>\n</template>\n"
    );
}

#[test]
fn recovers_webpack_require_vue_runtime_namespace() {
    let input = r#"
import { A } from "./module-262.js";
const vue_runtime_esm_bundler_js_ = require(536);
const _hoisted_1 = { style: { color: "red" } };
function render(_ctx, _cache, $props, $setup, $data, $options) {
  vue_runtime_esm_bundler_js_.openBlock();
  return vue_runtime_esm_bundler_js_.createElementBlock("div", _hoisted_1, vue_runtime_esm_bundler_js_.toDisplayString($data.title), 1);
}
const Contentvue_type_script_lang_js = {
  data() {
    return { title: "Remote Component in Action.." };
  }
};
const __exports__ = A(Contentvue_type_script_lang_js, [["render", render]]);
const Content = __exports__;
export { Content as default };
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script>\nexport default {\n    data () {\n        return {\n            title: \"Remote Component in Action..\"\n        };\n    }\n}\n</script>\n\n<template>\n  <div :style='{ color: \"red\" }'>{{ $data.title }}</div>\n</template>\n"
        );
}

#[test]
fn imports_webpack_vue_namespace_used_by_options_script() {
    let input = r#"
import { A } from "./module-262.js";
const vue_runtime_esm_bundler_js_ = require(536);
function render(_ctx, _cache) {
  vue_runtime_esm_bundler_js_.openBlock();
  return vue_runtime_esm_bundler_js_.createElementBlock("button", { onClick: _ctx.inc }, vue_runtime_esm_bundler_js_.toDisplayString(_ctx.count), 9, ["onClick"]);
}
const Appvue_type_script_lang_js = {
  setup() {
    const count = vue_runtime_esm_bundler_js_.ref(0);
    const inc = () => {
      count.value++;
    };
    return { count, inc };
  }
};
const __exports__ = A(Appvue_type_script_lang_js, [["render", render]]);
export { __exports__ as default };
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script>\nimport * as vue_runtime_esm_bundler_js_ from \"vue\";\n\nexport default {\n    setup () {\n        const count = vue_runtime_esm_bundler_js_.ref(0);\n        const inc = ()=>{\n            count.value++;\n        };\n        return {\n            count,\n            inc\n        };\n    }\n}\n</script>\n\n<template>\n  <button @click=\"inc\">{{ count }}</button>\n</template>\n"
        );
}

#[test]
fn decompiles_then_recovers_vue_sfc() {
    let input = r#"
import { toDisplayString as _toDisplayString, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  return (_openBlock(), _createElementBlock("div", null, _toDisplayString(_ctx.msg), 1));
}
__sfc__.render = render;
export default __sfc__;
"#;

    assert_eq!(
            decompile_sfc(input, DecompileOptions::default()).unwrap().code,
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
}

#[test]
fn decompiled_vue_sfc_clears_stale_js_source_map() {
    let input = r#"
import { toDisplayString as _toDisplayString, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  return (_openBlock(), _createElementBlock("div", null, _toDisplayString(_ctx.msg), 1));
}
__sfc__.render = render;
export default __sfc__;
"#;

    let output = decompile_sfc(
        input,
        DecompileOptions {
            emit_source_map: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(
        output.source_map.is_none(),
        "recovered SFC output must not keep the JS source map"
    );
    assert!(
        output.code.starts_with("<script>"),
        "expected recovered SFC output, got:\n{}",
        output.code
    );
}

#[test]
fn decompiles_single_system_register_vue_sfc() {
    let input = r#"
System.register(["./vendor-vue.js"], function (exports) {
  "use strict";
  var defineComponent, openBlock, createElementBlock;
  return {
    setters: [
      function (module) {
        defineComponent = module.d, openBlock = module.q, createElementBlock = module.X;
      }
    ],
    execute: function () {
      exports("_", defineComponent({
        __name: "LegacyGreeting",
        setup: function () {
          return function () {
            return openBlock(), createElementBlock("p", null, "Legacy");
          };
        }
      }));
    }
  };
});
"#;

    assert_eq!(
        decompile_sfc(input, DecompileOptions::default())
            .unwrap()
            .code,
        "<template>\n  <p>Legacy</p>\n</template>\n"
    );
}

#[test]
fn decompiles_component_matching_vue_filename() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
const InnerPanel = dc({
  __name: "InnerPanel",
  setup() {
    return () => (ob(), ce("p", null, "Inner"));
  }
});
export const Z = dc({
  __name: "TargetPanel",
  setup() {
    return () => (ob(), ce("p", null, "Target"));
  }
});
"#;

    assert_eq!(
        decompile_sfc(
            input,
            DecompileOptions {
                filename: "TargetPanel.vue_vue_type_script_setup_true_lang.js".to_string(),
                ..Default::default()
            }
        )
        .unwrap()
        .code,
        "<template>\n  <p>Target</p>\n</template>\n"
    );
}
