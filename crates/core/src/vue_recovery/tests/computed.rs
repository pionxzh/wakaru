use super::*;

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
