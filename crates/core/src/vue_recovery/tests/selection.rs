use super::*;

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
