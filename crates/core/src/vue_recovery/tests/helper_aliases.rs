use super::*;

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
