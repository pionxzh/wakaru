use super::*;

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
