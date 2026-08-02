use super::*;

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
fn recognizes_use_model_as_inline_script_setup_ref() {
    let input = r#"
import { useModel, vModelText, withDirectives, openBlock, createElementBlock } from "vue";
const component = {
  __name: "Example",
  props: { modelValue: {}, modelModifiers: {} },
  emits: ["update:modelValue"],
  setup(__props) {
    const value = useModel(__props, "modelValue");
    return (_ctx, _cache) => withDirectives(
      (openBlock(), createElementBlock("input", {
        "onUpdate:modelValue": _cache[0] || (_cache[0] = $event => value.value = $event)
      }, null, 512)),
      [[vModelText, value.value]]
    );
  }
};
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("const value = useModel(props, \"modelValue\");"),
        "{recovered}"
    );
    assert!(recovered.contains("v-model=\"value\""), "{recovered}");
    assert!(
        !recovered.contains("v-model=\"value.value\""),
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
fn cleans_minified_external_render_props_parameter() {
    let input = r#"
const component = {
  __name: "Example",
  props: { status: String },
  setup(e, { expose: r }) {
    r();
    const returned = {};
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
import { openBlock as r, createElementBlock as t, createCommentVNode as o } from "vue";
export function render(e, o, u, i, c, d) {
  return u.status === "ready"
    ? (r(), t("p", { key: 0 }, "Ready"))
    : o("v-if", true);
}
component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("v-if=\"status === 'ready'\""),
        "{recovered}"
    );
    assert!(!recovered.contains("u.status"), "{recovered}");
}

#[test]
fn restores_initializer_moved_into_compiled_setup_return_object() {
    let input = r#"
import { useModel, vModelText, withDirectives, openBlock, createElementBlock } from "vue";
const component = {
  __name: "Example",
  props: { modelValue: {}, modelModifiers: {} },
  emits: ["update:modelValue"],
  setup(__props, { expose: __expose }) {
    __expose();
    const value = void 0;
    const returned = { value: useModel(__props, "modelValue") };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
export function render(_ctx, _cache, $props, $setup) {
  return withDirectives(
    (openBlock(), createElementBlock("input", {
      "onUpdate:modelValue": _cache[0] || (_cache[0] = $event => $setup.value = $event)
    }, null, 512)),
    [[vModelText, $setup.value]]
  );
}
component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("import { useModel } from \"vue\";"),
        "{recovered}"
    );
    assert!(
        recovered.contains("const value = useModel(props, \"modelValue\");"),
        "{recovered}"
    );
    assert!(recovered.contains("v-model=\"value\""), "{recovered}");
}

#[test]
fn folds_rehydrated_props_alias_into_define_props_binding() {
    let input = r#"
const component = {
  __name: "Example",
  props: { active: Boolean, count: Number },
  emits: ["increment"],
  setup(__props, { expose: __expose, emit: __emit }) {
    __expose();
    const props = void 0;
    const emit = __emit;
    function increment() {
      emit("increment");
    }
    const returned = { props: __props, emit, increment };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
import { normalizeClass, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock("button", {
    class: normalizeClass(["counter", { active: $setup.props.active }]),
    onClick: $setup.increment
  }, toDisplayString($setup.props.count), 3);
}
component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("const props = defineProps("),
        "{recovered}"
    );
    assert!(
        !recovered.contains("const __props = defineProps("),
        "{recovered}"
    );
    assert!(!recovered.contains("const props = __props;"), "{recovered}");
}

#[test]
fn folds_non_default_returned_props_alias_without_dangling_template_refs() {
    let input = r#"
const component = {
  __name: "Example",
  props: { active: Boolean, count: Number },
  setup(__props, { expose: __expose }) {
    __expose();
    const returned = { myProps: __props };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
import { normalizeClass, openBlock, createElementBlock, toDisplayString } from "vue";
export function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock("button", {
    class: normalizeClass(["counter", { active: $setup.myProps.active }])
  }, toDisplayString($setup.myProps.count), 3);
}
component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("const props = defineProps("),
        "{recovered}"
    );
    assert!(recovered.contains(":class=\"{ active }\""), "{recovered}");
    assert!(recovered.contains("{{ count }}"), "{recovered}");
    assert!(!recovered.contains("myProps"), "{recovered}");
}

#[test]
fn preserves_non_default_returned_emit_alias_used_by_template() {
    let input = r#"
const component = {
  __name: "Example",
  emits: ["increment"],
  setup(__props, { expose: __expose, emit: __emit }) {
    __expose();
    const returned = { fire: __emit };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock("button", {
    onClick: $setup.fire
  }, "Increment");
}
component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    assert!(
        recovered.contains("const fire = defineEmits("),
        "{recovered}"
    );
    assert!(recovered.contains("@click=\"fire\""), "{recovered}");
    assert!(
        !recovered.contains("const emit = defineEmits("),
        "{recovered}"
    );
}

#[test]
fn preserves_order_of_initializers_moved_into_setup_return_object() {
    let input = r#"
const component = {
  __name: "Example",
  setup(__props, { expose: __expose }) {
    __expose();
    const returned = {
      first: alpha(),
      second: beta()
    };
    Object.defineProperty(returned, "__isScriptSetup", {
      enumerable: false,
      value: true
    });
    return returned;
  }
};
import { openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache, $props, $setup) {
  return openBlock(), createElementBlock("p", null, $setup.first + $setup.second, 1);
}
component.render = render;
export default component;
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();

    let first = recovered.find("const first = alpha();").unwrap();
    let second = recovered.find("const second = beta();").unwrap();
    assert!(
        first < second,
        "setup initializer order changed:\n{recovered}"
    );
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
