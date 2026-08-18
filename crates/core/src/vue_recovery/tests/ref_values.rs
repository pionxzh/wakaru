use super::*;

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
fn preserves_body_shadowed_context_member() {
    // The `_ctx` rebinding lives in a nested function BODY, not a param:
    // since swc_ecma_ast 29 function bodies are a distinct node from blocks,
    // the context-member cleaner's body-level shadow registration must fire
    // for `FunctionBody` or `_ctx.msg` would wrongly collapse to `msg`.
    let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  return openBlock(), createElementBlock("div", {
    title: (function () { const _ctx = getCtx(); return _ctx.msg; })()
  }, null, 8, ["title"]);
}
"#;

    let recovered = recover_vue_sfc_source_from_js(input, VueSfcRecoveryOptions::default())
        .unwrap()
        .unwrap();
    assert!(
        recovered.contains("_ctx.msg"),
        "body-level `const _ctx` must keep shadowing the render context: {recovered}"
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
