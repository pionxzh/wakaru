use super::*;
use crate::vue_template::{VueAttr, VueExpr};

fn test_stmt(source: &str) -> Stmt {
    let cm = Lrc::new(SourceMap::default());
    let module = parse_module(source, cm).unwrap();
    match module.body.into_iter().next().unwrap() {
        ModuleItem::Stmt(stmt) => stmt,
        _ => panic!("expected statement"),
    }
}

/// Parse `source` and run `resolver()` over it (mirroring the recovery entry
/// points) so statements carry real `SyntaxContext`s, then return the top-level
/// statements.
fn resolved_stmts(source: &str) -> Vec<Stmt> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module(source, cm).unwrap();
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        module
            .body
            .into_iter()
            .filter_map(|item| match item {
                ModuleItem::Stmt(stmt) => Some(stmt),
                _ => None,
            })
            .collect()
    })
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

fn test_atoms(names: &[&str]) -> Vec<Atom> {
    names.iter().map(|name| Atom::from(*name)).collect()
}

fn test_atom_set(names: &[&str]) -> HashSet<Atom> {
    names.iter().map(|name| Atom::from(*name)).collect()
}

fn recover_source_with_imports<F>(source: &str, resolve_import: F) -> Result<Option<String>>
where
    F: FnMut(&str) -> Option<String>,
{
    recover_vue_sfc_source_from_js(
        source,
        VueSfcRecoveryOptions::default().with_import_resolver(resolve_import),
    )
}

fn decompile_sfc(source: &str, decompile: DecompileOptions) -> Result<DecompileOutput> {
    Ok(decompile_vue_sfc(source, VueSfcDecompileOptions::new(decompile))?.output)
}

fn decompile_sfc_with_imports<F>(
    source: &str,
    decompile: DecompileOptions,
    resolve_import: F,
) -> Result<DecompileOutput>
where
    F: FnMut(&str) -> Option<String>,
{
    Ok(decompile_vue_sfc(
        source,
        VueSfcDecompileOptions {
            decompile,
            recovery: VueSfcRecoveryOptions::default().with_import_resolver(resolve_import),
        },
    )?
    .output)
}

fn test_local_binding(
    source: &str,
    bindings: &[&str],
    emitted_bindings: &[&str],
    refs: &[&str],
) -> VueSetupLocalBinding {
    test_local_binding_with_scope(source, bindings, emitted_bindings, refs, false)
}

fn test_local_binding_with_scope(
    source: &str,
    bindings: &[&str],
    emitted_bindings: &[&str],
    refs: &[&str],
    module_scope: bool,
) -> VueSetupLocalBinding {
    VueSetupLocalBinding {
        bindings: test_atoms(bindings),
        emitted_bindings: test_atoms(emitted_bindings),
        refs: test_atom_set(refs),
        source: source.to_string(),
        import_refs: HashSet::new(),
        stmt: test_stmt(source),
        module_scope,
        template_selectable: true,
        setup_order: 0,
        always_emit: false,
        preserve_ref_values: false,
    }
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

#[test]
fn recovers_static_element_with_hoisted_props() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
const _hoisted_1 = { class: "card" };
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("section", _hoisted_1, "Hello Vue");
}
__sfc__.render = render;
export default __sfc__;
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <section class=\"card\">Hello Vue</section>\n</template>\n"
    );
}

#[test]
fn recovers_interpolation_and_component_options() {
    let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(_ctx.msg), 1);
}
__sfc__.render = render;
export default __sfc__;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
}

#[test]
fn recovers_default_exported_component_options() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({ props: { msg: String } });
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(_ctx.msg), 1);
}
_sfc_main.render = render;
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
}

#[test]
fn recovers_compiled_script_setup_with_external_render_function() {
    let input = r#"
import DemoGrid from "./Grid.vue";
import { ref } from "vue";

const _sfc_ = {
  __name: "App",
  setup(__props, { expose: __expose }) {
    __expose();
    const searchQuery = ref("");
    const gridColumns = ["name", "power"];
    const gridData = [{ name: "Chuck Norris", power: Infinity }];
    const returned = { searchQuery, gridColumns, gridData, DemoGrid, ref };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};

import { createVNode, Fragment, openBlock, createElementBlock } from "vue";
function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock(Fragment, null, [
    createVNode($setup["DemoGrid"], {
      data: $setup.gridData,
      columns: $setup.gridColumns,
      "filter-key": $setup.searchQuery
    }, null, 8, ["filter-key"])
  ], 64);
}

_sfc_.render = render;
_sfc_.__file = "src/App.vue";
export default _sfc_;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(recovered.contains("<script setup>"));
    assert!(
        recovered.contains("import DemoGrid from \"./Grid.vue\";"),
        "{recovered}"
    );
    assert!(recovered.contains("import { ref } from \"vue\";"));
    assert!(recovered.contains("const searchQuery = ref(\"\");"));
    assert!(recovered.contains("<DemoGrid"));
    assert!(recovered.contains(":data=\"gridData\""));
    assert!(recovered.contains(":filter-key=\"searchQuery\""));
    assert!(!recovered.contains("$setup"));
    assert!(!recovered.contains("__isScriptSetup"));
    assert!(!recovered.contains("__expose"));
}

#[test]
fn preserves_compiled_inline_script_setup_order_and_effects() {
    let input = r#"
import { ref, watchEffect, openBlock, createElementBlock, toDisplayString } from "vue";

const API_URL = "https://example.test/items?branch=";
const component = {
  __name: "Example",
  setup(__props) {
    const branches = ["main", "minor"];
    const currentBranch = ref(branches[0]);
    const items = ref([]);
    const { ignored } = globalThis.makeState();
    watchEffect(async () => {
      items.value = await (await fetch(API_URL + currentBranch.value)).json();
    });
    return (_ctx, _cache) => (
      openBlock(),
      createElementBlock("p", null, toDisplayString(currentBranch.value), 1)
    );
  }
};

component.__file = "src/Example.vue";
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("import { ref, watchEffect } from \"vue\";"),
        "{recovered}"
    );
    assert!(recovered.contains("watchEffect(async ()=>{"), "{recovered}");
    assert!(
        recovered
            .contains("items.value = await (await fetch(API_URL + currentBranch.value)).json();"),
        "{recovered}"
    );
    assert!(
        recovered.contains("const { ignored } = globalThis.makeState();"),
        "{recovered}"
    );

    let api = recovered.find("const API_URL").unwrap();
    let branches = recovered.find("const branches").unwrap();
    let current_branch = recovered.find("const currentBranch").unwrap();
    let items = recovered.find("const items").unwrap();
    let destructuring = recovered.find("const { ignored }").unwrap();
    let effect = recovered.find("watchEffect(async").unwrap();
    assert!(
        api < branches
            && branches < current_branch
            && current_branch < items
            && items < destructuring
            && destructuring < effect,
        "compiled setup declarations must retain dependency-safe source order:\n{recovered}"
    );
}

#[test]
fn recognizes_minified_compiled_inline_script_setup() {
    let input = r#"
import { ref, watchEffect, openBlock, createElementBlock, toDisplayString } from "vue";
const component = {
  __name: "Example",
  setup(p) {
    const current = ref(0);
    watchEffect(() => console.log(current.value));
    return (c, k) => (
      openBlock(), createElementBlock("p", null, toDisplayString(current.value), 1)
    );
  }
};
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("import { ref, watchEffect } from \"vue\";"),
        "{recovered}"
    );
    assert!(
        recovered.contains("watchEffect(()=>console.log(current.value));"),
        "{recovered}"
    );
}

#[test]
fn preserves_compiled_script_setup_side_effects_and_their_imports() {
    let input = r#"
import { onUnmounted, ref, watch } from "vue";

const _sfc_ = {
  __name: "App",
  setup(__props, { expose: __expose }) {
    __expose();
    const selected = ref("");
    watch(selected, () => console.log(selected.value));
    onUnmounted(() => console.log("done"));
    console.log("__isScriptSetup");
    const returned = { selected, onUnmounted, ref, watch };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};

import { createElementBlock, openBlock, toDisplayString } from "vue";
function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock("p", null, toDisplayString($setup.selected), 1);
}

_sfc_.render = render;
export default _sfc_;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("import { onUnmounted, ref, watch } from \"vue\";"),
        "{recovered}"
    );
    assert!(
        recovered.contains("watch(selected, ()=>console.log(selected.value));"),
        "{recovered}"
    );
    assert!(
        recovered.contains("onUnmounted(()=>console.log(\"done\"));"),
        "{recovered}"
    );
    assert!(
        recovered.contains("console.log(\"__isScriptSetup\");"),
        "{recovered}"
    );
    assert!(!recovered.contains("__expose"));
    assert!(!recovered.contains("Object.defineProperty"));
}

#[test]
fn authored_script_setup_literal_does_not_mark_options_as_compiled_script_setup() {
    let input = r#"
import { createElementBlock, openBlock } from "vue";

const component = {
  setup() {
    console.log("__isScriptSetup");
    return {};
  }
};

function render(_ctx, _cache) {
  return openBlock(), createElementBlock("p", null, "Ready");
}

component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(recovered.contains("<script>"), "{recovered}");
    assert!(!recovered.contains("<script setup>"), "{recovered}");
    assert!(
        recovered.contains("console.log(\"__isScriptSetup\");"),
        "{recovered}"
    );
}

#[test]
fn recovers_minified_render_context_interpolation() {
    let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const e = { props: { msg: String } };
export function render(e, o) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(e.msg), 1);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div>{{ msg }}</div>\n</template>\n"
    );
}

#[test]
fn preserves_value_member_after_minified_render_context() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(e, _cache) {
  return openBlock(), createElementBlock("div", {
    title: e.title,
    count: items.value.filter((e) => e.ok).length
  }, null, 8, ["title", "count"]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <div :title=\"title\" :count=\"items.value.filter((e)=>e.ok).length\" />\n</template>\n"
        );
}

#[test]
fn recovers_setup_object_destructure_used_by_template() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock, toDisplayString } from "vue";
import { useData } from "./data.js";
import { useView } from "./view.js";
export default defineComponent({
  setup() {
    const view = useView();
    const { frontmatter, site } = useData();
    watch(frontmatter, refresh);
    return () => (
      openBlock(), createElementBlock("div", { title: site.value.title }, toDisplayString(view.label), 9, ["title"])
    );
  }
});
"#;
    let data = r#"
function tracked(source) {
  const value = createRef();
  watch(source, (next) => {
    value.value = next;
  });
  return readonly(value);
}
export function createData(source) {
  return {
    frontmatter: tracked(() => source.frontmatter),
    site: tracked(() => source.site)
  };
}
export function useData() {
  const data = inject(dataKey);
  if (!data) {
    throw new Error("missing data");
  }
  return data;
}
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./data.js").then(|| data.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { useData } from \"./data.js\";\nimport { useView } from \"./view.js\";\n\nconst view = useView();\nconst { frontmatter, site } = useData();\n</script>\n\n<template>\n  <div :title=\"site.title\">{{ view.label }}</div>\n</template>\n"
        );
}

#[test]
fn recovers_setup_returned_render_arrow() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "Greeting",
  setup(__props) {
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("h1", null, toDisplayString(_ctx.title), 1)
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <h1>{{ title }}</h1>\n</template>\n"
    );
}

#[test]
fn recovers_setup_render_block_component_context() {
    let input = r#"
import { defineComponent, resolveComponent, openBlock, createBlock } from "vue";
const _sfc_main = defineComponent({
  __name: "WrappedPanel",
  setup(__props) {
    return (_ctx, _cache) => {
      const _component_Panel = resolveComponent("Panel");
      return openBlock(), createBlock(_component_Panel, { title: _ctx.title }, null, 8, ["title"]);
    };
  }
});
export default _sfc_main;
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <Panel :title=\"title\" />\n</template>\n"
    );
}

#[test]
fn recovers_setup_props_context() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "PropsInput",
  setup(props) {
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("input", {
        id: props.id,
        disabled: props.disabled,
        onInput: _cache[0] || (_cache[0] = (event) => props.onChange(event.target.value))
      }, null, 40, ["id", "disabled", "onInput"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <input :id=\"id\" :disabled=\"disabled\" @input=\"onChange($event.target.value)\" />\n</template>\n"
        );
}

#[test]
fn emits_define_props_for_props_only_template_refs() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    id: String,
    disabled: Boolean,
    onChange: Function,
  },
  setup(props) {
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("input", {
        id: props.id,
        disabled: props.disabled,
        onInput: _cache[0] || (_cache[0] = (event) => props.onChange(event.target.value))
      }, null, 40, ["id", "disabled", "onInput"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    id: String,\n    disabled: Boolean,\n    onChange: Function\n});\nconst { disabled, id, onChange } = props;\n</script>\n\n<template>\n  <input :id=\"id\" :disabled=\"disabled\" @input=\"onChange($event.target.value)\" />\n</template>\n"
        );
}

#[test]
fn recovers_setup_props_alias_context() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "PropsAlias",
  setup(props) {
    const p = props;
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("span", { title: p.title }, toDisplayString(p.label), 9, ["title"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <span :title=\"title\">{{ label }}</span>\n</template>\n"
    );
}

#[test]
fn expands_setup_props_shorthand_in_script_local_declarations() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    title: String
  },
  setup(p) {
    const snapshot = { p, extra: p.title };
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("pre", null, toDisplayString(snapshot.extra), 1)
    );
  }
});
"#;

    let output = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        output.contains("const snapshot = {\n    p: props,\n    extra: title\n};"),
        "setup props shorthand should preserve the property key and rewrite the value:\n{output}"
    );
    assert!(
        !output.contains("{ p, extra: title }"),
        "setup props shorthand must not leave a stale props alias:\n{output}"
    );
}

#[test]
fn recovers_vite_vendor_vue_helper_aliases() {
    let input = r#"
import { d as dc, q as ob, X as ce, J as td } from "./vendor-vue-C85wAS_L.js";
const _sfc_main = dc({
  __name: "Greeting",
  setup(__props) {
    return (_ctx, _cache) => (
      ob(), ce("h1", null, td(_ctx.title), 1)
    );
  }
});
export default _sfc_main;
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <h1>{{ title }}</h1>\n</template>\n"
    );
}

#[test]
fn recovers_vite_static_template_literal_helper_args() {
    let input = r#"
import { f as dc, y as ob, c as eb, a as ev, rt as td } from "./vendor-vue.js";
const hoisted = { class: `notice` };
const _sfc_main = dc({
  __name: `Greeting`,
  setup() {
    return (_ctx, _cache) => (
      ob(), eb(`section`, hoisted, [
        ev(`h1`, null, `Hello`, -1),
        ev(`p`, null, td(_ctx.title), 1)
      ])
    );
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section class=\"notice\">\n    <h1>Hello</h1>\n    <p>{{ title }}</p>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_vite_static_template_literal_component_helpers() {
    let input = r#"
import { C as rc, E as wc, d as cv, f as dc, u as tv, y as ob } from "./vendor-vue.js";
const _sfc_main = dc({
  __name: `UsesLink`,
  setup() {
    return () => {
      const Link = rc(`AppLink`);
      return ob(), cv(Link, { name: `home` }, {
        default: wc(() => [
          tv(` Home `)
        ]),
        _: 1
      });
    };
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <AppLink name=\"home\">\n    <template v-slot:default> Home </template>\n  </AppLink>\n</template>\n"
        );
}

#[test]
fn recovers_aliased_block_helpers_when_not_shadowed() {
    // Control for `does_not_recover_shadowed_block_helper`: the same minified
    // aliases recover normally when nothing shadows them.
    let input = r#"
import { openBlock as o, createElementBlock as c } from "vue";
export function render(_ctx) {
  return o(), c("div", null, "hello");
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div>hello</div>\n</template>\n"
    );
}

#[test]
fn does_not_recover_shadowed_block_helper() {
    // A render-local reuses the minified alias of `createElementBlock`. The
    // `c(...)` call resolves to the local, not the Vue import, so recovery must
    // not treat it as a block helper and fabricate a `<div>`. Before Vue
    // recovery was resolver-backed this was matched by name and mis-recovered.
    let input = r#"
import { openBlock as o, createElementBlock as c } from "vue";
export function render(_ctx) {
  const c = _ctx.pickTag;
  return o(), c("div", null, "hello");
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap(),
        None,
    );
}

#[test]
fn does_not_recover_render_local_fragment_as_vue_fragment() {
    // The `Fragment` binding is render-local, not Vue's imported Fragment helper.
    // Fragment block inference must respect resolver contexts instead of treating
    // the conventional helper name as proof.
    let input = r#"
import { openBlock as o, createElementBlock as c } from "vue";
export function render(_ctx) {
  const Fragment = _ctx.pick;
  return o(), c(Fragment, null, "hello", 64);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap(),
        None,
    );
}

#[test]
fn recovers_logical_assign_cached_static_vnode() {
    let input = r#"
import { openBlock, createElementBlock, createElementVNode } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    _cache[0] ||= createElementVNode("h1", null, "Ready", -1)
  ]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <section>\n    <h1>Ready</h1>\n  </section>\n</template>\n"
    );
}

#[test]
fn recovers_runtime_core_cached_slot_text_array() {
    let input = r#"
import { C as rc, E as wc, c as eb, d as cv, u as tv, y as ob } from "./runtime-core.esm-bundler-DvtSYmKL.js";
export function render(_ctx, _cache) {
  const AppLink = rc(`AppLink`);
  return ob(), eb(`div`, null, [
    cv(AppLink, null, {
      default: wc(() => [..._cache[0] ||= [tv(` Go to Home `, -1)]]),
      _: 1
    })
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <div>\n    <AppLink>\n      <template v-slot:default> Go to Home </template>\n    </AppLink>\n  </div>\n</template>\n"
        );
}

#[test]
fn recovers_vite_vendor_vue_component_slot_aliases() {
    let input = r#"
import { d as dc, a7 as rc, q as ob, C as cv, R as wc, X as ce, J as td } from "./vendor-vue-C85wAS_L.js";
const _sfc_main = dc({
  __name: "WrappedPanel",
  setup(__props) {
    return (_ctx, _cache) => {
      const _component_Panel = rc("Panel");
      return ob(), cv(_component_Panel, { title: _ctx.title }, {
        default: wc(() => [
          ce("span", null, td(_ctx.message), 1)
        ]),
        _: 1
      }, 8, ["title"]);
    };
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <Panel :title=\"title\">\n    <template v-slot:default>\n      <span>{{ message }}</span>\n    </template>\n  </Panel>\n</template>\n"
        );
}

#[test]
fn recovers_vite_split_runtime_chunk_helper_aliases() {
    let input = r#"
import { ob, eb } from "./chunk-block.js";
import { Q, Je, gs } from "./chunk-vnode.js";
const SYMBOL_V_FGT = Symbol.for("v-fgt");
const _sfc_main = {
  __name: "GreetingCard",
  props: { msg: String },
  setup(props) {
    return (_ctx, _cache) => {
      ob();
      return eb(SYMBOL_V_FGT, null, [
        Q("h1", null, gs(props.msg), 1),
        _cache[0] || (_cache[0] = Je("Ready"))
      ], 64);
    };
  }
};
"#;
    let block_chunk = r#"
import { Q } from "./chunk-vnode.js";
let currentBlock = null;
const blockStack = [];
export function ob(e = false) {
  blockStack.push(currentBlock = e ? null : []);
}
function closeBlock(vnode) {
  vnode.dynamicChildren = currentBlock;
  return vnode;
}
export function eb(e, t, s, n, r, i) {
  return closeBlock(Q(e, t, s, n, r, i, true));
}
"#;
    let vnode_chunk = r#"
const Text = Symbol("_text");
export function Q(type, props = null, children = null, patchFlag = 0) {
  return { __v_isVNode: true, type, props, children, patchFlag };
}
export function Je(text = " ", flag = 0) {
  return Q(Text, null, text, flag);
}
export const gs = (value) => value == null ? "" : String(value);
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| match source {
                "./chunk-block.js" => Some(block_chunk.to_string()),
                "./chunk-vnode.js" => Some(vnode_chunk.to_string()),
                _ => None,
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n</script>\n\n<template>\n  <h1>{{ msg }}</h1>\n  Ready\n</template>\n"
        );
}

#[test]
fn recovers_vite_split_runtime_block_wrapper_helper_alias() {
    let input = r#"
import { ob } from "./chunk-block.js";
import { eb } from "./chunk-block-wrapper.js";
import { Q, gs } from "./chunk-vnode.js";
const _sfc_main = {
  __name: "GreetingCard",
  props: { msg: String },
  setup(props) {
    return (_ctx, _cache) => {
      ob();
      return eb("section", null, [
        Q("h1", null, gs(props.msg), 1)
      ]);
    };
  }
};
"#;
    let block_chunk = r#"
let currentBlock = null;
const blockStack = [];
export function ob(e = false) {
  blockStack.push(currentBlock = e ? null : []);
}
export function closeBlock(vnode) {
  vnode.dynamicChildren = currentBlock;
  return vnode;
}
"#;
    let block_wrapper_chunk = r#"
import { closeBlock } from "./chunk-block.js";
import { Q } from "./chunk-vnode.js";
export function eb(e, t, s, n, r, i) {
  return closeBlock(Q(e, t, s, n, r, i, true));
}
"#;
    let vnode_chunk = r#"
export function Q(type, props = null, children = null, patchFlag = 0) {
  return { __v_isVNode: true, type, props, children, patchFlag };
}
export const gs = (value) => value == null ? "" : String(value);
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| match source {
                "./chunk-block.js" => Some(block_chunk.to_string()),
                "./chunk-block-wrapper.js" => Some(block_wrapper_chunk.to_string()),
                "./chunk-vnode.js" => Some(vnode_chunk.to_string()),
                _ => None,
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n</script>\n\n<template>\n  <section>\n    <h1>{{ msg }}</h1>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_split_runtime_fragment_alias_without_export_metadata() {
    let input = r#"
import { ft } from "./chunk-ft.js";
import { It } from "./entry.js";
export function render(_ctx, _cache) {
  return ft("div", null, [
    ft(It, null, [
      ft("span", null, "Ready")
    ], 64)
  ]);
}
"#;
    let block_chunk = r#"
import { V } from "./chunk-vnode.js";
function closeBlock(vnode) {
  vnode.dynamicChildren = [];
  return vnode;
}
export function ft(t, e, n, s, r, o) {
  return closeBlock(V(t, e, n, s, r, o, true));
}
"#;

    assert_eq!(
        recover_source_with_imports(input, |source| {
            (source == "./chunk-ft.js").then(|| block_chunk.to_string())
        })
        .unwrap()
        .unwrap(),
        "<template>\n  <div>\n    <span>Ready</span>\n  </div>\n</template>\n"
    );
}

#[test]
fn recovers_vite_scoped_render_helper_with_local_options() {
    let input = r#"
import { openBlock, createElementBlock, toDisplayString } from "vue";
const base = {
  props: {
    name: {
      type: String,
      default: ""
    }
  },
  emits: ["confirm"]
};
const hoisted = { class: "todo-item" };
function render(ctx, cache) {
  return openBlock(), createElementBlock("span", hoisted, toDisplayString(ctx.name), 1);
}
const scoped = scope(base, [
  ["render", render],
  ["__scopeId", "data-v-test"]
]);
export { scoped as T };
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        name: {\n            type: String,\n            default: \"\"\n        }\n    },\n    emits: [\n        \"confirm\"\n    ]\n}\n</script>\n\n<template>\n  <span class=\"todo-item\">{{ name }}</span>\n</template>\n"
        );
}

#[test]
fn recovers_vite_scoped_render_helper_with_imported_options() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
import { base } from "./chunk-options.js";
const hoisted = { class: "app-shell" };
function render(ctx, cache) {
  return openBlock(), createElementBlock("main", hoisted, "Ready");
}
const scoped = scope(base, [
  ["__scopeId", "data-v-test"],
  ["render", render]
]);
export default scoped;
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <main class=\"app-shell\">Ready</main>\n</template>\n"
    );
}

#[test]
fn recovers_multiple_setup_components_from_one_scope_hoisted_module() {
    let input = r#"
import { openBlock, createElementBlock, createVNode } from "vue";
const Child = {
  __name: "Child",
  props: { msg: String },
  setup(props) {
    return (_ctx, _cache) => (openBlock(), createElementBlock("span", null, props.msg, 1));
  }
};
const App = {
  __name: "App",
  setup() {
    return (_ctx, _cache) => (openBlock(), createElementBlock("main", null, [
      createVNode(Child, { msg: "Hi" })
    ]));
  }
};
"#;

    let recovered = recover_vue_sfcs_from_js(input, VueSfcRecoveryOptions::default()).unwrap();
    assert_eq!(
        recovered
            .iter()
            .map(|sfc| sfc.name.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Child"), Some("App")]
    );
    assert_eq!(
            recovered[0].sfc.print(),
            "<script setup>\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n</script>\n\n<template>\n  <span>{{ msg }}</span>\n</template>\n"
        );
    assert_eq!(
        recovered[1].sfc.print(),
        "<template>\n  <main>\n    <Child msg=\"Hi\" />\n  </main>\n</template>\n"
    );
}

#[test]
fn prefers_vite_exported_component_when_chunk_has_multiple_setup_renders() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
const _sfc_banner = dc({
  __name: "Banner",
  setup() {
    return () => (ob(), ce("aside", null, "Banner"));
  }
});
const _sfc_main = dc({
  __name: "Main",
  setup() {
    return () => (ob(), ce("main", null, "Main"));
  }
});
export { _sfc_banner as T, _sfc_main as _ };
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <main>Main</main>\n</template>\n"
    );
}

#[test]
fn prefers_webpack_default_component_when_module_has_multiple_setup_renders() {
    let input = r#"
import * as Vue from "vue";
const SecondaryPanel = Vue.defineComponent({
  name: "SecondaryPanel",
  setup() {
    return () => (Vue.openBlock(), Vue.createElementBlock("aside", null, "Secondary"));
  }
});
const PrimaryPanel = Vue.defineComponent({
  name: "PrimaryPanel",
  setup() {
    return () => (Vue.openBlock(), Vue.createElementBlock("main", null, "Primary"));
  }
});
export { SecondaryPanel as Panel, PrimaryPanel as default };
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <main>Primary</main>\n</template>\n"
    );
}

#[test]
fn prefers_decompiled_vite_exported_component_decl() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
const _sfc_banner = dc({
  __name: "Banner",
  setup() {
    return () => (ob(), ce("aside", null, "Banner"));
  }
});
export const _ = dc({
  __name: "Main",
  setup() {
    return () => (ob(), ce("main", null, "Main"));
  }
});
export { _sfc_banner as T };
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <main>Main</main>\n</template>\n"
    );
}

#[test]
fn recovers_setup_render_if_return_chain() {
    let input = r#"
import { defineComponent, openBlock, createBlock, createElementVNode, createCommentVNode, withCtx } from "vue";
const _sfc_main = defineComponent({
  __name: "MaybeNotice",
  setup() {
    return (_ctx, _cache) => {
      if (_ctx.isLoaded) {
        return openBlock(), createBlock(Notice, { key: 0 }, {
          default: withCtx(() => [
            createElementVNode("span", { innerHTML: _ctx.message }, null, 8, ["innerHTML"])
          ]),
          _: 1
        });
      }
      return createCommentVNode("", true);
    };
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <Notice v-if=\"isLoaded\">\n    <template v-slot:default>\n      <span v-html=\"message\" />\n    </template>\n  </Notice>\n</template>\n"
        );
}

#[test]
fn recovers_vue_file_component_import_alias() {
    let input = r#"
import { _ as __1 } from "./Notification.vue_vue_type_script_setup_true_lang-D4OJlsAz.js";
import { d as dc, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "UsesNotification",
  setup() {
    return () => (ob(), cb(__1, { key: 0 }, null));
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { _ as Notification } from \"./Notification.vue_vue_type_script_setup_true_lang-D4OJlsAz.js\";\n</script>\n\n<template>\n  <Notification :key=\"0\" />\n</template>\n"
        );
}

#[test]
fn aliases_imported_component_when_tag_collides_with_setup_binding() {
    let input = r#"
import { defineComponent, computed, openBlock, createVNode } from "vue";
import { P } from "./Panel.vue";
export default defineComponent({
  __name: "PanelWrapper",
  setup() {
    const Panel = computed(() => createPanelState({
      title: "Ready",
      enabled: true,
      rank: 1,
      group: "main"
    }));
    return () => (
      openBlock(), createVNode(P, { state: Panel.value }, null, 8, ["state"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { P as Panel_1 } from \"./Panel.vue\";\n\nconst Panel = computed(()=>createPanelState({\n        title: \"Ready\",\n        enabled: true,\n        rank: 1,\n        group: \"main\"\n    }));\n</script>\n\n<template>\n  <Panel_1 :state=\"Panel\" />\n</template>\n"
        );
}

#[test]
fn recovers_scoped_local_component_alias() {
    let input = r#"
import { d as dc, _ as scope, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
const local = dc({
  __name: "LocalPanel",
  setup() {
    return () => (ob(), cb("section", null, "Local"));
  }
});
const scoped = scope(local, [["__scopeId", "data-v-test"]]);
export const _ = dc({
  __name: "UsesLocalPanel",
  setup() {
    return () => (ob(), cb(scoped, { title: "Ready" }, null));
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <LocalPanel title=\"Ready\" />\n</template>\n"
    );
}

#[test]
fn recovers_nested_scoped_local_component_alias() {
    let input = r#"
import { d as dc, _ as scope, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
const scoped = scope(dc({
  __name: "MyBetRow",
  setup() {
    return () => null;
  }
}), [["__scopeId", "data-v-test"]]);
export const _ = dc({
  __name: "UsesMyBetRow",
  setup() {
    return () => (ob(), cb(scoped, { title: "Ready" }, null));
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <MyBetRow title=\"Ready\" />\n</template>\n"
    );
}

#[test]
fn recovers_exported_local_component_alias() {
    let input = r#"
import { d as dc, q as ob, aa as cb, X as ce, R as wc } from "./vendor-vue-C85wAS_L.js";
export const r = dc({
  __name: "NavbarRowItem",
  setup() {
    return () => null;
  }
});
export const _ = dc({
  __name: "Navbar",
  setup() {
    return () => (
      ob(), cb(r, null, {
        default: wc(() => [
          ce("span", null, "Title")
        ]),
        _: 1
      })
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <NavbarRowItem>\n    <template v-slot:default>\n      <span>Title</span>\n    </template>\n  </NavbarRowItem>\n</template>\n"
        );
}

#[test]
fn recovers_cross_module_component_export_alias() {
    let input = r#"
import { q as ob, aa as cb, _ as rd } from "./vendor-vue.js";
import { B as B_1 } from "./main.js";
export function render(_ctx, _cache) {
  return ob(), cb(rd(B_1), { text: "Details" }, null, 8, ["text"]);
}
"#;
    let shared = r#"
import { defineComponent } from "vue";
const YP = defineComponent({
  name: "VTooltip",
  props: { text: String }
});
export { YP as B };
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./main.js").then(|| shared.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { B as VTooltip } from \"./main.js\";\n</script>\n\n<template>\n  <VTooltip text=\"Details\" />\n</template>\n"
        );
}

#[test]
fn recovers_cross_module_default_member_component_export_alias() {
    let input = r#"
import { q as ob, aa as cb } from "./vendor-vue.js";
import Child from "./Child.vue";
export function render(_ctx, _cache) {
  return ob(), cb(Child["default"], { text: "Details" }, null, 8, ["text"]);
}
"#;
    let child = r#"
import { defineComponent } from "vue";
const ChildPanel = defineComponent({
  name: "ChildPanel",
  props: { text: String }
});
export default ChildPanel;
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./Child.vue").then(|| child.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport ChildPanel from \"./Child.vue\";\n</script>\n\n<template>\n  <ChildPanel text=\"Details\" />\n</template>\n"
        );
}

#[test]
fn recovers_cross_module_systemjs_component_export_alias() {
    let input = r#"
import { q as ob, aa as cb } from "./vendor-vue.js";
import { V as V_1 } from "./main-legacy.js";
export function render(_ctx, _cache) {
  return ob(), cb(V_1, { flat: "" }, null, 8, ["flat"]);
}
"#;
    let shared = r#"
System.register(["./vendor-vue.js"], function (_export) {
  var defineComponent;
  return {
    setters: [
      function (module) {
        defineComponent = module.d;
      }
    ],
    execute: function () {
      _export("V", defineComponent({
        __name: "VButton",
        setup: function () {
          return function () {
            return null;
          };
        }
      }));
    }
  };
});
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./main-legacy.js").then(|| shared.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { V as VButton } from \"./main-legacy.js\";\n</script>\n\n<template>\n  <VButton flat />\n</template>\n"
        );
}

#[test]
fn decompiles_single_system_register_with_component_export_alias() {
    let input = r#"
System.register(["./main-legacy.js", "./vendor-vue.js"], function (_export) {
  var VButton, defineComponent, openBlock, createBlock;
  return {
    setters: [
      function (module) {
        VButton = module.V;
      },
      function (module) {
        defineComponent = module.d;
        openBlock = module.q;
        createBlock = module.aa;
      }
    ],
    execute: function () {
      _export("_", defineComponent({
        __name: "UsesButton",
        setup: function () {
          return function () {
            return openBlock(), createBlock(VButton, { flat: "" }, null, 8, ["flat"]);
          };
        }
      }));
    }
  };
});
"#;
    let shared = r#"
!function () {
  function scope(component, attrs) {
    return component;
  }
  System.register(["./side-effect.js", "./vendor-vue.js"], function (_export) {
    var defineComponent;
    return {
      setters: [
        null,
        function (module) {
          defineComponent = module.d;
        }
      ],
      execute: function () {
        var base = defineComponent({
          __name: "VButton",
          setup: function () {
            return function () {
              return null;
            };
          }
        }), scoped = scope(base, [["__scopeId", "data-v-test"]]);
        _export("V", scoped);
      }
    };
  });
}();
"#;

    assert_eq!(
            decompile_sfc_with_imports(input, DecompileOptions::default(), |source| {
                (source == "./main-legacy.js").then(|| shared.to_string())
            })
            .unwrap()
            .code,
            "<script setup>\nimport { V as VButton } from \"./main-legacy.js\";\n</script>\n\n<template>\n  <VButton flat />\n</template>\n"
        );
}

#[test]
fn decompiles_system_register_style_sequence_direct_export() {
    let input = r#"
System.register(["./Badge.vue", "./vendor-vue.js"], function (_export) {
  var Badge, defineComponent, openBlock, createBlock;
  return {
    setters: [
      function (module) {
        Badge = module.B;
      },
      function (module) {
        defineComponent = module.d;
        openBlock = module.q;
        createBlock = module.aa;
      }
    ],
    execute: function () {
      var style = document.createElement("style");
      style.textContent = ".badge{}", document.head.appendChild(style), _export("_", defineComponent({
        __name: "TeamBadge",
        setup: function (props) {
          return function (_ctx, _cache) {
            return openBlock(), createBlock(Badge, { text: props.team.name }, null, 8, ["text"]);
          };
        }
      }));
    }
  };
});
"#;

    assert_eq!(
            decompile_sfc(input, DecompileOptions::default()).unwrap().code,
            "<script setup>\nimport { B as Badge } from \"./Badge.vue\";\n</script>\n\n<template>\n  <Badge :text=\"team.name\" />\n</template>\n"
        );
}

#[test]
fn ignores_unparseable_import_source_when_resolving_component_aliases() {
    let input = r#"
import data from "./config.json";
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("div", null, "Ready");
}
"#;

    assert_eq!(
        recover_source_with_imports(input, |_| { Some("{ not javascript".to_string()) })
            .unwrap()
            .unwrap(),
        "<template>\n  <div>Ready</div>\n</template>\n"
    );
}

#[test]
fn recovers_pascal_case_chunk_component_import_alias() {
    let input = r#"
import { S as __1 } from "./SvgIcon-Dg6MjH_p.js";
import { d as dc, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "UsesSvgIcon",
  setup() {
    return () => (ob(), cb(__1, { name: "icon-system-play-video-cycle" }, null));
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as SvgIcon } from \"./SvgIcon-Dg6MjH_p.js\";\n</script>\n\n<template>\n  <SvgIcon name=\"icon-system-play-video-cycle\" />\n</template>\n"
        );
}

#[test]
fn recovers_unref_helper_alias_in_conditions_and_expressions() {
    let input = r#"
import { d as dc, _ as ur, q as ob, aa as cb, X as ce, J as td, Z as cc } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "MaybeNotice",
  setup() {
    return () => {
      if (ur(isLoaded)) {
        return ob(), cb(Notice, null, {
          default: () => [
            ce("span", null, td(ur(i18n).t("loaded")), 1)
          ],
          _: 1
        });
      }
      return cc("", true);
    };
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <Notice v-if=\"isLoaded\">\n    <template v-slot:default>\n      <span>{{ i18n.t(\"loaded\") }}</span>\n    </template>\n  </Notice>\n</template>\n"
        );
}

#[test]
fn recovers_unref_helper_alias_in_component_props_and_events() {
    let input = r#"
import { P as Panel } from "./Panel.vue";
import { d as dc, _ as ur, q as ob, aa as cb } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "PanelHost",
  setup() {
    return () => (
      ob(), cb(Panel, {
        disabled: !ur(open),
        items: ur(items),
        onClose: ur(closePanel)
      }, null, 8, ["disabled", "items", "onClose"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { P as Panel } from \"./Panel.vue\";\n</script>\n\n<template>\n  <Panel :disabled=\"!open\" :items=\"items\" @close=\"closePanel\" />\n</template>\n"
        );
}

#[test]
fn recovers_unref_helper_alias_in_render_conditions_and_lists() {
    let input = r#"
import { d as dc, _ as ur, q as ob, X as ce, F as Fragment, R as rl, Z as cc } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "PanelList",
  setup() {
    return () => (
      ob(), ce(Fragment, null, [
        ur(open) && ur(enabled)
          ? (ob(), ce("p", { key: 0 }, "Open"))
          : cc("", true),
        (ob(true), ce(Fragment, null, rl(ur(items), (item) => (
          ob(), ce("span", { key: item.id }, item.name, 1)
        )), 128))
      ], 64)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <p v-if=\"open &amp;&amp; enabled\">Open</p>\n  <span v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</span>\n</template>\n"
        );
}

#[test]
fn recovers_setup_computed_value_alias() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ComputedLabel",
  setup() {
    const label = computed(() => format(total.value));
    return () => (
      openBlock(), createElementBlock("span", { innerHTML: label.value }, null, 8, ["innerHTML"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <span v-html=\"format(total.value)\" />\n</template>\n"
    );
}

#[test]
fn computed_value_inliner_avoids_arrow_param_capture() {
    let input = r#"
import { defineComponent, computed, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  __name: "ComputedCapture",
  setup() {
    const selected = useSelected();
    const current = computed(() => selected.id);
    const items = useItems();
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(items, selected => (
          openBlock(), createElementBlock("li", {
            key: selected.id,
            class: current.value === selected.id ? "active" : ""
          }, toDisplayString(selected.name), 3)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\n\nconst selected = useSelected();\nconst current = computed(()=>selected.id);\nconst items = useItems();\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items\" :key=\"item.id\" :class='current === item.id ? \"active\" : \"\"'>{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn assignment_targets_in_nested_handlers_do_not_shadow_setup_bindings() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ToggleButton",
  setup() {
    let open = false;
    function toggle() {
      open = !open;
    }
    return () => (
      openBlock(), createElementBlock("button", { onClick: toggle }, open ? "Open" : "Closed", 9, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nlet open = false;\nfunction toggle() {\n    open = !open;\n}\n</script>\n\n<template>\n  <button @click=\"toggle\">\n    <template v-if=\"open\">\n      Open\n    </template>\n    <template v-else>\n      Closed\n    </template>\n  </button>\n</template>\n"
        );
}

#[test]
fn recovers_vite_setup_computed_value_alias() {
    let input = r#"
import { d as dc, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "ComputedMessage",
  setup() {
    const formatted = cp(() => format(total.value));
    const message = cp(() => t("max_payout_message", { value: formatted.value }));
    return () => (
      ob(), ce("span", { innerHTML: message.value }, null, 8, ["innerHTML"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <span v-html='t(\"max_payout_message\", { value: (format(total.value)) })' />\n</template>\n"
        );
}

#[test]
fn recovers_computed_value_inside_template_literal() {
    let input = r#"
import { d as dc, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "ComputedStyle",
  setup() {
    const height = cp(() => itemHeight.value + gap.value);
    return () => (
      ob(), ce("div", { style: { height: `${height.value}px` } }, null, 4)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <div :style=\"{ height: `${(itemHeight.value + gap.value)}px` }\" />\n</template>\n"
        );
}

#[test]
fn recovers_computed_block_local_return_alias() {
    let input = r#"
import { defineComponent, ref, computed, openBlock, createVNode } from "vue";
import { I as ItemPicker } from "./ItemPicker.vue";
export default defineComponent({
  __name: "ItemFilters",
  setup() {
    const sortedItems = ref([]);
    const itemFilters = computed(() => {
      const ids = sortedItems.value.map((item) => item.id);
      return uniqueBy(ids, (id) => id);
    });
    return () => (
      openBlock(), createVNode(ItemPicker, { itemFilters: itemFilters.value }, null, 8, ["itemFilters"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\nimport { I as ItemPicker } from \"./ItemPicker.vue\";\n\nconst sortedItems = ref([]);\n</script>\n\n<template>\n  <ItemPicker :itemFilters=\"uniqueBy(sortedItems.map((item)=>item.id), (id)=>id)\" />\n</template>\n"
        );
}

#[test]
fn preserves_complex_computed_template_binding() {
    let input = r#"
import { defineComponent, ref, computed, openBlock, createVNode } from "vue";
import { L as ListView } from "./ListView.vue";
export default defineComponent({
  __name: "GroupedList",
  setup() {
    const items = ref([]);
    const groups = computed(() => items.value.map((item) => {
      const label = format(item.name);
      return { label, item };
    }));
    return () => (
      openBlock(), createVNode(ListView, { groups: groups.value }, null, 8, ["groups"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed, ref } from \"vue\";\nimport { L as ListView } from \"./ListView.vue\";\n\nconst items = ref([]);\n\nconst groups = computed(()=>items.map((item)=>{\n        const label = format(item.name);\n        return {\n            label,\n            item\n        };\n    }));\n</script>\n\n<template>\n  <ListView :groups=\"groups\" />\n</template>\n"
        );
}

#[test]
fn preserves_complex_computed_object_binding() {
    let input = r#"
import { defineComponent, ref, computed, openBlock, createVNode } from "vue";
import { P as Panel } from "./Panel.vue";
export default defineComponent({
  __name: "PanelWrapper",
  setup() {
    const visible = ref(true);
    const config = computed(() => ({
      title: visible.value ? "Open" : "Closed",
      onClose: () => {
        closePanel();
      },
    }));
    return () => (
      openBlock(), createVNode(Panel, { config: config.value }, null, 8, ["config"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed, ref } from \"vue\";\nimport { P as Panel } from \"./Panel.vue\";\n\nconst visible = ref(true);\n\nconst config = computed(()=>({\n        title: visible ? \"Open\" : \"Closed\",\n        onClose: ()=>{\n            closePanel();\n        }\n    }));\n</script>\n\n<template>\n  <Panel :config=\"config\" />\n</template>\n"
        );
}

#[test]
fn orders_preserved_computed_before_dependent_setup_local() {
    let input = r#"
import { defineComponent, ref, computed, openBlock, createVNode } from "vue";
import { I as ItemPicker } from "./ItemPicker.vue";
export default defineComponent({
  __name: "FilterPanel",
  setup() {
    const items = ref([]);
    function createPanel(filters) {
      return { filters };
    }
    const filters = computed(() => uniqueBy(items.value.map((item) => ({ id: item.id, name: item.name, enabled: item.enabled, rank: item.rank })), (item) => item.id));
    const panel = createPanel(filters);
    return () => (
      openBlock(), createVNode(ItemPicker, { filters: filters.value, panel }, null, 8, ["filters", "panel"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed, ref } from \"vue\";\nimport { I as ItemPicker } from \"./ItemPicker.vue\";\n\nconst items = ref([]);\n\nfunction createPanel(filters) {\n    return {\n        filters\n    };\n}\n\nconst filters = computed(()=>uniqueBy(items.map((item)=>({\n            id: item.id,\n            name: item.name,\n            enabled: item.enabled,\n            rank: item.rank\n        })), (item)=>item.id));\n\nconst panel = createPanel(filters);\n</script>\n\n<template>\n  <ItemPicker :filters=\"filters\" :panel=\"panel\" />\n</template>\n"
        );
}

#[test]
fn inlines_plain_computed_object_style_binding() {
    let input = r#"
import { defineComponent, ref, computed, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "Badge",
  setup() {
    const clickable = ref(true);
    const padding = ref("4px");
    const style = computed(() => ({
      cursor: clickable.value ? "pointer" : "default",
      ...padding.value && { padding: padding.value },
    }));
    return () => (
      openBlock(), createElementBlock("span", { style: style.value }, "Badge", 4)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst clickable = ref(true);\nconst padding = ref(\"4px\");\n</script>\n\n<template>\n  <span :style='{ cursor: clickable ? \"pointer\" : \"default\", ...padding &amp;&amp; { padding: padding } }'>Badge</span>\n</template>\n"
        );
}

#[test]
fn recovers_computed_block_destructured_setup_props() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock, createCommentVNode } from "vue";
const _sfc_main = defineComponent({
  props: {
    show: Boolean,
    progressDuration: Number,
  },
  setup(__props) {
    const props = __props;
    const duration = computed(() => {
      const { show: isShown, progressDuration: ms } = props;
      if (isShown) {
        return ms;
      }
      return 0;
    });
    return (_ctx, _cache) => (
      openBlock(),
      createElementBlock("div", null, [
        duration.value !== void 0
          ? (openBlock(), createElementBlock("div", {
              style: `animation-duration: ${duration.value}ms;`
            }, null, 4))
          : createCommentVNode("", true)
      ])
    );
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    show: Boolean,\n    progressDuration: Number\n});\nconst { progressDuration, show } = props;\n</script>\n\n<template>\n  <div>\n    <div v-if=\"(show ? progressDuration : 0) !== void 0\" :style=\"`animation-duration: ${(show ? progressDuration : 0)}ms;`\" />\n  </div>\n</template>\n"
        );
}

#[test]
fn preserves_mutated_computed_block_local_binding() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({
  props: {
    padding: String,
  },
  setup(__props) {
    const props = __props;
    const style = computed(() => {
      const result = {};
      if (props.padding) {
        result.padding = props.padding;
      }
      return result;
    });
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", { style: style.value }, null, 4)
    );
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\n\nconst props = defineProps({\n    padding: String\n});\nconst { padding } = props;\n\nconst style = computed(()=>{\n    const result = {};\n    if (padding) {\n        result.padding = padding;\n    }\n    return result;\n});\n</script>\n\n<template>\n  <div :style=\"style\" />\n</template>\n"
        );
}

#[test]
fn imports_helpers_used_by_script_setup_computed_bindings() {
    let input = r#"
import { normalizePadding } from "./format.js";
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({
  props: {
    padding: String,
  },
  setup(props) {
    const style = computed(() => {
      const result = {};
      const value = normalizePadding(props.padding);
      if (value) {
        result.padding = value;
      }
      return result;
    });
    return () => (
      openBlock(), createElementBlock("div", { style: style.value }, null, 4)
    );
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { normalizePadding } from \"./format.js\";\n\nconst props = defineProps({\n    padding: String\n});\nconst { padding } = props;\n\nconst style = computed(()=>{\n    const result = {};\n    const value = normalizePadding(padding);\n    if (value) {\n        result.padding = value;\n    }\n    return result;\n});\n</script>\n\n<template>\n  <div :style=\"style\" />\n</template>\n"
        );
}

#[test]
fn setup_dependencies_do_not_select_shadowed_module_locals() {
    let ctx = VueRecoveryContext {
        script_local_bindings: vec![test_local_binding_with_scope(
            "const t = document.createElement(\"style\");",
            &["t"],
            &["t"],
            &[],
            true,
        )],
        setup_local_bindings: vec![
            test_local_binding(
                "const t = toRefs(props);",
                &["t"],
                &["t"],
                &["props", "toRefs"],
            ),
            test_local_binding("const value = t.event;", &["value"], &["value"], &["t"]),
        ],
        ..Default::default()
    };
    let root = VueNode::Interpolation(VueExpr::new("value.name"));
    let template_usage = VueTemplateUsage::new(&root);

    let selected = setup_local_declarations(&ctx, &template_usage)
        .into_iter()
        .map(|declaration| declaration.source.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        selected,
        vec!["const t = toRefs(props);", "const value = t.event;"]
    );
}

#[test]
fn setup_dependencies_select_object_destructure_read_by_template() {
    let ctx = VueRecoveryContext {
        bindings: VueBindingTable {
            composable_refs: test_atom_set(&["site"]),
            ..Default::default()
        },
        setup_local_bindings: vec![test_local_binding(
            "const { frontmatter, site } = useData();",
            &["frontmatter", "site"],
            &["frontmatter", "site"],
            &["useData"],
        )],
        ..Default::default()
    };
    let root = VueNode::Interpolation(VueExpr::new("site.value.contentProps"));
    let template_usage = VueTemplateUsage::new(&root);

    let selected = setup_local_declarations(&ctx, &template_usage)
        .into_iter()
        .map(|declaration| declaration.source.as_str())
        .collect::<Vec<_>>();

    assert_eq!(selected, vec!["const { frontmatter, site } = useData();"]);
}

#[test]
fn selection_plan_expands_setup_refs_to_module_dependencies() {
    let ctx = VueRecoveryContext {
        script_local_bindings: vec![
            test_local_binding_with_scope(
                "const options = getOptions();",
                &["options"],
                &["options"],
                &["getOptions"],
                true,
            ),
            test_local_binding_with_scope(
                "const format = makeFormatter(options);",
                &["format"],
                &["format"],
                &["makeFormatter", "options"],
                true,
            ),
        ],
        setup_local_bindings: vec![test_local_binding(
            "const message = format(value);",
            &["message"],
            &["message"],
            &["format", "value"],
        )],
        ..Default::default()
    };
    let root = VueNode::Interpolation(VueExpr::new("message"));
    let template_usage = VueTemplateUsage::new(&root);

    let selected = setup_local_declarations(&ctx, &template_usage)
        .into_iter()
        .map(|declaration| declaration.source.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        selected,
        vec![
            "const options = getOptions();",
            "const format = makeFormatter(options);",
            "const message = format(value);"
        ]
    );
}

#[test]
fn setup_selection_context_collects_initial_setup_refs() {
    use crate::vue_template::VueElement;

    let ctx = VueRecoveryContext {
        bindings: VueBindingTable {
            composable_refs: test_atom_set(&["store"]),
            ..Default::default()
        },
        setup_script_bindings: vec![VueSetupScriptBinding {
            binding: Atom::from("model"),
            value: "makeModel(dep)".to_string(),
            setup_order: 0,
        }],
        setup_emit_context: Some(Atom::from("emit")),
        slot_bindings: test_atom_set(&["slotProps"]),
        ..Default::default()
    };
    let candidates = [
        test_local_binding(
            "const label = format(value);",
            &["label"],
            &["label"],
            &["format", "value"],
        ),
        test_local_binding(
            "const handler = () => emit(\"save\");",
            &["handler"],
            &["handler"],
            &["emit"],
        ),
        test_local_binding_with_scope(
            "const moduleOnly = readModule();",
            &["moduleOnly"],
            &["moduleOnly"],
            &["readModule"],
            true,
        ),
    ];
    let candidate_refs = candidates.iter().collect::<Vec<_>>();
    let root = VueNode::Element(
        VueElement::new("button")
            .with_attrs(vec![VueAttr::On {
                name: "click".to_string(),
                expr: VueExpr::new("handler()"),
                modifiers: Vec::new(),
            }])
            .with_children(vec![VueNode::Interpolation(VueExpr::new(
                "label + store.name",
            ))]),
    );
    let template_usage = VueTemplateUsage::new(&root);

    let selection_context = VueSetupSelectionContext::new(&ctx, &template_usage, &candidate_refs);

    assert!(selection_context
        .setup_scope_bindings
        .contains(&Atom::from("label")));
    assert!(selection_context
        .setup_scope_bindings
        .contains(&Atom::from("handler")));
    assert!(selection_context
        .setup_scope_bindings
        .contains(&Atom::from("emit")));
    assert!(selection_context
        .setup_scope_bindings
        .contains(&Atom::from("slotProps")));
    assert!(!selection_context
        .setup_scope_bindings
        .contains(&Atom::from("moduleOnly")));
    assert!(selection_context
        .initial_setup_refs
        .contains(&Atom::from("label")));
    assert!(selection_context
        .initial_setup_refs
        .contains(&Atom::from("handler")));
    assert!(selection_context
        .initial_setup_refs
        .contains(&Atom::from("store")));
    assert!(selection_context
        .initial_setup_refs
        .contains(&Atom::from("dep")));
}

#[test]
fn setup_script_plan_collects_rendered_setup_declarations() {
    let cm = Lrc::new(SourceMap::default());
    let module = parse_module("function render() { return null; }", cm.clone()).unwrap();
    let render = match &module.body[0] {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => RenderSource::Function {
            render: function,
            component_options: None,
        },
        _ => panic!("expected render function"),
    };
    let ctx = VueRecoveryContext {
        setup_local_bindings: vec![test_local_binding(
            "const message = value;",
            &["message"],
            &["message"],
            &["value"],
        )],
        cm,
        ..Default::default()
    };
    let mut root = VueNode::Interpolation(VueExpr::new("message"));

    let plan = VueSetupScriptPlan::build(&ctx, &mut root, render).unwrap();

    assert!(!plan.is_empty());
    assert_eq!(plan.local_declarations.len(), 1);
    assert_eq!(plan.local_declarations[0].source, "const message = value;");
    assert_eq!(plan.scheduled_declarations.len(), 1);
    assert_eq!(
        plan.scheduled_declarations[0].bindings,
        test_atoms(&["message"])
    );
    assert_eq!(plan.render(&ctx), "const message = value;\n");
}

#[test]
fn template_usage_ignores_scoped_for_locals() {
    use crate::vue_template::{VueElement, VueFor};

    let root = VueNode::For(VueFor {
        value: "item".to_string(),
        source: VueExpr::new("items"),
        node: Box::new(VueNode::Element(
            VueElement::new("button")
                .with_attrs(vec![
                    VueAttr::Static {
                        name: "ref".to_string(),
                        value: Some("buttonRef".to_string()),
                    },
                    VueAttr::On {
                        name: "click".to_string(),
                        expr: VueExpr::new("select(item, selected)"),
                        modifiers: Vec::new(),
                    },
                    VueAttr::Bind {
                        name: "title".to_string(),
                        expr: VueExpr::new("item.label || fallback"),
                    },
                ])
                .with_children(vec![VueNode::Interpolation(VueExpr::new(
                    "item.name + suffix",
                ))]),
        )),
        scope: VueTemplateScope::from_local("item"),
    });

    let usage = VueTemplateUsage::new(&root);

    assert_eq!(usage.static_ref_names, vec!["buttonRef"]);
    assert_eq!(usage.for_source_refs, test_atom_set(&["items"]));
    assert!(usage.expr_refs.contains(&Atom::from("items")));
    assert!(usage.expr_refs.contains(&Atom::from("select")));
    assert!(usage.expr_refs.contains(&Atom::from("selected")));
    assert!(usage.expr_refs.contains(&Atom::from("fallback")));
    assert!(usage.expr_refs.contains(&Atom::from("suffix")));
    assert!(!usage.expr_refs.contains(&Atom::from("item")));
    assert!(usage.event_refs.contains(&Atom::from("select")));
    assert!(usage.event_refs.contains(&Atom::from("selected")));
    assert!(!usage.event_refs.contains(&Atom::from("item")));
    assert!(!usage.read_refs.contains(&Atom::from("item")));
}

#[test]
fn template_usage_applies_slot_scope_to_children() {
    use crate::vue_template::{VueDirective, VueElement};

    let root = VueNode::Element(
        VueElement::new("template")
            .with_attrs(vec![VueAttr::Directive(
                VueDirective::new("slot")
                    .with_dynamic_arg("slotName")
                    .with_scope(VueTemplateScope::from_local("slotProps")),
            )])
            .with_children(vec![
                VueNode::Interpolation(VueExpr::new("slotProps.title + outer")),
                VueNode::Element(VueElement::new("button").with_attrs(vec![VueAttr::On {
                    name: "click".to_string(),
                    expr: VueExpr::new("select(slotProps, outer)"),
                    modifiers: Vec::new(),
                }])),
            ]),
    );

    let usage = VueTemplateUsage::new(&root);

    assert!(usage.expr_refs.contains(&Atom::from("slotName")));
    assert!(usage.expr_refs.contains(&Atom::from("outer")));
    assert!(usage.expr_refs.contains(&Atom::from("select")));
    assert!(!usage.expr_refs.contains(&Atom::from("slotProps")));
    assert!(usage.event_refs.contains(&Atom::from("select")));
    assert!(usage.event_refs.contains(&Atom::from("outer")));
    assert!(!usage.event_refs.contains(&Atom::from("slotProps")));
    assert!(!usage.read_refs.contains(&Atom::from("slotProps")));
}

#[test]
fn imports_inlined_computed_script_setup_dependencies() {
    let input = r#"
import { sections } from "./sections.js";
import { useViewState } from "./state.js";
import { defineComponent, computed, openBlock, createElementBlock, Fragment, renderList, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const { page } = useViewState();
    const labels = computed(() => ({
      [sections.Home]: {
        title: page.name
      }
    }));
    const links = computed(() => {
      const list = page.meta.steps ?? [];
      return list.map((name, index) => ({
        title: labels.value[name]?.title ?? "",
        enabled: index < list.length - 1
      }));
    });
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(links.value, (item) => (
          openBlock(), createElementBlock("li", { key: item.title }, toDisplayString(item.title), 1)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { sections } from \"./sections.js\";\nimport { useViewState } from \"./state.js\";\n\nconst { page } = useViewState();\n\nconst links = computed(()=>{\n    const list = page.meta.steps ?? [];\n    return list.map((name, index)=>({\n            title: (({\n    [sections.Home]: {\n        title: page.name\n    }\n}))[name]?.title ?? \"\",\n            enabled: index < list.length - 1\n        }));\n});\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in links\" :key=\"item.title\">{{ item.title }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn imports_template_expression_refs_into_script_setup() {
    let input = r#"
import { formatStatus } from "./status.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    return () => (
      openBlock(), createElementBlock("span", { title: formatStatus("ok") }, "Ok", 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { formatStatus } from \"./status.js\";\n</script>\n\n<template>\n  <span :title='formatStatus(\"ok\")'>Ok</span>\n</template>\n"
        );
}

#[test]
fn imports_template_helpers_and_component_tags() {
    let input = r#"
import { S as StatusTag } from "./StatusTag.vue";
import { statusLevel } from "./status.js";
import { defineComponent, openBlock, createVNode } from "vue";
export default defineComponent({
  props: {
    status: String,
  },
  setup(props) {
    return () => (
      openBlock(), createVNode(StatusTag, { level: statusLevel(props.status) }, null, 8, ["level"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as StatusTag } from \"./StatusTag.vue\";\nimport { statusLevel } from \"./status.js\";\n\nconst props = defineProps({\n    status: String\n});\nconst { status } = props;\n</script>\n\n<template>\n  <StatusTag :level=\"statusLevel(status)\" />\n</template>\n"
        );
}

#[test]
fn uses_readable_define_props_binding_for_minified_setup_param() {
    let input = r#"
import { formatMsg } from "./format.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    msg: String,
  },
  setup(e) {
    return () => (
      openBlock(), createElementBlock("div", { title: formatMsg(e.msg) }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { formatMsg } from \"./format.js\";\n\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n</script>\n\n<template>\n  <div :title=\"formatMsg(msg)\" />\n</template>\n"
        );
}

#[test]
fn rewrites_whole_setup_props_param_in_selected_local() {
    let input = r#"
import { useState } from "./state.js";
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    msg: String,
  },
  setup(e) {
    const state = useState(e);
    return () => (
      openBlock(), createElementBlock("span", null, toDisplayString(state.msg), 1)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { useState } from \"./state.js\";\n\nconst props = defineProps({\n    msg: String\n});\nconst { msg } = props;\n\nconst state = useState(props);\n</script>\n\n<template>\n  <span>{{ state.msg }}</span>\n</template>\n"
        );
}

#[test]
fn avoids_props_binding_when_props_is_a_prop_name() {
    let input = r#"
import { formatMsg } from "./format.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    props: String,
  },
  setup(e) {
    return () => (
      openBlock(), createElementBlock("div", { title: formatMsg(e.props) }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { formatMsg } from \"./format.js\";\n\nconst e = defineProps({\n    props: String\n});\nconst { props } = e;\n</script>\n\n<template>\n  <div :title=\"formatMsg(props)\" />\n</template>\n"
        );
}

#[test]
fn does_not_import_template_arrow_params() {
    let input = r#"
import { item } from "./format.js";
import { next } from "./format.js";
import { total } from "./format.js";
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  props: {
    list: Array,
  },
  setup(props) {
    return () => (
      openBlock(), createElementBlock("span", {
        title: props.list.reduce((total, item) => {
          const next = item.count;
          return total + next;
        }, 0)
      }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    list: Array\n});\nconst { list } = props;\n</script>\n\n<template>\n  <span :title=\"list.reduce((total, item)=>{ const next = item.count; return total + next; }, 0)\" />\n</template>\n"
        );
}

#[test]
fn template_arrow_param_does_not_hide_setup_local_elsewhere() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock, createElementVNode, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const list = useList();
    const item = useSelectedItem();
    return () => (
      openBlock(), createElementBlock("section", {
        title: list.map(item => item.name).join(",")
      }, [
        createElementVNode("p", null, toDisplayString(item.label), 1)
      ], 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst list = useList();\nconst item = useSelectedItem();\n</script>\n\n<template>\n  <section :title='list.map((item)=>item.name).join(\",\")'>\n    <p>{{ item.label }}</p>\n  </section>\n</template>\n"
        );
}

#[test]
fn does_not_import_identifiers_used_only_as_props_or_properties() {
    let input = r#"
import { padding } from "./format.js";
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
const _sfc_main = defineComponent({
  props: {
    padding: String,
  },
  setup(props) {
    const style = computed(() => {
      const result = {};
      if (props.padding) {
        result.padding = props.padding;
      }
      return result;
    });
    return () => (
      openBlock(), createElementBlock("div", { style: style.value }, null, 4)
    );
  }
});
export default _sfc_main;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\n\nconst props = defineProps({\n    padding: String\n});\nconst { padding } = props;\n\nconst style = computed(()=>{\n    const result = {};\n    if (padding) {\n        result.padding = padding;\n    }\n    return result;\n});\n</script>\n\n<template>\n  <div :style=\"style\" />\n</template>\n"
        );
}

#[test]
fn does_not_import_member_property_names() {
    let input = r#"
import { i, t } from "./format.js";
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    return () => (
      openBlock(), createElementBlock("span", null, toDisplayString(i.t("hello")), 1)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { i } from \"./format.js\";\n</script>\n\n<template>\n  <span>{{ i.t(\"hello\") }}</span>\n</template>\n"
        );
}

#[test]
fn emits_script_setup_refs_used_by_template() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock, createElementVNode, normalizeStyle } from "vue";
export default defineComponent({
  props: {
    show: { type: Boolean, default: false },
  },
  setup(props) {
    const innerRef = ref(null);
    const height = ref(0);
    return () => (
      openBlock(), createElementBlock("section", {
        style: normalizeStyle({ height: props.show ? `${height.value}px` : 0 })
      }, [
        createElementVNode("div", { ref_key: "innerRef", ref: innerRef }, null, 512)
      ], 4)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst props = defineProps({\n    show: {\n        type: Boolean,\n        default: false\n    }\n});\nconst { show } = props;\n\nconst height = ref(0);\nconst innerRef = ref(null);\n</script>\n\n<template>\n  <section :style=\"{ height: show ? `${height}px` : 0 }\">\n    <div ref=\"innerRef\" />\n  </section>\n</template>\n"
        );
}

#[test]
fn emits_define_emits_for_setup_emit_alias() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  emits: ["click"],
  setup(props, { emit }) {
    const send = emit;
    return () => (
      openBlock(), createElementBlock("button", { onClick: () => send("click") }, "More", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst send = defineEmits([\n    \"click\"\n]);\n</script>\n\n<template>\n  <button @click='send(\"click\")'>More</button>\n</template>\n"
        );
}

#[test]
fn emits_define_emits_for_direct_setup_emit() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  emits: ["click"],
  setup(props, { emit }) {
    return () => (
      openBlock(), createElementBlock("button", { onClick: () => emit("click") }, "More", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst emit = defineEmits([\n    \"click\"\n]);\n</script>\n\n<template>\n  <button @click='emit(\"click\")'>More</button>\n</template>\n"
        );
}

#[test]
fn does_not_emit_define_emits_for_unused_setup_emit() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  emits: ["click"],
  setup(props, { emit }) {
    const count = ref(0);
    return () => (
      openBlock(), createElementBlock("button", { title: count.value }, "More", 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button :title=\"count\">More</button>\n</template>\n"
        );
}

#[test]
fn keeps_setup_ref_when_nested_local_reuses_its_name() {
    // A nested arrow param `count` reads `count.text` (a non-`.value` member).
    // Under resolver that param carries a different SyntaxContext than the setup
    // ref `count`, so it must not be mistaken for a non-value member access on
    // the ref. Ref classification is keyed on (name, ctxt), not name alone; if
    // shadow safety regressed, the outer `count` would stop being emitted as
    // `ref(0)`.
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup(props) {
    const count = ref(0);
    const format = (count) => count.text;
    return () => (
      openBlock(), createElementBlock("button", { title: count.value }, "More", 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button :title=\"count\">More</button>\n</template>\n"
    );
}

#[test]
fn does_not_emit_ref_for_candidate_without_value_usage() {
    let input = r#"
import { d as dc, x as useSlots, _ as unref, q as ob, X as ce } from "./vendor-vue.js";
export const _ = dc({
  __name: "SlotsPanel",
  setup() {
    const slots = useSlots();
    return () => (
      ob(), ce("div", { title: unref(slots).All }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div :title=\"slots.All\" />\n</template>\n"
    );
}

#[test]
fn emits_opaque_helper_object_used_by_script_handler() {
    let input = r#"
import { d as dc, Q as useRouter, q as ob, X as ce } from "./vendor-vue.js";
import { sections } from "./sections.js";
export const _ = dc({
  __name: "ErrorPanel",
  setup() {
    const router = useRouter();
    function backToHome() {
      router.push({ name: sections.Home });
    }
    return () => (
      ob(), ce("button", { onClick: backToHome }, "Back", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { Q as useRouter } from \"./vendor-vue.js\";\nimport { sections } from \"./sections.js\";\n\nconst router = useRouter();\nfunction backToHome() {\n    router.push({\n        name: sections.Home\n    });\n}\n</script>\n\n<template>\n  <button @click=\"backToHome\">Back</button>\n</template>\n"
        );
}

#[test]
fn preserves_callable_vendor_helper_candidate_used_by_event() {
    let input = r#"
import { d as dc, _ as ur, h as debounce, q as ob, X as ce } from "./vendor-vue.js";
import { submit } from "./api.js";
export const _ = dc({
  __name: "SubmitButton",
  setup() {
    const send = debounce(submit, 1000);
    const payload = { kind: "save" };
    return () => (
      ob(), ce("button", {
        onClick: () => ur(send)(payload)
      }, "Save", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { h as debounce } from \"./vendor-vue.js\";\nimport { submit } from \"./api.js\";\n\nconst send = debounce(submit, 1000);\nconst payload = {\n    kind: \"save\"\n};\n</script>\n\n<template>\n  <button @click=\"send(payload)\">Save</button>\n</template>\n"
        );
}

#[test]
fn emits_module_local_helpers_used_by_setup_declarations() {
    let input = r#"
import { d as dc, r, c as cp, q as ob, X as ce } from "./vendor-vue.js";
import { n as normalize } from "./format.js";
const decorate = (item) => normalize(item.name);
function useItems(kind) {
  return {
    items: r([decorate(kind.value)]),
    loaded: r(true)
  };
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    const kind = { value: "soccer" };
    const r = [","];
    const { items, loaded } = useItems(kind);
    const label = cp(() => {
      const names = [];
      items.value.forEach((item) => names.push(item.name));
      return names.join(r[0]);
    });
    return () => (
      ob(), ce("p", { title: label.value }, loaded.value ? "Ready" : "Wait", 9, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { n as normalize } from \"./format.js\";\nimport { r as r_1 } from \"./vendor-vue.js\";\n\nconst decorate = (item)=>normalize(item.name);\nfunction useItems(kind) {\n    return {\n        items: r_1([\n            decorate(kind.value)\n        ]),\n        loaded: r_1(true)\n    };\n}\nconst kind = {\n    value: \"soccer\"\n};\nconst r = [\n    \",\"\n];\nconst { items, loaded } = useItems(kind);\n\nconst label = computed(()=>{\n    const names = [];\n    items.value.forEach((item)=>names.push(item.name));\n    return names.join(r[0]);\n});\n</script>\n\n<template>\n  <p :title=\"label\">\n    <template v-if=\"loaded.value\">\n      Ready\n    </template>\n    <template v-else>\n      Wait\n    </template>\n  </p>\n</template>\n"
        );
}

#[test]
fn aliases_module_local_helper_when_setup_local_collides() {
    let input = r#"
import { d as dc, r as rf, c as cp, q as ob, X as ce } from "./vendor-vue.js";
import { n as normalize } from "./format.js";
const r = (item) => normalize(item.name);
function useItems(kind) {
  return {
    items: rf([r(kind.value)]),
    loaded: rf(true)
  };
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    const kind = { value: "soccer" };
    const r = [","];
    const { items, loaded } = useItems(kind);
    const label = cp(() => {
      const names = [];
      items.value.forEach((item) => names.push(r[0] + item));
      return names.join("");
    });
    return () => (
      ob(), ce("p", { title: label.value }, loaded.value ? "Ready" : "Wait", 9, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { n as normalize } from \"./format.js\";\nimport { r as rf } from \"./vendor-vue.js\";\n\nconst r_1 = (item)=>normalize(item.name);\nfunction useItems(kind) {\n    return {\n        items: rf([\n            r_1(kind.value)\n        ]),\n        loaded: rf(true)\n    };\n}\nconst kind = {\n    value: \"soccer\"\n};\nconst r = [\n    \",\"\n];\nconst { items, loaded } = useItems(kind);\n\nconst label = computed(()=>{\n    const names = [];\n    items.value.forEach((item)=>names.push(r[0] + item));\n    return names.join(\"\");\n});\n</script>\n\n<template>\n  <p :title=\"label\">\n    <template v-if=\"loaded.value\">\n      Ready\n    </template>\n    <template v-else>\n      Wait\n    </template>\n  </p>\n</template>\n"
        );
}

#[test]
fn does_not_rewrite_setup_local_refs_to_module_aliases() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
const source = () => "module";
function useItems() {
  return source();
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    const source = { value: "setup" };
    function onClick() {
      return source.value + useItems();
    }
    return () => (
      ob(), ce("button", { title: source.value, onClick: onClick }, "Ready", 8, ["title", "onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst source_1 = ()=>\"module\";\nfunction useItems() {\n    return source_1();\n}\nconst source = {\n    value: \"setup\"\n};\nfunction onClick() {\n    return source.value + useItems();\n}\n</script>\n\n<template>\n  <button :title=\"source.value\" @click=\"onClick\">Ready</button>\n</template>\n"
        );
}

#[test]
fn omits_later_duplicate_module_local_candidates() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
function r(step) {
  return step();
}
var r = document.createElement("style");
function useItems() {
  return r(() => "ready");
}
export const _ = dc({
  __name: "ItemsPanel",
  setup() {
    function onClick() {
      return useItems();
    }
    return () => (
      ob(), ce("button", { onClick: onClick }, "Ready", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nfunction r(step) {\n    return step();\n}\nfunction useItems() {\n    return r(()=>\"ready\");\n}\nfunction onClick() {\n    return useItems();\n}\n</script>\n\n<template>\n  <button @click=\"onClick\">Ready</button>\n</template>\n"
        );
}

#[test]
fn omits_transpiler_runtime_helpers_from_module_dependencies() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
function runtime() {
  const start = "suspendedStart";
  const iterator = "@@iterator";
  function invoke() {
    return "_invoke";
  }
  return { start, iterator, invoke };
}
function useLabel() {
  return runtime().invoke();
}
export const _ = dc({
  setup() {
    function onClick() {
      return useLabel();
    }
    return () => (
      ob(), ce("button", { onClick: onClick }, "Ready", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nfunction useLabel() {\n    return runtime().invoke();\n}\nfunction onClick() {\n    return useLabel();\n}\n</script>\n\n<template>\n  <button @click=\"onClick\">Ready</button>\n</template>\n"
        );
}

#[test]
fn emits_candidate_ref_used_by_inlined_setup_computed() {
    let input = r#"
import { d as dc, r as rf, c as cp, q as ob, X as ce } from "./vendor-vue.js";
export const _ = dc({
  __name: "HeightPanel",
  setup() {
    const height = rf(0);
    const style = cp(() => ({ height: `${height.value}px` }));
    return () => (
      ob(), ce("div", { title: style.value }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst height = ref(0);\n</script>\n\n<template>\n  <div :title=\"{ height: `${height}px` }\" />\n</template>\n"
        );
}

#[test]
fn preserves_computed_block_local_shadowing() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ShadowedLocal",
  setup() {
    const label = computed(() => {
      const values = items.value;
      return values.map((values) => values.value).join(",");
    });
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <p :title='items.value.map((values)=>values.value).join(\",\")' />\n</template>\n"
        );
}

#[test]
fn recovers_setup_ref_value_alias() {
    let input = r#"
import { defineComponent, ref, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "Counter",
  setup() {
    const count = ref(0);
    return () => (
      openBlock(), createElementBlock("button", { title: count.value }, toDisplayString(count.value), 9, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button :title=\"count\">{{ count }}</button>\n</template>\n"
        );
}

#[test]
fn recovers_vite_setup_ref_value_alias() {
    let input = r#"
import { d as dc, r as rf, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "Viewport",
  setup() {
    const height = rf(0);
    return () => (
      ob(), ce("div", { style: { height: `${height.value}px` } }, null, 4)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst height = ref(0);\n</script>\n\n<template>\n  <div :style=\"{ height: `${height}px` }\" />\n</template>\n"
        );
}

#[test]
fn preserves_shadowed_ref_value_member() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "ShadowedCounter",
  setup() {
    const count = ref(0);
    return () => (
      openBlock(), createElementBlock("div", { title: [count].map((count) => count.value).join(",") }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <div :title='[ count ].map((count)=>count.value).join(\",\")' />\n</template>\n"
        );
}

#[test]
fn recovers_store_to_refs_destructured_values() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
import { storeToRefs } from "pinia";
export default defineComponent({
  __name: "StoreStatus",
  setup() {
    const store = useStore();
    const { currentUser, isLoaded } = storeToRefs(store);
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, toDisplayString(isLoaded.value), 9, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { storeToRefs } from \"pinia\";\n\nconst store = useStore();\nconst { currentUser, isLoaded } = storeToRefs(store);\n</script>\n\n<template>\n  <p :title=\"currentUser.name\">{{ isLoaded }}</p>\n</template>\n"
        );
}

#[test]
fn recovers_vite_store_to_refs_destructured_values() {
    let input = r#"
import { d as dc, K as sr, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "StoreStatus",
  setup() {
    const { currentUser } = sr(useStore());
    const label = cp(() => currentUser.value.name);
    return () => (
      ob(), ce("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { K as sr } from \"./vendor-vue-C85wAS_L.js\";\n\nconst { currentUser } = sr(useStore());\n</script>\n\n<template>\n  <p :title=\"currentUser.name\" />\n</template>\n"
        );
}

#[test]
fn recovers_vite_store_to_refs_destructured_alias_values() {
    let input = r#"
import { d as dc, K as sr, c as cp, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "StoreStatus",
  setup() {
    const refs = sr(useStore());
    const { currentUser } = refs;
    const label = cp(() => currentUser.value.name);
    return () => (
      ob(), ce("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { K as sr } from \"./vendor-vue-C85wAS_L.js\";\n\nconst refs = sr(useStore());\nconst { currentUser } = refs;\n</script>\n\n<template>\n  <p :title=\"currentUser.name\" />\n</template>\n"
        );
}

#[test]
fn recovers_ref_object_member_extracted_values() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
import { storeToRefs } from "pinia";
export default defineComponent({
  __name: "StoreStatus",
  setup() {
    const currentUser = storeToRefs(useStore()).currentUser;
    const refs = storeToRefs(useOtherStore());
    const isLoaded = refs.isLoaded;
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, toDisplayString(isLoaded.value), 9, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { storeToRefs } from \"pinia\";\n\nconst currentUser = storeToRefs(useStore()).currentUser;\nconst refs = storeToRefs(useOtherStore());\nconst isLoaded = refs.isLoaded;\n</script>\n\n<template>\n  <p :title=\"currentUser.name\">{{ isLoaded }}</p>\n</template>\n"
        );
}

#[test]
fn emits_dependencies_for_inlined_setup_computed_values() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock, Fragment, renderList } from "vue";
import { storeToRefs } from "pinia";
export default defineComponent({
  setup() {
    const { items, selected } = storeToRefs(useStore());
    const visibleItems = computed(() => items.value.filter((item) => selected.value.includes(item.id)));
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(visibleItems.value, (item) => (
          openBlock(), createElementBlock("li", { key: item.id }, item.name, 1)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { storeToRefs } from \"pinia\";\n\nconst { items, selected } = storeToRefs(useStore());\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items.filter((item)=>selected.includes(item.id))\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn emits_alias_dependencies_for_inlined_setup_computed_values() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock, Fragment, renderList } from "vue";
import { a } from "./vendor-vue.js";
export default defineComponent({
  setup() {
    const refs = a(useStore());
    const { items } = refs;
    const visibleItems = computed(() => items.value.filter((item) => item.visible));
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(visibleItems.value, (item) => (
          openBlock(), createElementBlock("li", { key: item.id }, item.name, 1)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { a } from \"./vendor-vue.js\";\n\nconst refs = a(useStore());\nconst { items } = refs;\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items.filter((item)=>item.visible)\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn cleans_template_ref_alias_in_opaque_ref_object_dependency() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { c, r } from "./vendor-vue.js";
export default defineComponent({
  setup() {
    const D = r(null);
    const scroller = c(D, { offset: { left: 1 } });
    const { x } = scroller;
    const scroll = () => x.value;
    return () => (
      openBlock(), createElementBlock("div", { ref_key: "scrollContainer", ref: D, onClick: scroll }, null, 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\nimport { c } from \"./vendor-vue.js\";\n\nconst scrollContainer = ref(null);\n\nconst scroller = c(scrollContainer, {\n    offset: {\n        left: 1\n    }\n});\nconst { x } = scroller;\nconst scroll = ()=>x;\n</script>\n\n<template>\n  <div ref=\"scrollContainer\" @click=\"scroll\" />\n</template>\n"
        );
}

#[test]
fn preserves_plain_destructured_value_members() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
export default defineComponent({
  __name: "PlainValue",
  setup() {
    const { currentUser } = usePlainStore();
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
    );
}

#[test]
fn recovers_imported_composable_returned_ref_values() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
import { u as useViewState } from "./state.js";
export default defineComponent({
  __name: "UsesViewState",
  setup() {
    const { page, selectedKey, raw } = useViewState();
    const label = computed(() => {
      const parts = [];
      parts.push(page.name);
      parts.push(selectedKey.value);
      parts.push(raw.value);
      return parts.join(":");
    });
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;
    let state = r#"
function trackedValue(source) {
  const value = createRef();
  watch(source, (next) => {
    value.value = next;
  });
  return readonly(value);
}
const useViewState = () => {
  const page = usePage();
  const selectedKey = trackedValue(() => page.params.kind);
  const raw = { value: "plain" };
  return { page, selectedKey, raw };
};
export { useViewState as u };
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { u as useViewState } from \"./state.js\";\n\nconst { page, selectedKey, raw } = useViewState();\n\nconst label = computed(()=>{\n    const parts = [];\n    parts.push(page.name);\n    parts.push(selectedKey);\n    parts.push(raw.value);\n    return parts.join(\":\");\n});\n</script>\n\n<template>\n  <p :title=\"label\" />\n</template>\n"
        );
}

#[test]
fn recovers_imported_composable_member_ref_values() {
    let input = r#"
import { defineComponent, toDisplayString, openBlock, createElementBlock } from "vue";
import { u as useViewState } from "./state.js";
export default defineComponent({
  __name: "UsesViewState",
  setup() {
    const selectedKey = useViewState().selectedKey;
    return () => (
      openBlock(), createElementBlock("p", { title: selectedKey.value }, toDisplayString(selectedKey.value), 9, ["title"])
    );
  }
});
"#;
    let state = r#"
function trackedValue(source) {
  const value = createRef();
  watch(source, (next) => {
    value.value = next;
  });
  return readonly(value);
}
const useViewState = () => {
  const selectedKey = trackedValue(() => route.params.kind);
  const raw = { value: "plain" };
  return { selectedKey, raw };
};
export { useViewState as u };
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useViewState } from \"./state.js\";\n\nconst selectedKey = useViewState().selectedKey;\n</script>\n\n<template>\n  <p :title=\"selectedKey\">{{ selectedKey }}</p>\n</template>\n"
        );
}

#[test]
fn recovers_imported_composable_tuple_member_ref_values() {
    let input = r#"
import { defineComponent, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useStatus } from "./status.js";
export default defineComponent({
  __name: "UsesStatus",
  setup() {
    const selectedStatus = useStatus().selectedStatus;
    return () => (
      openBlock(), createElementBlock("div", { class: normalizeClass({ rise: selectedStatus.value === "rise" }) }, null, 2)
    );
  }
});
"#;
    let state = r#"
export const u = () => {
  const [status, setStatus] = useResetState("remain");
  if (status.value === "drop") {
    setStatus("remain");
  }
  return { selectedStatus: status };
};
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./status.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useStatus } from \"./status.js\";\n\nconst selectedStatus = useStatus().selectedStatus;\n</script>\n\n<template>\n  <div :class='{ rise: selectedStatus === \"rise\" }' />\n</template>\n"
        );
}

#[test]
fn recovers_imported_composable_written_ref_values() {
    let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
import { u as useListState } from "./state.js";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items, raw } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;
    let state = r#"
export const u = () => {
  const itemList = createList([]);
  itemList.value.push("ready");
  const raw = { value: { name: "plain" } };
  return { items: itemList, raw };
};
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { L as ListView } from \"./ListView.vue\";\nimport { u as useListState } from \"./state.js\";\n\nconst { items, raw } = useListState();\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
}

#[test]
fn recovers_imported_composable_callback_written_ref_values() {
    let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
import { u as useListState } from "./state.js";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items, raw } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;
    let state = r#"
export const u = () => {
  const itemList = createList([]);
  subscribe(() => {
    itemList.value.push("ready");
  });
  const raw = { value: { name: "plain" } };
  return { items: itemList, raw };
};
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { L as ListView } from \"./ListView.vue\";\nimport { u as useListState } from \"./state.js\";\n\nconst { items, raw } = useListState();\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
}

#[test]
fn recovers_imported_composable_legacy_tuple_member_ref_values() {
    let input = r#"
import { defineComponent, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useStatus } from "./status-legacy.js";
export default defineComponent({
  __name: "UsesStatus",
  setup() {
    const selectedStatus = useStatus().selectedStatus;
    return () => (
      openBlock(), createElementBlock("div", { class: normalizeClass({ rise: selectedStatus.value === "rise" }) }, null, 2)
    );
  }
});
"#;
    let state = r#"
System.register([], function (_export) {
  return {
    setters: [],
    execute: function () {
      _export("u", () => {
        const pair = _slicedToArray(useResetState("remain"), 2);
        const status = pair[0];
        const setStatus = pair[1];
        if (status.value === "drop") {
          setStatus("remain");
        }
        return { selectedStatus: status };
      });
    }
  };
});
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./status-legacy.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as useStatus } from \"./status-legacy.js\";\n\nconst selectedStatus = useStatus().selectedStatus;\n</script>\n\n<template>\n  <div :class='{ rise: selectedStatus === \"rise\" }' />\n</template>\n"
        );
}

#[test]
fn recovers_local_composable_written_ref_values() {
    let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
function useListState() {
  const itemList = createList([]);
  itemList.value.push("ready");
  const raw = { value: { name: "plain" } };
  return { items: itemList, raw };
}
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items, raw } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { L as ListView } from \"./ListView.vue\";\n\nfunction useListState() {\n    const itemList = createList([]);\n    itemList.value.push(\"ready\");\n    const raw = {\n        value: {\n            name: \"plain\"\n        }\n    };\n    return {\n        items: itemList,\n        raw\n    };\n}\nconst { items, raw } = useListState();\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
}

#[test]
fn recovers_iife_composable_result_ref_values() {
    let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const state = ((enabled) => {
      const itemList = createList([]);
      subscribe(() => {
        itemList.value.push("ready");
      });
      const raw = { value: { name: "plain" } };
      return { items: itemList, raw };
    })(true);
    const { items, raw } = state;
    return () => (
      openBlock(), createBlock(ListView, { items: items.value, title: raw.value.name }, null, 8, ["items", "title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { L as ListView } from \"./ListView.vue\";\n\nconst state = ((enabled)=>{\n    const itemList = createList([]);\n    subscribe(()=>{\n        itemList.value.push(\"ready\");\n    });\n    const raw = {\n        value: {\n            name: \"plain\"\n        }\n    };\n    return {\n        items: itemList,\n        raw\n    };\n})(true);\nconst { items, raw } = state;\n</script>\n\n<template>\n  <ListView :items=\"items\" :title=\"raw.value.name\" />\n</template>\n"
        );
}

#[test]
fn preserves_iife_composable_shadowed_callback_value_members() {
    let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const state = ((enabled) => {
      const itemList = createList([]);
      subscribe((itemList) => {
        itemList.value.push("nested");
      });
      return { items: itemList };
    })(true);
    const { items } = state;
    return () => (
      openBlock(), createBlock(ListView, { items: items.value.name }, null, 8, ["items"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { L as ListView } from \"./ListView.vue\";\n</script>\n\n<template>\n  <ListView :items=\"items.value.name\" />\n</template>\n"
        );
}

#[test]
fn preserves_imported_composable_shadowed_callback_value_members() {
    let input = r#"
import { defineComponent, openBlock, createBlock } from "vue";
import { L as ListView } from "./ListView.vue";
import { u as useListState } from "./state.js";
export default defineComponent({
  __name: "UsesListState",
  setup() {
    const { items } = useListState();
    return () => (
      openBlock(), createBlock(ListView, { items: items.value.name }, null, 8, ["items"])
    );
  }
});
"#;
    let state = r#"
export const u = () => {
  const itemList = createList([]);
  subscribe((itemList) => {
    itemList.value.push("nested");
  });
  return { items: itemList };
};
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { L as ListView } from \"./ListView.vue\";\n</script>\n\n<template>\n  <ListView :items=\"items.value.name\" />\n</template>\n"
        );
}

#[test]
fn preserves_imported_composable_member_plain_value_members() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as usePlainState } from "./state.js";
export default defineComponent({
  __name: "UsesPlainState",
  setup() {
    const currentUser = usePlainState().currentUser;
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, null, 8, ["title"])
    );
  }
});
"#;
    let state = r#"
const usePlainState = () => {
  const currentUser = { value: { name: "Ada" } };
  return { currentUser };
};
export { usePlainState as u };
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as usePlainState } from \"./state.js\";\n\nconst currentUser = usePlainState().currentUser;\n</script>\n\n<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
        );
}

#[test]
fn preserves_imported_composable_tuple_plain_value_members() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as usePlainState } from "./state.js";
export default defineComponent({
  __name: "UsesPlainState",
  setup() {
    const currentUser = usePlainState().currentUser;
    return () => (
      openBlock(), createElementBlock("p", { title: currentUser.value.name }, null, 8, ["title"])
    );
  }
});
"#;
    let state = r#"
export const u = () => {
  const [currentUser] = usePlainTuple();
  const label = currentUser.value.name;
  return { currentUser, label };
};
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { u as usePlainState } from \"./state.js\";\n\nconst currentUser = usePlainState().currentUser;\n</script>\n\n<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
        );
}

#[test]
fn preserves_imported_composable_returned_plain_value_members() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
import { u as usePlainState } from "./state.js";
export default defineComponent({
  __name: "UsesPlainState",
  setup() {
    const { currentUser } = usePlainState();
    const label = computed(() => currentUser.value.name);
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;
    let state = r#"
const usePlainState = () => {
  const currentUser = { value: { name: "Ada" } };
  return { currentUser };
};
export { usePlainState as u };
"#;

    assert_eq!(
        recover_source_with_imports(input, |source| {
            (source == "./state.js").then(|| state.to_string())
        })
        .unwrap()
        .unwrap(),
        "<template>\n  <p :title=\"currentUser.value.name\" />\n</template>\n"
    );
}

#[test]
fn recovers_imported_systemjs_composable_returned_ref_values() {
    let input = r#"
import { defineComponent, computed, openBlock, createElementBlock } from "vue";
import { u as useViewState } from "./state-legacy.js";
export default defineComponent({
  __name: "UsesLegacyViewState",
  setup() {
    const { page, selectedKey, raw } = useViewState();
    const label = computed(() => {
      const parts = [];
      parts.push(page.name);
      parts.push(selectedKey.value);
      parts.push(raw.value);
      return parts.join(":");
    });
    return () => (
      openBlock(), createElementBlock("p", { title: label.value }, null, 8, ["title"])
    );
  }
});
"#;
    let state = r#"
System.register(["./vendor-vue.js"], function (_export) {
  var ref, watch, readonly;
  return {
    setters: [
      function (module) {
        ref = module.B;
        watch = module.w;
        readonly = module.aB;
      }
    ],
    execute: function () {
      function trackedValue(source) {
        const value = ref();
        watch(source, (next) => {
          value.value = next;
        });
        return readonly(value);
      }
      _export("u", () => {
        const page = usePage();
        const selectedKey = trackedValue(() => page.params.kind);
        const raw = { value: "plain" };
        return { page, selectedKey, raw };
      });
    }
  };
});
"#;

    assert_eq!(
            recover_source_with_imports(input, |source| {
                (source == "./state-legacy.js").then(|| state.to_string())
            })
            .unwrap()
            .unwrap(),
            "<script setup>\nimport { computed } from \"vue\";\nimport { u as useViewState } from \"./state-legacy.js\";\n\nconst { page, selectedKey, raw } = useViewState();\n\nconst label = computed(()=>{\n    const parts = [];\n    parts.push(page.name);\n    parts.push(selectedKey);\n    parts.push(raw.value);\n    return parts.join(\":\");\n});\n</script>\n\n<template>\n  <p :title=\"label\" />\n</template>\n"
        );
}

#[test]
fn recovers_provider_returned_ref_values() {
    let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  const visibleItems = cp(() => items.value.filter((item) => item.enabled));
  const loaded = cp(() => ready.value);
  return { visibleItems, loaded };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { visibleItems, loaded } = state.provide();
    const hasItems = cp(() => visibleItems.value.length > 0);
    return () => (
      ob(), cb(SummaryPanel, { hasItems: hasItems.value, loaded: loaded.value }, null, 8, ["hasItems", "loaded"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as SummaryPanel } from \"./SummaryPanel.vue\";\n</script>\n\n<template>\n  <SummaryPanel :hasItems=\"visibleItems.length > 0\" :loaded=\"loaded\" />\n</template>\n"
        );
}

#[test]
fn emits_setup_dependencies_for_provider_computed_aliases() {
    let input = r#"
import { defineComponent, computed, ref, openBlock, createElementBlock, createVNode, createCommentVNode, Fragment } from "vue";
import { P as ListPanel } from "./ListPanel.vue";
import { I as ItemPicker } from "./ItemPicker.vue";
const state = createProvider("State", () => {
  const items = computed(() => source.value);
  const loaded = computed(() => ready.value);
  return { items, loaded };
});
function prepare(filters) {
  return { isOpen: ref(false), setIsOpen(value) {} };
}
export default defineComponent({
  __name: "UsesStateBlock",
  setup() {
    const { items, loaded } = state.provide();
    const visibleItems = computed(() => items.value.filter((item) => item.enabled));
    const itemFilters = computed(() => {
      const mapped = items.value.map((item) => ({ id: item.id, name: item.name, size: item.size }));
      return uniqueBy(mapped, (item) => item.id);
    });
    const { isOpen, setIsOpen } = prepare(itemFilters);
    const isSticky = true;
    return (_ctx, _cache) => (
      openBlock(), createElementBlock(Fragment, null, [
        visibleItems.value.length > 0 ? (openBlock(), createVNode(ListPanel, { active: true, isSticky }, null, 8, ["isSticky"])) : createCommentVNode("", true),
        createVNode(ItemPicker, { itemFilters: itemFilters.value, loaded: loaded.value, onClose: _cache[0] || (_cache[0] = (event) => setIsOpen(false)) }, null, 8, ["itemFilters", "loaded", "onClose"])
      ], 64)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { computed, ref } from \"vue\";\nimport { I as ItemPicker } from \"./ItemPicker.vue\";\nimport { P as ListPanel } from \"./ListPanel.vue\";\n\nconst state = createProvider(\"State\", ()=>{\n    const items = computed(()=>source.value);\n    const loaded = computed(()=>ready.value);\n    return {\n        items,\n        loaded\n    };\n});\nfunction prepare(filters) {\n    return {\n        isOpen: ref(false),\n        setIsOpen (value) {}\n    };\n}\nconst { items, loaded } = state.provide();\n\nconst itemFilters = computed(()=>{\n    const mapped = items.map((item)=>({\n            id: item.id,\n            name: item.name,\n            size: item.size\n        }));\n    return uniqueBy(mapped, (item)=>item.id);\n});\n\nconst { isOpen, setIsOpen } = prepare(itemFilters);\nconst isSticky = true;\n</script>\n\n<template>\n  <ListPanel v-if=\"(items.filter((item)=>item.enabled)).length > 0\" active :isSticky=\"isSticky\" />\n  <ItemPicker :itemFilters=\"itemFilters\" :loaded=\"loaded\" @close=\"setIsOpen(false)\" />\n</template>\n"
        );
}

#[test]
fn recovers_provider_returned_ref_alias_values() {
    let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  const loaded_1 = cp(() => ready.value);
  return { loaded: loaded_1 };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { loaded: isLoaded } = state.provide();
    return () => (
      ob(), cb(SummaryPanel, { loaded: isLoaded.value }, null, 8, ["loaded"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as SummaryPanel } from \"./SummaryPanel.vue\";\n</script>\n\n<template>\n  <SummaryPanel :loaded=\"isLoaded\" />\n</template>\n"
        );
}

#[test]
fn recovers_provider_returned_direct_ref_values() {
    let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  return { visibleItems: cp(() => items.value) };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { visibleItems } = state.provide();
    return () => (
      ob(), cb(SummaryPanel, { hasItems: visibleItems.value.length > 0 }, null, 8, ["hasItems"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as SummaryPanel } from \"./SummaryPanel.vue\";\n</script>\n\n<template>\n  <SummaryPanel :hasItems=\"visibleItems.length > 0\" />\n</template>\n"
        );
}

#[test]
fn recovers_provider_result_alias_ref_values() {
    let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  return { visibleItems: cp(() => items.value) };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const provided = state.provide();
    const { visibleItems } = provided;
    const hasItems = cp(() => visibleItems.value.length > 0);
    return () => (
      ob(), cb(SummaryPanel, { hasItems: hasItems.value }, null, 8, ["hasItems"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as SummaryPanel } from \"./SummaryPanel.vue\";\n</script>\n\n<template>\n  <SummaryPanel :hasItems=\"visibleItems.length > 0\" />\n</template>\n"
        );
}

#[test]
fn recovers_provider_injected_ref_values() {
    let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as SummaryPanel } from "./SummaryPanel.vue";
const state = createProvider("State", () => {
  return { items: cp(() => loadedItems.value) };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const injected = state.inject();
    const { items } = injected;
    return () => (
      ob(), cb(SummaryPanel, { count: items.value.length }, null, 8, ["count"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as SummaryPanel } from \"./SummaryPanel.vue\";\n</script>\n\n<template>\n  <SummaryPanel :count=\"items.length\" />\n</template>\n"
        );
}

#[test]
fn preserves_provider_returned_plain_value_members() {
    let input = r#"
import { d as dc, q as ob, X as ce } from "./vendor-vue.js";
const state = createProvider("State", () => {
  const value = { value: 1 };
  return { value };
});
export const _ = dc({
  __name: "UsesState",
  setup() {
    const { value } = state.provide();
    return () => (
      ob(), ce("p", { title: value.value }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p :title=\"value.value\" />\n</template>\n"
    );
}

#[test]
fn recovers_computed_if_return_chain() {
    let input = r#"
import { d as dc, c as cp, q as ob, aa as cb } from "./vendor-vue.js";
import { S as StatusTag } from "./StatusTag.vue";
export const _ = dc({
  __name: "BetStatusTag",
  setup(props) {
    const level = cp(() => {
      if (props.status === 1) {
        return "danger";
      }
      if (props.status === 2) {
        return "warning";
      }
      return "info";
    });
    return () => (ob(), cb(StatusTag, { level: level.value }, null, 8, ["level"]));
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { S as StatusTag } from \"./StatusTag.vue\";\n</script>\n\n<template>\n  <StatusTag :level='status === 1 ? \"danger\" : status === 2 ? \"warning\" : \"info\"' />\n</template>\n"
        );
}

#[test]
fn ignores_setup_render_like_code_without_vue_import_signal() {
    let input = r#"
import { x as element } from "./render-helpers.js";
export default {
  setup() {
    return () => element("h1", null, "Not Vue");
  }
};
"#;

    assert!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn recovers_class_binding_and_event_handler() {
    let input = r#"
import { toDisplayString, normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("button", {
    class: normalizeClass({ active: props.active }),
    onClick: increment
  }, toDisplayString(props.count), 3);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <button :class=\"{ active: props.active }\" @click=\"increment\">{{ props.count }}</button>\n</template>\n"
        );
}

#[test]
fn recovers_shorthand_class_object_entries() {
    let input = r#"
import { normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", {
    class: normalizeClass(["panel", { "active": active, "panel-ready": ready, expanded: expanded }])
  }, null, 2);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section class=\"panel\" :class='{ active, \"panel-ready\": ready, expanded }' />\n</template>\n"
        );
}

#[test]
fn recovers_empty_string_class_ternaries() {
    let input = r#"
import { normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", {
    class: normalizeClass([
      "panel",
      active ? "is-active" : ""
    ])
  }, [
    createElementBlock("span", {
      class: normalizeClass(tone ? `tone-${tone}` : "")
    }, null, 2),
    createElementBlock("strong", {
      class: normalizeClass(iconAlign === "top" ? "iconUpper" : iconAlign === "bottom" ? "iconLower" : "")
    }, null, 2)
  ], 2);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section class=\"panel\" :class='active &amp;&amp; \"is-active\"'>\n    <span :class=\"tone &amp;&amp; `tone-${tone}`\" />\n    <strong :class='iconAlign === \"top\" ? \"iconUpper\" : iconAlign === \"bottom\" &amp;&amp; \"iconLower\"' />\n  </section>\n</template>\n"
        );
}

#[test]
fn coalesces_multiple_dynamic_class_array_entries() {
    let input = r#"
import { normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", {
    class: normalizeClass([
      "panel",
      active ? "is-active" : "",
      { disabled: disabled },
      tone ? `tone-${tone}` : ""
    ])
  }, null, 2);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section class=\"panel\" :class='[ active &amp;&amp; \"is-active\", { disabled }, tone &amp;&amp; `tone-${tone}` ]' />\n</template>\n"
        );
}

#[test]
fn recovers_shorthand_event_handler() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", { onClick }, "Go", 8, ["onClick"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click=\"onClick\">Go</button>\n</template>\n"
    );
}

#[test]
fn keeps_lowercase_on_prefixed_props_as_bindings() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", { once: _ctx.once }, "Run", 8, ["once"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button :once=\"once\">Run</button>\n</template>\n"
    );
}

#[test]
fn recovers_component_shorthand_event_handler() {
    let input = r#"
import { B as Badge } from "./Badge.vue";
import { openBlock, createVNode } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createVNode(Badge, { onClick }, null, 8, ["onClick"]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { B as Badge } from \"./Badge.vue\";\n</script>\n\n<template>\n  <Badge @click=\"onClick\" />\n</template>\n"
        );
}

#[test]
fn recovers_component_camel_event_names_as_kebab() {
    let input = r#"
import { openBlock, createVNode } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createVNode(ContestCard, {
    contest: item,
    onContestEnded
  }, null, 8, ["contest", "onContestEnded"]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <ContestCard :contest=\"item\" @contest-ended=\"onContestEnded\" />\n</template>\n"
        );
}

#[test]
fn preserves_on_prefixed_component_event_names() {
    let input = r#"
import { openBlock, createVNode } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createVNode(ContestPoolHeader, {
    onOnBack
  }, null, 8, ["onOnBack"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <ContestPoolHeader @onBack=\"onOnBack\" />\n</template>\n"
    );
}

#[test]
fn preserves_component_update_event_names() {
    let input = r#"
import { openBlock, createVNode } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createVNode(FormInput, {
    "onUpdate:modelValue": onUpdate
  }, null, 8, ["onUpdate:modelValue"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <FormInput @update:modelValue=\"onUpdate\" />\n</template>\n"
    );
}

#[test]
fn recovers_vnode_lifecycle_event_names() {
    let input = r#"
import { resolveDynamicComponent, openBlock, createBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createBlock(resolveDynamicComponent(_ctx.component), {
    onVnodeMounted: track,
    onVnodeUpdated: track,
    onVnodeUnmounted: track
  }, null, 40, ["onVnodeMounted", "onVnodeUpdated", "onVnodeUnmounted"]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <component :is=\"component\" @vue:mounted=\"track\" @vue:updated=\"track\" @vue:unmounted=\"track\" />\n</template>\n"
        );
}

#[test]
fn recovers_template_ref_key_attrs() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", {
    ref_key: "innerRef",
    ref: innerRef
  }, null, 512);
}
__sfc__.render = render;
export default __sfc__;
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst innerRef = ref(null);\n</script>\n\n<template>\n  <div ref=\"innerRef\" />\n</template>\n"
        );
}

#[test]
fn omits_generated_numeric_if_branch_keys() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.ok
    ? createElementBlock("p", { key: 0 }, "Ready")
    : createElementBlock("span", { key: 1 }, "Waiting");
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p v-if=\"ok\">Ready</p>\n  <span v-else>Waiting</span>\n</template>\n"
    );
}

#[test]
fn preserves_non_numeric_if_branch_keys() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.ok
    ? createElementBlock("p", { key: _ctx.item.id }, "Ready", 8, ["key"])
    : createElementBlock("span", { key: "fallback" }, "Waiting");
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <p v-if=\"ok\" :key=\"item.id\">Ready</p>\n  <span v-else key=\"fallback\">Waiting</span>\n</template>\n"
        );
}

#[test]
fn preserves_empty_if_branch_keys() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.ok
    ? createElementBlock("p", { key: "" }, "Ready")
    : createElementBlock("span", { key: 1 }, "Waiting");
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p v-if=\"ok\" key>Ready</p>\n  <span v-else>Waiting</span>\n</template>\n"
    );
}

#[test]
fn omits_template_ref_for_attrs() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", {
    ref_for: true,
    ref: setItemRef
  }, null, 512);
}
__sfc__.render = render;
export default __sfc__;
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div :ref=\"setItemRef\" />\n</template>\n"
    );
}

#[test]
fn recovers_html_and_text_directive_props() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    createElementBlock("span", { innerHTML: _ctx.message }, null, 8, ["innerHTML"]),
    createElementBlock("p", { textContent: _ctx.label }, null, 8, ["textContent"])
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <span v-html=\"message\" />\n    <p v-text=\"label\" />\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_static_vnode_html() {
    let input = r#"
import { createStaticVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    createStaticVNode('<svg viewBox="0 0 10 10"><path d="M0 0h10v10H0z"></path></svg>', 1)
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <svg viewBox=\"0 0 10 10\"><path d=\"M0 0h10v10H0z\"></path></svg>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_with_memo_directive() {
    let input = r#"
import { withMemo, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return withMemo([_ctx.stakeDisplay, () => _ctx.i18n.locale], () => (
    openBlock(), createElementBlock("input", { value: _ctx.stakeDisplay }, null, 8, ["value"])
  ), _cache, 0);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <input :value=\"stakeDisplay\" v-memo=\"[ stakeDisplay, ()=>i18n.locale ]\" />\n</template>\n"
        );
}

#[test]
fn recovers_event_handler_modifiers() {
    let input = r#"
import { withKeys, withModifiers, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("input", {
    onKeyup: withKeys(withModifiers(_cache[0] || (_cache[0] = (...args) => (_ctx.submit && _ctx.submit(...args))), ["stop", "prevent"]), ["enter"])
  }, null, 40));
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <input @keyup.enter.stop.prevent=\"submit\" />\n</template>\n"
    );
}

#[test]
fn recovers_cached_event_modifier_handler() {
    let input = r#"
import { withModifiers, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("button", {
    onClick: _cache[0] || (_cache[0] = withModifiers(($event) => _ctx.close("ok"), ["self"]))
  }, "Close", 40, ["onClick"]));
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click.self='close(\"ok\")'>Close</button>\n</template>\n"
    );
}

#[test]
fn recovers_vite_cached_event_modifier_alias() {
    let input = r#"
import { q as ob, X as ce, aE as wm } from "./vendor-vue-C85wAS_L.js";
export function render(_ctx, _cache) {
  return ob(), ce("button", {
    onClick: _cache[0] || (_cache[0] = wm(($event) => _ctx.close("ok"), ["self"]))
  }, "Close", 40, ["onClick"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click.self='close(\"ok\")'>Close</button>\n</template>\n"
    );
}

#[test]
fn recovers_cached_event_modifier_noop_handler() {
    let input = r#"
import { q as ob, X as ce, aE as wm } from "./vendor-vue-C85wAS_L.js";
export function render(_ctx, _cache) {
  return ob(), ce("button", {
    onClick: _cache[0] || (_cache[0] = wm(() => {}, ["stop"]))
  }, "Close", 40, ["onClick"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click.stop>Close</button>\n</template>\n"
    );
}

#[test]
fn recovers_vue_cached_event_and_class_array() {
    let input = r#"
import { toDisplayString, normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = { props: { active: Boolean, count: Number } };
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("button", {
    class: normalizeClass(["counter", { active: _ctx.props.active }]),
    onClick: _cache[0] || (_cache[0] = (...args) => (_ctx.increment && _ctx.increment(...args)))
  }, toDisplayString(_ctx.props.count), 3));
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        active: Boolean,\n        count: Number\n    }\n}\n</script>\n\n<template>\n  <button class=\"counter\" :class=\"{ active: props.active }\" @click=\"increment\">{{ props.count }}</button>\n</template>\n"
        );
}

#[test]
fn recovers_legacy_function_cached_event_handler() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", {
    onClick: _cache[0] || (_cache[0] = function() { return _ctx.increment && _ctx.increment(...arguments); })
  }, "Go", 40, ["onClick"]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click=\"increment\">Go</button>\n</template>\n"
    );
}

#[test]
fn recovers_cached_event_direct_call() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("input", {
    onInput: _cache[0] || (_cache[0] = (t) => _ctx.onChange(t.target.checked))
  }, null, 40);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <input @input=\"onChange($event.target.checked)\" />\n</template>\n"
    );
}

#[test]
fn recovers_cached_compound_assignment_event() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", {
    onClick: _cache[0] || (_cache[0] = ($event) => _ctx.message += "!")
  }, "Append");
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click='message += \"!\"'>Append</button>\n</template>\n"
    );
}

#[test]
fn preserves_destructured_cached_vnode_hook_parameter() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("input", {
    onVnodeMounted: _cache[0] || (_cache[0] = ({ el }) => el.focus())
  });
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <input @vue:mounted=\"({ el })=>el.focus()\" />\n</template>\n"
    );
}

#[test]
fn recovers_logical_assign_cached_event_direct_call() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("input", {
    onInput: _cache[0] ||= (event) => _ctx.onChange(event.target.checked)
  }, null, 40);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <input @input=\"onChange($event.target.checked)\" />\n</template>\n"
    );
}

#[test]
fn recovers_cached_block_event_statements() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", {
    onClick: _cache[0] || (_cache[0] = (event) => {
      _ctx.addTodo(_ctx.todo);
      _ctx.todo = "";
    })
  }, "Add", 40);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <button @click='addTodo(todo); todo = \"\"'>Add</button>\n</template>\n"
    );
}

#[test]
fn recovers_cached_event_ref_assignment() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const ready = ref(false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => ready.value = true)
      }, "Go", 40, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst ready = ref(false);\n</script>\n\n<template>\n  <button @click=\"ready = true\">Go</button>\n</template>\n"
        );
}

#[test]
fn recovers_cached_event_update_without_importing_cache_param() {
    let input = r#"
import { n } from "./cache.js";
import { defineComponent, ref, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const count = ref(0);
    return (_ctx, n) => (
      openBlock(), createElementBlock("button", {
        onClick: n[0] || (n[0] = (event) => count.value++)
      }, toDisplayString(count.value), 40, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button @click=\"count++\">{{ count }}</button>\n</template>\n"
        );
}

#[test]
fn setup_ref_prevents_same_name_module_local_selection() {
    let input = r#"
import { defineComponent, ref, toDisplayString, openBlock, createElementBlock } from "vue";
export const count = document.createElement("link").relList;
export default defineComponent({
  setup() {
    const count = ref(0);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => count.value++)
      }, toDisplayString(count.value), 40, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst count = ref(0);\n</script>\n\n<template>\n  <button @click=\"count++\">{{ count }}</button>\n</template>\n"
        );
}

#[test]
fn recovers_tuple_ref_event_assignment() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [ready] = useState(false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("iframe", {
        onLoad: _cache[0] || (_cache[0] = (event) => ready.value = true),
        style: { height: ready.value ? "100px" : 0 }
      }, null, 44, ["onLoad", "style"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [ready] = useState(false);\n</script>\n\n<template>\n  <iframe @load=\"ready = true\" :style='{ height: ready ? \"100px\" : 0 }' />\n</template>\n"
        );
}

#[test]
fn recovers_tuple_local_used_only_by_template_bindings() {
    let input = r#"
import { defineComponent, unref, openBlock, createElementBlock, createCommentVNode } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [open, setOpen] = useState(false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("section", {
        disabled: !unref(open)
      }, [
        unref(open)
          ? (openBlock(), createElementBlock("p", { key: 0 }, "Open"))
          : createCommentVNode("", true)
      ], 8, ["disabled"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [open, setOpen] = useState(false);\n</script>\n\n<template>\n  <section :disabled=\"!open\">\n    <p v-if=\"open\">Open</p>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_tuple_ref_inside_class_binding() {
    let input = r#"
import { defineComponent, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [open, setOpen] = useState(false);
    const left = false;
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", {
        class: normalizeClass({ hidden: !(open.value && left === false) })
      }, null, 2)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [open, setOpen] = useState(false);\nconst left = false;\n</script>\n\n<template>\n  <div :class=\"{ hidden: !(open &amp;&amp; left === false) }\" />\n</template>\n"
        );
}

#[test]
fn recovers_tuple_ref_inside_inlined_computed_class_binding() {
    let input = r#"
import { defineComponent, computed, normalizeClass, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [open, setOpen] = useState(false);
    const left = false;
    const hidden = computed(() => open.value && left === false);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", {
        class: normalizeClass({ hidden: !hidden.value })
      }, null, 2)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [open, setOpen] = useState(false);\nconst left = false;\n</script>\n\n<template>\n  <div :class=\"{ hidden: !(open &amp;&amp; left === false) }\" />\n</template>\n"
        );
}

#[test]
fn recovers_computed_array_push_class_binding() {
    let input = r#"
import { defineComponent, computed, normalizeClass, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const level = "info";
    const size = "sm";
    const align = "left";
    const mirrored = false;
    const classes = computed(() => {
      const out = [];
      out.push(`stateTag-${level}`);
      if (size) {
        out.push(`stateTag-${size}`);
      }
      if (align === "left") {
        out.push("stateTag-left");
      } else if (mirrored) {
        out.push("stateTag-right");
      }
      return out;
    });
    return () => (
      openBlock(), createElementBlock("span", {
        class: normalizeClass(["stateTag", classes.value])
      }, "Ok", 2)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst level = \"info\";\nconst size = \"sm\";\nconst align = \"left\";\nconst mirrored = false;\n</script>\n\n<template>\n  <span class=\"stateTag\" :class='[ `stateTag-${level}`, size &amp;&amp; `stateTag-${size}`, align === \"left\" ? \"stateTag-left\" : mirrored &amp;&amp; \"stateTag-right\" ]'>Ok</span>\n</template>\n"
        );
}

#[test]
fn preserves_tuple_ref_assignment_in_script_handler() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const [ready] = useState(false);
    function markReady() {
      ready.value = true;
    }
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => ready.value = false),
        onDblclick: markReady
      }, "Go", 40, ["onClick", "onDblclick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { u as useState } from \"./state.js\";\n\nconst [ready] = useState(false);\nfunction markReady() {\n    ready.value = true;\n}\n</script>\n\n<template>\n  <button @click=\"ready = false\" @dblclick=\"markReady\">Go</button>\n</template>\n"
        );
}

#[test]
fn recovers_tuple_element_ref_event_assignment() {
    let input = r#"
import { defineComponent, openBlock, createElementBlock } from "vue";
import { s as slice } from "./helpers.js";
import { u as useState } from "./state.js";
export default defineComponent({
  setup() {
    const ready = slice(useState(false), 1)[0];
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("iframe", {
        onLoad: _cache[0] || (_cache[0] = (event) => ready.value = true),
        style: { height: ready.value ? "100px" : 0 }
      }, null, 44, ["onLoad", "style"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { s as slice } from \"./helpers.js\";\nimport { u as useState } from \"./state.js\";\n\nconst ready = slice(useState(false), 1)[0];\n</script>\n\n<template>\n  <iframe @load=\"ready = true\" :style='{ height: ready ? \"100px\" : 0 }' />\n</template>\n"
        );
}

#[test]
fn recovers_object_destructured_ref_event_assignment() {
    let input = r#"
import { defineComponent, unref, openBlock, createElementBlock } from "vue";
import { C as AppContext } from "./context.js";
export default defineComponent({
  setup() {
    const { selectedKind, isGrouped } = AppContext.inject();
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", null, [
        createElementBlock("button", {
          class: unref(selectedKind) === "primary" ? "active" : "",
          title: unref(isGrouped) ? "grouped" : "single",
          onClick: _cache[0] || (_cache[0] = (event) => selectedKind.value = "primary")
        }, "Primary", 42, ["class", "title", "onClick"])
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { C as AppContext } from \"./context.js\";\n\nconst { selectedKind, isGrouped } = AppContext.inject();\n</script>\n\n<template>\n  <div>\n    <button :class='selectedKind === \"primary\" ? \"active\" : \"\"' :title='isGrouped ? \"grouped\" : \"single\"' @click='selectedKind = \"primary\"'>Primary</button>\n  </div>\n</template>\n"
        );
}

#[test]
fn recovers_object_destructured_sibling_ref_in_inlined_computed() {
    let input = r#"
import { defineComponent, computed, unref, openBlock, createElementBlock, Fragment, renderList } from "vue";
import { C as AppContext } from "./context.js";
export default defineComponent({
  setup() {
    const { selected, isReady } = AppContext.inject();
    const visibleItems = computed(() => isReady.value ? ["one"] : []);
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("div", null, [
        createElementBlock("button", {
          class: unref(selected) === "one" ? "active" : "",
          onClick: _cache[0] || (_cache[0] = (event) => selected.value = "one")
        }, "One", 42, ["class", "onClick"]),
        (openBlock(true), createElementBlock(Fragment, null, renderList(visibleItems.value, (item) => (
          openBlock(), createElementBlock("span", { key: item }, item, 1)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { C as AppContext } from \"./context.js\";\n\nconst { selected, isReady } = AppContext.inject();\n</script>\n\n<template>\n  <div>\n    <button :class='selected === \"one\" ? \"active\" : \"\"' @click='selected = \"one\"'>One</button>\n    <span v-for='item in isReady ? [ \"one\" ] : []' :key=\"item\">{{ item }}</span>\n  </div>\n</template>\n"
        );
}

#[test]
fn recovers_object_destructure_depending_on_template_ref_key() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
import { useScroll } from "@vueuse/core";
export default defineComponent({
  props: {
    disabled: { type: Boolean, default: false }
  },
  setup(t) {
    const target = ref(null);
    const { x, arrivedState } = useScroll(target);
    const scrollLeft = () => {
      let t;
      if (!arrivedState.left) {
        if (!((t = target.value) === null || t === undefined)) {
          t.scroll({ left: x.value - 200 });
        }
      }
    };
    return () => (
      openBlock(), createElementBlock("div", {
        ref_key: "scrollContainer",
        ref: target
      }, [
        createElementBlock("button", {
          disabled: t.disabled || arrivedState.left,
          onClick: scrollLeft
        }, "Left", 8, ["disabled", "onClick"])
      ], 512)
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\nimport { useScroll } from \"@vueuse/core\";\n\nconst props = defineProps({\n    disabled: {\n        type: Boolean,\n        default: false\n    }\n});\nconst { disabled } = props;\n\nconst scrollContainer = ref(null);\n\nconst { x, arrivedState } = useScroll(scrollContainer);\nconst scrollLeft = ()=>{\n    let t;\n    if (!arrivedState.left) {\n        if (!((t = scrollContainer.value) === null || t === undefined)) {\n            t.scroll({\n                left: x.value - 200\n            });\n        }\n    }\n};\n</script>\n\n<template>\n  <div ref=\"scrollContainer\">\n    <button :disabled=\"disabled || arrivedState.left\" @click=\"scrollLeft\">Left</button>\n  </div>\n</template>\n"
        );
}

#[test]
fn cleans_template_ref_key_alias_value_in_template_expression() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const target = ref(null);
    return () => (
      openBlock(), createElementBlock("div", {
        ref_key: "scrollContainer",
        ref: target,
        title: target.value ? "ready" : "idle"
      }, null, 520, ["title"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst scrollContainer = ref(null);\n</script>\n\n<template>\n  <div ref=\"scrollContainer\" :title='scrollContainer ? \"ready\" : \"idle\"' />\n</template>\n"
        );
}

#[test]
fn does_not_emit_object_destructure_for_unref_read_only() {
    let input = r#"
import { defineComponent, unref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const { status } = useStatus();
    return () => (
      openBlock(), createElementBlock("p", {
        title: unref(status).label
      }, null, 8, ["title"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p :title=\"status.label\" />\n</template>\n"
    );
}

#[test]
fn recovers_ref_object_destructure_used_only_by_template_bindings() {
    let input = r#"
import { d as dc, K as sr, c as cp, q as ob, X as ce, Z as cc } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "BannerGate",
  setup() {
    const { isBannerEnabled, isFallbackEnabled } = sr(useSettings());
    const showFallback = cp(() => isFallbackEnabled.value);
    return () => (
      ob(), ce("section", null, [
        isBannerEnabled.value
          ? (ob(), ce("p", { key: 0 }, "Banner"))
          : cc("", true),
        showFallback.value
          ? (ob(), ce("p", { key: 1 }, "Fallback"))
          : cc("", true)
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { K as sr } from \"./vendor-vue-C85wAS_L.js\";\n\nconst { isBannerEnabled, isFallbackEnabled } = sr(useSettings());\n</script>\n\n<template>\n  <section>\n    <p v-if=\"isBannerEnabled\">Banner</p>\n    <p v-if=\"isFallbackEnabled\">Fallback</p>\n  </section>\n</template>\n"
        );
}

#[test]
fn does_not_select_ref_object_destructure_used_only_as_template_object_key() {
    let input = r#"
import { d as dc, K as sr, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "StaticSize",
  setup() {
    const { width, height } = sr(useWindowSize());
    return () => (
      ob(), ce("div", { style: { height: "100%" } }, null, 4)
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div :style='{ height: \"100%\" }' />\n</template>\n"
    );
}

#[test]
fn recovers_inline_object_spread_helper_in_style_attr() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
function ownKeys(object, enumerableOnly) {
  return Object.keys(object);
}
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("span", {
    style: (function(target) {
      for (let index = 1; index < arguments.length; index++) {
        var source = arguments[index] ?? {};
        if (index % 2) {
          ownKeys(Object(source), true).forEach((key) => { target[key] = source[key]; });
        } else if (Object.getOwnPropertyDescriptors) {
          Object.defineProperties(target, Object.getOwnPropertyDescriptors(source));
        } else {
          ownKeys(Object(source)).forEach((key) => {
            Object.defineProperty(target, key, Object.getOwnPropertyDescriptor(source, key));
          });
        }
      }
      return target;
    })({ cursor: _ctx.clickable ? "pointer" : "default" }, _ctx.padding && { padding: _ctx.padding })
  }, "Badge", 4);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <span :style='{ cursor: clickable ? \"pointer\" : \"default\", ...padding &amp;&amp; { padding: padding } }'>Badge</span>\n</template>\n"
        );
}

#[test]
fn preserves_setup_ref_assignment_in_script_handler() {
    let input = r#"
import { defineComponent, ref, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const ready = ref(false);
    function markReady() {
      ready.value = true;
    }
    return (_ctx, _cache) => (
      openBlock(), createElementBlock("button", {
        onClick: _cache[0] || (_cache[0] = (event) => ready.value = false),
        onDblclick: markReady
      }, "Go", 40, ["onClick", "onDblclick"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nimport { ref } from \"vue\";\n\nconst ready = ref(false);\n\nfunction markReady() {\n    ready.value = true;\n}\n</script>\n\n<template>\n  <button @click=\"ready = false\" @dblclick=\"markReady\">Go</button>\n</template>\n"
        );
}

#[test]
fn preserves_nested_event_shadowing() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", {
    onClick: _cache[0] || (_cache[0] = (e) => _ctx.report([1].map((e) => e + 1), e.target.checked))
  }, null, 8, ["onClick"]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <button @click=\"report([ 1 ].map((e)=>e + 1), $event.target.checked)\" />\n</template>\n"
        );
}

#[test]
fn recovers_cached_event_unref_call() {
    let input = r#"
import { d as dc, _ as ur, q as ob, X as ce } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "SubTab",
  setup() {
    return (_ctx, _cache) => (
      ob(), ce("li", {
        onClick: _cache[0] || (_cache[0] = (event) => ur(selectTab)(name))
      }, "Tab", 8, ["onClick"])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <li @click=\"selectTab(name)\">Tab</li>\n</template>\n"
    );
}

#[test]
fn recovers_conditional_branch_chain() {
    let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const _hoisted_1 = { key: 0 };
const _hoisted_2 = { key: 1 };
const _hoisted_3 = { key: 2 };
export function render(_ctx, _cache) {
  return (_ctx.status === 'loading')
    ? (openBlock(), createElementBlock("p", _hoisted_1, "Loading"))
    : (_ctx.status === 'error')
      ? (openBlock(), createElementBlock("p", _hoisted_2, toDisplayString(_ctx.error), 1))
      : (openBlock(), createElementBlock("p", _hoisted_3, "Ready"));
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <p v-if=\"status === 'loading'\">Loading</p>\n  <p v-else-if=\"status === 'error'\">{{ error }}</p>\n  <p v-else>Ready</p>\n</template>\n"
        );
}

#[test]
fn recovers_decompiled_if_return_branch_chain() {
    let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const _hoisted_1 = { key: 0 };
const _hoisted_2 = { key: 1 };
const _hoisted_3 = { key: 2 };
export function render(_ctx, _cache) {
  if (_ctx.status === "loading") {
    return openBlock(), createElementBlock("p", _hoisted_1, "Loading");
  }
  if (_ctx.status === 'error') {
    return openBlock(), createElementBlock("p", _hoisted_2, toDisplayString(_ctx.error), 1);
  }
  return openBlock(), createElementBlock("p", _hoisted_3, "Ready");
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <p v-if=\"status === 'loading'\">Loading</p>\n  <p v-else-if=\"status === 'error'\">{{ error }}</p>\n  <p v-else>Ready</p>\n</template>\n"
        );
}

#[test]
fn omits_empty_comment_vnode_else_branch() {
    let input = r#"
import { createCommentVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return _ctx.visible
    ? (openBlock(), createElementBlock("p", null, "Visible"))
    : createCommentVNode("v-if", true);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p v-if=\"visible\">Visible</p>\n</template>\n"
    );
}

#[test]
fn inverts_condition_when_empty_comment_vnode_is_consequent() {
    let input = r#"
import { createCommentVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return _ctx.visible
    ? createCommentVNode("v-if", true)
    : (openBlock(), createElementBlock("p", null, "Hidden"));
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p v-if=\"!visible\">Hidden</p>\n</template>\n"
    );
}

#[test]
fn recovers_render_list_fragment_with_mangled_item_param() {
    let input = r#"
import { renderList as r, Fragment as t, openBlock as n, createElementBlock as o, toDisplayString as s } from "vue";
export function render(e, a) {
  return n(), o("ul", null, [
    (n(true), o(t, null, r(e.items, e => (n(), o("li", { key: e.id }, s(e.name), 1))), 128))
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <ul>\n    <li v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn nested_v_for_fallback_params_do_not_shadow_each_other() {
    let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("table", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.rows, entry => (
      openBlock(), createElementBlock("tr", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.columns, key => (
          openBlock(), createElementBlock("td", null, toDisplayString(entry[key]), 1)
        )), 256))
      ])
    )), 256))
  ]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <table>\n    <tr v-for=\"item in rows\">\n      <td v-for=\"item_1 in columns\">{{ item[item_1] }}</td>\n    </tr>\n  </table>\n</template>\n"
    );
}

#[test]
fn v_for_fallback_param_avoids_outer_template_binding_capture() {
    let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const item = useSelectedItem();
    const items = useItems();
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(items, e => (
          openBlock(), createElementBlock("li", { key: e.id, title: item.label }, toDisplayString(e.name), 9, ["title"])
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst item = useSelectedItem();\nconst items = useItems();\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item_1 in items\" :key=\"item_1.id\" :title=\"item.label\">{{ item_1.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn recovers_render_list_index_param() {
    let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("ol", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.items, (e, i) => (
      openBlock(), createElementBlock("li", { key: i, title: i, class: i % 2 === 0 ? "even" : "odd" }, toDisplayString(e.name), 9, ["title", "class"])
    )), 128))
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <ol>\n    <li v-for=\"(item, index) in items\" :key=\"index\" :title=\"index\" :class='index % 2 === 0 ? \"even\" : \"odd\"'>{{ item.name }}</li>\n  </ol>\n</template>\n"
        );
}

#[test]
fn recovers_render_list_outer_context_member() {
    let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, createCommentVNode } from "vue";
export function render(e, _cache) {
  return openBlock(), createElementBlock("ul", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(e.items, (t, i) => (
      e.$slots.placeholder
        ? (openBlock(), createElementBlock("li", { key: t.id, title: i }, "Placeholder", 8, ["title"]))
        : createCommentVNode("", true)
    )), 128))
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <ul>\n    <template v-for=\"(item, index) in items\">\n      <li v-if=\"$slots.placeholder\" :key=\"item.id\" :title=\"index\">Placeholder</li>\n    </template>\n  </ul>\n</template>\n"
        );
}

#[test]
fn recovers_template_literal_text_children() {
    let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.items, (e, i) => (
      openBlock(), createElementBlock("p", { key: e.id }, `${toDisplayString(e.name)} - ${i}`, 1)
    )), 128))
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <p v-for=\"(item, index) in items\" :key=\"item.id\">{{ item.name }} - {{ index }}</p>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_text_vnode_string_concat_children() {
    let input = r#"
import { openBlock, createElementBlock, createElementVNode, createTextVNode, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("button", null, [
    createElementVNode("i", { class: "ion-plus-round" }, null, -1),
    createTextVNode(" " + toDisplayString(_ctx.following ? "Unfollow" : "Follow") + " " + toDisplayString(_ctx.username), 1)
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <button>\n    <i class=\"ion-plus-round\" />\n     {{ following ? \"Unfollow\" : \"Follow\" }} {{ username }}\n  </button>\n</template>\n"
        );
}

#[test]
fn recovers_element_text_string_concat_children() {
    let input = r#"
import { openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("span", null, "(" + toDisplayString(_ctx.count) + ")", 1);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <span>({{ count }})</span>\n</template>\n"
    );
}

#[test]
fn recovers_text_patch_expression_children() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("p", null, _ctx.format(_ctx.price), 1);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <p>{{ format(price) }}</p>\n</template>\n"
    );
}

#[test]
fn recovers_render_list_destructured_param() {
    let input = r#"
import { renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("section", null, [
    (openBlock(true), createElementBlock(Fragment, null, renderList(_ctx.entries, ([groupId, rows]) => (
      openBlock(), createElementBlock("article", { key: groupId }, toDisplayString(rows.length), 1)
    )), 128))
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <article v-for=\"[groupId, rows] in entries\" :key=\"groupId\">{{ rows.length }}</article>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_vite_fragment_alias_from_block() {
    let input = r#"
import { d as dc, q as ob, X as ce, F as fr, a0 as tv, R as td } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "FragmentBlock",
  setup() {
    return () => (
      ob(), ce(fr, { key: 0 }, [
        tv(td(count), 1)
      ], 64)
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  {{ count }}\n</template>\n"
    );
}

#[test]
fn recovers_component_vnode_and_named_slot() {
    let input = r#"
import { resolveComponent, createVNode, renderSlot, createTextVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_PanelHeader = resolveComponent("PanelHeader");
  return openBlock(), createElementBlock("article", null, [
    createVNode(_component_PanelHeader, { title: _ctx.title }, null, 8, ["title"]),
    renderSlot(_ctx.$slots, "body", {}, () => [
      _cache[0] || (_cache[0] = createTextVNode("Empty", -1))
    ])
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <article>\n    <PanelHeader :title=\"title\" />\n    <slot name=\"body\">Empty</slot>\n  </article>\n</template>\n"
        );
}

#[test]
fn recovers_vite_render_slot_alias() {
    let input = r#"
import { d as dc, q as ob, X as ce, Y as rs } from "./vendor-vue-C85wAS_L.js";
export const _ = dc({
  __name: "SlotForwarder",
  setup() {
    return (_ctx, _cache) => (
      ob(), ce("div", null, [
        rs(_ctx.$slots, "default")
      ])
    );
  }
});
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div>\n    <slot />\n  </div>\n</template>\n"
    );
}

#[test]
fn recovers_direct_slot_call_with_props() {
    let input = r#"
import { openBlock } from "vue";
export function render(_ctx, _cache) {
  openBlock();
  return _ctx.$slots.default({
    item: _ctx.item
  });
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <slot :item=\"item\" />\n</template>\n"
    );
}

#[test]
fn recovers_render_local_slot_call_alias() {
    let input = r#"
import { openBlock, createElementBlock, normalizeSlotValue } from "vue";
export function render(_ctx, _cache) {
  openBlock();
  const slot = _ctx.$slots.default && normalizeSlotValue(_ctx.$slots.default({
    item: _ctx.item
  }));
  if (_ctx.custom) {
    return slot;
  }
  return createElementBlock("span", null, "Fallback");
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <slot v-if=\"custom\" :item=\"item\" />\n  <span v-else>Fallback</span>\n</template>\n"
        );
}

#[test]
fn recovers_render_local_normalized_slot_call_alias() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
function normalizeSlotValue(value) {
  if (value.length === 1) {
    return value[0];
  }
  return value;
}
export function render(_ctx, _cache) {
  openBlock();
  const slot = _ctx.$slots.default && normalizeSlotValue(_ctx.$slots.default({
    item: _ctx.item
  }));
  if (_ctx.custom) {
    return slot;
  }
  return createElementBlock("span", null, "Fallback");
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <slot v-if=\"custom\" :item=\"item\" />\n  <span v-else>Fallback</span>\n</template>\n"
        );
}

#[test]
fn preserves_user_wrapped_slot_call_alias_as_unsupported() {
    let input = r#"
import { openBlock, createElementBlock } from "vue";
function transformSlot(value) {
  return value;
}
export function render(_ctx, _cache) {
  openBlock();
  const slot = _ctx.$slots.default && transformSlot(_ctx.$slots.default({
    item: _ctx.item
  }));
  if (_ctx.custom) {
    return slot;
  }
  return createElementBlock("span", null, "Fallback");
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <template v-if=\"custom\">\n    <!-- wakaru: slot -->\n  </template>\n  <span v-else>Fallback</span>\n</template>\n"
        );
}

#[test]
fn recovers_slot_bucket_children_and_logical_vnodes() {
    let input = r#"
import { h } from "./vendor-vue.js";
export default {
  setup(props, context) {
    const slots = context.slots;
    return () => {
      const slotState = partitionSlots(slots);
      const { slots: namedSlots } = slotState;
      return h(props.tag, null, [
        namedSlots["container-start"],
        h("main", null, [
          namedSlots["wrapper-start"],
          namedSlots["wrapper-end"]
        ]),
        props.showControls && [
          h("button", { class: "prev" }),
          h("button", { class: "next" })
        ],
        props.showBar && h("div", { class: "bar" }),
        namedSlots["container-end"]
      ]);
    };
  }
};
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <component :is=\"tag\">\n    <slot name=\"container-start\" />\n    <main>\n      <slot name=\"wrapper-start\" />\n      <slot name=\"wrapper-end\" />\n    </main>\n    <template v-if=\"showControls\">\n      <button class=\"prev\" />\n      <button class=\"next\" />\n    </template>\n    <div v-if=\"showBar\" class=\"bar\" />\n    <slot name=\"container-end\" />\n  </component>\n</template>\n"
        );
}

#[test]
fn recovers_render_local_slot_partition_vnode_children_as_default_slot() {
    let input = r#"
import { h } from "./vendor-vue.js";
function getConfig(props) {
  return props;
}
export default {
  props: {
    tag: String,
    wrapperTag: String,
    config: Object,
  },
  setup(props, context) {
    const slots = context.slots;
    const { params: p } = getConfig(props);
    return () => {
      const slotState = partitionSlots(slots);
      const { slides, slots: namedSlots } = slotState;
      return h(props.tag, null, [
        h(props.wrapperTag, { class: p.wrapperClass }, [
          namedSlots["wrapper-start"],
          renderSlides(slides),
          namedSlots["wrapper-end"]
        ])
      ]);
    };
  }
};
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    tag: String,\n    wrapperTag: String,\n    config: Object\n});\nconst { config, tag, wrapperTag } = props;\n\nfunction getConfig(props) {\n    return props;\n}\nconst { params: p } = getConfig(props);\n</script>\n\n<template>\n  <component :is=\"tag\">\n    <component :is=\"wrapperTag\" :class=\"p.wrapperClass\">\n      <slot name=\"wrapper-start\" />\n      <slot />\n      <slot name=\"wrapper-end\" />\n    </component>\n  </component>\n</template>\n"
        );
}

#[test]
fn scoped_slot_props_do_not_select_setup_locals_with_same_name() {
    let input = r#"
import { defineComponent, resolveComponent, createVNode, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const item = useSelectedItem();
    return () => {
      const _component_Card = resolveComponent("Card");
      return openBlock(), createElementBlock("section", null, [
        createVNode(_component_Card, null, {
          default: withCtx(({ item }) => [
            createElementVNode("span", { title: item.id }, toDisplayString(item.name), 9, ["title"])
          ]),
          _: 1
        })
      ]);
    };
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <Card>\n      <template v-slot:default=\"{ item }\">\n        <span :title=\"item.id\">{{ item.name }}</span>\n      </template>\n    </Card>\n  </section>\n</template>\n"
        );
}

#[test]
fn scoped_slot_aliased_props_keep_setup_ref_with_same_property_name() {
    let input = r#"
import { defineComponent, resolveComponent, createVNode, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export default defineComponent({
  setup() {
    const item = useSelectedItem();
    return () => {
      const _component_Card = resolveComponent("Card");
      return openBlock(), createElementBlock("section", null, [
        createVNode(_component_Card, null, {
          default: withCtx(({ item: row }) => [
            createElementVNode("span", null, toDisplayString(item.label + row.name), 1)
          ]),
          _: 1
        })
      ]);
    };
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst item = useSelectedItem();\n</script>\n\n<template>\n  <section>\n    <Card>\n      <template v-slot:default=\"{ item: row }\">\n        <span>{{ item.label + row.name }}</span>\n      </template>\n    </Card>\n  </section>\n</template>\n"
        );
}

#[test]
fn v_for_locals_do_not_select_setup_locals_with_same_name() {
    let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const items = useItems();
    const item = useSelectedItem();
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(items, item => (
          openBlock(), createElementBlock("li", { key: item.id }, toDisplayString(item.name), 1)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst items = useItems();\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn v_for_aliased_destructure_keeps_setup_ref_with_same_property_name() {
    let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const rows = useRows();
    const item = useSelectedItem();
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(rows, ({ item: row }) => (
          openBlock(), createElementBlock("li", { key: row.id }, toDisplayString(item.label + row.name), 1)
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst rows = useRows();\nconst item = useSelectedItem();\n</script>\n\n<template>\n  <ul>\n    <li v-for=\"{ item: row } in rows\" :key=\"row.id\">{{ item.label + row.name }}</li>\n  </ul>\n</template>\n"
        );
}

#[test]
fn v_for_event_locals_do_not_select_setup_locals_with_same_name() {
    let input = r#"
import { defineComponent, renderList, Fragment, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const items = useItems();
    const item = useSelectedItem();
    function select(row) {
      return row.id;
    }
    return () => (
      openBlock(), createElementBlock("ul", null, [
        (openBlock(true), createElementBlock(Fragment, null, renderList(items, item => (
          openBlock(), createElementBlock("button", {
            key: item.id,
            onClick: event => select(item)
          }, toDisplayString(item.name), 9, ["onClick"])
        )), 128))
      ])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst items = useItems();\nfunction select(row) {\n    return row.id;\n}\n</script>\n\n<template>\n  <ul>\n    <button v-for=\"item in items\" :key=\"item.id\" @click=\"select(item)\">{{ item.name }}</button>\n  </ul>\n</template>\n"
        );
}

#[test]
fn recovers_component_slot_object_children() {
    let input = r#"
import { resolveComponent, createVNode, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_DashboardCard = resolveComponent("DashboardCard");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_DashboardCard, { title: _ctx.title }, {
      header: withCtx(() => [
        createElementVNode("h2", null, "Latest")
      ]),
      default: withCtx(({ item }) => [
        createElementVNode("span", null, toDisplayString(item.name), 1)
      ]),
      _: 1
    }, 8, ["title"])
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <DashboardCard :title=\"title\">\n      <template v-slot:header>\n        <h2>Latest</h2>\n      </template>\n      <template v-slot:default=\"{ item }\">\n        <span>{{ item.name }}</span>\n      </template>\n    </DashboardCard>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_create_slots_dynamic_component_children() {
    let input = r#"
import { resolveComponent, createVNode, createSlots, withCtx, createElementVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_Navbar = resolveComponent("Navbar");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_Navbar, null, createSlots({
      topRow: withCtx(() => [
        createElementVNode("div", null, "Top")
      ]),
      _: 2
    }, [
      _ctx.showTitle ? {
        name: "navbarTitle",
        fn: withCtx(() => [
          createElementVNode("strong", null, "Title")
        ]),
        key: "0"
      } : undefined
    ]), 1024)
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <Navbar>\n      <template v-slot:topRow>\n        <div>Top</div>\n      </template>\n      <template v-if=\"showTitle\" v-slot:navbarTitle>\n        <strong>Title</strong>\n      </template>\n    </Navbar>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_render_list_dynamic_slot_names() {
    let input = r#"
import { resolveComponent, createVNode, createSlots, renderList, withCtx, createElementVNode, toDisplayString, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_I18nT = resolveComponent("I18nT");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_I18nT, { keypath: _ctx.configKey }, createSlots({ _: 2 }, [
      renderList(_ctx.props.config.slots, slot => ({
        name: slot.name,
        fn: withCtx(() => [
          createElementVNode("span", null, toDisplayString(slot.content), 1)
        ]),
        key: slot.name
      }))
    ]), 1024)
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <I18nT :keypath=\"configKey\">\n      <template v-for=\"slot in props.config.slots\" v-slot:[slot.name] :key=\"slot.name\">\n        <span>{{ slot.content }}</span>\n      </template>\n    </I18nT>\n  </section>\n</template>\n"
        );
}

#[test]
fn recovers_aliased_vue_builtin_component() {
    let input = r##"
import { Teleport as _Teleport, createBlock, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createBlock(_Teleport, { to: "#portal" }, [
    createElementBlock("div", null, "Popup")
  ]);
}
"##;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <Teleport to=\"#portal\">\n    <div>Popup</div>\n  </Teleport>\n</template>\n"
        );
}

#[test]
fn recovers_vendor_vue_transition_component_alias() {
    let input = r#"
import { d as defineComponent, n as openBlock, aa as createBlock, $ as withCtx, Y as renderSlot, aj } from "./vendor-vue.js";
export default defineComponent({
  emits: ["after-enter"],
  setup(props, context) {
    const send = context.emit;
    const cleanup = () => send("after-enter");
    const afterEnter = cleanup;
    return (ctx) => (
      openBlock(),
      createBlock(aj, {
        name: "fade",
        onAfterEnter: afterEnter
      }, {
        default: withCtx(() => [
          renderSlot(ctx.$slots, "default")
        ]),
        _: 3
      }, 8, ["onAfterEnter"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst send = defineEmits([\n    \"after-enter\"\n]);\n\nconst cleanup = ()=>send(\"after-enter\");\n</script>\n\n<template>\n  <Transition name=\"fade\" @after-enter=\"cleanup\">\n    <template v-slot:default>\n      <slot />\n    </template>\n  </Transition>\n</template>\n"
        );
}

#[test]
fn renames_setup_prop_when_consumed_alias_collides() {
    let input = r#"
import { defineComponent, openBlock, createBlock, Transition, unref } from "vue";
export default defineComponent({
  props: {
    x: {
      type: Boolean
    }
  },
  emits: ["done"],
  setup(props, context) {
    const p = props;
    const emit = context.emit;
    const mode = p.x ? "wide" : "tall";
    function finish() {
      if (mode) {
        emit("done");
      }
    }
    const x = finish;
    return () => (
      openBlock(),
      createBlock(Transition, {
        name: mode,
        onAfterLeave: finish,
        onLeaveCancelled: unref(x)
      }, null, 8, ["name", "onLeaveCancelled"])
    );
  }
});
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<script setup>\nconst props = defineProps({\n    x: {\n        type: Boolean\n    }\n});\nconst { x: x_1 } = props;\n\nconst emit = defineEmits([\n    \"done\"\n]);\n\nconst mode = x_1 ? \"wide\" : \"tall\";\nfunction finish() {\n    if (mode) {\n        emit(\"done\");\n    }\n}\n</script>\n\n<template>\n  <Transition :name=\"mode\" @after-leave=\"finish\" @leave-cancelled=\"finish\" />\n</template>\n"
        );
}

#[test]
fn recovers_component_v_model_pairs() {
    let input = r#"
import { resolveComponent, createVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_FormInput = resolveComponent("FormInput");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_FormInput, {
      modelValue: _ctx.name,
      "onUpdate:modelValue": $event => _ctx.name = $event,
      modelModifiers: { trim: true },
      filter: _ctx.filter,
      "onUpdate:filter": $event => _ctx.filter = $event,
      filterModifiers: { number: true, lazy: true },
      label: "Name"
    }, null, 8, ["modelValue", "filter"])
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <FormInput v-model.trim=\"name\" v-model:filter.number.lazy=\"filter\" label=\"Name\" />\n  </section>\n</template>\n"
        );
}

#[test]
fn preserves_custom_component_update_handlers() {
    let input = r#"
	import { resolveComponent, createVNode, openBlock, createElementBlock } from "vue";
	export function render(_ctx, _cache) {
  const _component_FormInput = resolveComponent("FormInput");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_FormInput, {
      visible: _ctx.visible,
      "onUpdate:visible": _ctx.closeAndLog
    }, null, 8, ["visible", "onUpdate:visible"])
  ]);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <section>\n    <FormInput :visible=\"visible\" @update:visible=\"closeAndLog\" />\n  </section>\n</template>\n"
        );
}

#[test]
fn preserves_component_update_handlers_with_side_effects() {
    let input = r#"
	import { resolveComponent, createVNode, openBlock, createElementBlock } from "vue";
	export function render(_ctx, _cache) {
	  const _component_FormInput = resolveComponent("FormInput");
	  return openBlock(), createElementBlock("section", null, [
	    createVNode(_component_FormInput, {
	      visible: _ctx.visible,
	      "onUpdate:visible": $event => {
	        _ctx.visible = $event;
	        _ctx.log($event);
	      }
	    }, null, 8, ["visible", "onUpdate:visible"])
	  ]);
	}
	"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        !recovered.contains("v-model"),
        "multi-statement update handlers must not collapse to v-model:\n{recovered}"
    );
    assert!(
        recovered.contains(r#":visible="visible""#)
            && recovered.contains(r#"@update:visible="visible = $event; log($event)""#),
        "update handler side effect should be preserved:\n{recovered}"
    );
}

#[test]
fn keeps_vueuse_composable_calls_in_script_setup() {
    let input = r#"
	import { defineComponent, openBlock, createElementBlock, toDisplayString } from "vue";
import { useStorage } from "@vueuse/core";
export default defineComponent({
  setup() {
    const token = useStorage("k", "");
    return (_ctx, _cache) => (
      openBlock(),
      createElementBlock("div", null, toDisplayString(token.value), 1)
    );
  }
});
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        recovered.contains(r#"import { useStorage } from "@vueuse/core";"#),
        "expected original composable import to be preserved:\n{recovered}"
    );
    assert!(
        recovered.contains(r#"const token = useStorage("k", "");"#),
        "expected original composable call to be preserved:\n{recovered}"
    );
    assert!(
        !recovered.contains(r#"ref("k", "")"#),
        "composable call must not be rewritten into ref():\n{recovered}"
    );
}

#[test]
fn keeps_relative_vueuse_composable_calls_in_script_setup() {
    let input = r#"
	import { defineComponent, openBlock, createElementBlock, toDisplayString } from "vue";
	import { useStorage } from "./vueuse-core.js";
	export default defineComponent({
	  setup() {
	    const token = useStorage("k", "");
	    return (_ctx, _cache) => (
	      openBlock(),
	      createElementBlock("div", null, toDisplayString(token.value), 1)
	    );
	  }
	});
	"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        recovered.contains(r#"import { useStorage } from "./vueuse-core.js";"#),
        "expected relative composable import to be preserved:\n{recovered}"
    );
    assert!(
        recovered.contains(r#"const token = useStorage("k", "");"#),
        "expected relative composable call to be preserved:\n{recovered}"
    );
    assert!(
        !recovered.contains(r#"ref("k", "")"#),
        "relative composable call must not be rewritten into ref():\n{recovered}"
    );
}

#[test]
fn computed_getter_with_nested_branch_stays_explicit() {
    let input = r#"
	import { defineComponent, computed, openBlock, createElementBlock, toDisplayString } from "vue";
export default defineComponent({
  setup() {
    const ready = true;
    const deep = false;
    const label = computed(() => {
      if (ready) {
        if (deep) {
          return "deep";
        }
        return "ready";
      }
      return "idle";
    });
    return (_ctx, _cache) => (
      openBlock(),
      createElementBlock("div", null, toDisplayString(label.value), 1)
    );
  }
});
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        recovered.contains("if (deep)"),
        "nested branch should not be collapsed away:\n{recovered}"
    );
    assert!(
        !recovered.contains(r#"ready ? "deep" : "idle""#),
        "computed recovery must not drop the nested branch:\n{recovered}"
    );
}

#[test]
fn computed_local_inliner_preserves_captured_names() {
    let input = r#"
	import { defineComponent, computed, openBlock, createElementBlock } from "vue";
	export default defineComponent({
	  setup() {
	    const current = source.value;
	    const label = computed(() => {
	      const suffix = current;
	      return items.value.map((current) => suffix + current.name).join(",");
	    });
	    return (_ctx, _cache) => (
	      openBlock(),
	      createElementBlock("p", { title: label.value }, null, 8, ["title"])
	    );
	  }
	});
	"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        recovered.contains("const label = computed(()=>{")
            && recovered.contains("const suffix = current;"),
        "computed block should stay explicit when inlining would capture names:\n{recovered}"
    );
    assert!(
        recovered.contains("map((current)=>suffix + current.name)")
            && !recovered.contains("map((current)=>current + current.name)"),
        "inliner must not capture the outer current binding inside the callback:\n{recovered}"
    );
}

#[test]
fn recovers_dynamic_component() {
    let input = r#"
import { resolveDynamicComponent, openBlock, createBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createBlock(resolveDynamicComponent(_ctx.currentView), {
    class: "panel"
  }, null, 512);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <component :is=\"currentView\" class=\"panel\" />\n</template>\n"
    );
}

#[test]
fn recovers_direct_dynamic_component_target() {
    let input = r#"
import { openBlock, createVNode } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createVNode(_ctx.currentView, {
    class: "panel"
  }, null, 512);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <component :is=\"currentView\" class=\"panel\" />\n</template>\n"
    );
}

#[test]
fn recovers_conditional_direct_dynamic_component_target() {
    let input = r#"
import { openBlock, createVNode, createCommentVNode } from "vue";
export function render(_ctx, _cache) {
  return _ctx.streamDisplay
    ? (openBlock(), createVNode(_ctx.streamDisplay.component))
    : createCommentVNode("", true);
}
"#;

    assert_eq!(
            recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default()).unwrap().unwrap(),
            "<template>\n  <component v-if=\"streamDisplay\" :is=\"streamDisplay.component\" />\n</template>\n"
        );
}

#[test]
fn recovers_model_and_show_directives() {
    let input = r#"
import { vModelText, vShow, withDirectives, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return withDirectives((openBlock(), createElementBlock("input", {
    "onUpdate:modelValue": _cache[0] || (_cache[0] = $event => _ctx.value = $event)
  }, null, 512)), [
    [vModelText, _ctx.value, void 0, { trim: true, number: true }],
    [vShow, _ctx.visible]
  ]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <input v-model.trim.number=\"value\" v-show=\"visible\" />\n</template>\n"
    );
}

#[test]
fn recovers_split_runtime_model_and_show_directives() {
    let input = r#"
import { withDirs } from "./chunk-directives.js";
import { modelText } from "./chunk-model.js";
import { show } from "./chunk-show.js";
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return withDirs((openBlock(), createElementBlock("input", {
    "onUpdate:modelValue": _cache[0] || (_cache[0] = $event => _ctx.value = $event)
  }, null, 512)), [
    [modelText, _ctx.value],
    [show, _ctx.visible]
  ]);
}
"#;
    let show_chunk = r#"
const localShow = {
  name: "show",
  beforeMount() {}
};
export { localShow as show };
"#;

    assert_eq!(
        recover_source_with_imports(input, |source| {
            (source == "./chunk-show.js").then(|| show_chunk.to_string())
        })
        .unwrap()
        .unwrap(),
        "<template>\n  <input v-model=\"value\" v-show=\"visible\" />\n</template>\n"
    );
}

#[test]
fn recovers_custom_directive_payload() {
    let input = r#"
import { resolveDirective, withDirectives, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _directive_focus = resolveDirective("focus");
  return withDirectives((openBlock(), createElementBlock("div", null, null, 512)), [
    [_directive_focus, _ctx.value, "current", { trim: true, deep: true }]
  ]);
}
"#;

    assert_eq!(
        recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
            .unwrap()
            .unwrap(),
        "<template>\n  <div v-focus:current.trim.deep=\"value\" />\n</template>\n"
    );
}
