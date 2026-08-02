use super::*;

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
