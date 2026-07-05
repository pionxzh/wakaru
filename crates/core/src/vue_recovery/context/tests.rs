use super::super::VueRecoveryContext;
use super::{binding_renames, is_vue_helper_candidate_source, SetupPropsRefRewriter};
use std::collections::HashMap;
use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{Expr, Ident, Prop, PropName};
use swc_core::ecma::visit::VisitMutWith;

#[test]
fn vue_helper_candidates_exclude_adjacent_bare_packages() {
    assert!(!is_vue_helper_candidate_source("vuex"));
    assert!(!is_vue_helper_candidate_source("vue-router"));
    assert!(!is_vue_helper_candidate_source("@vueuse/core"));
    assert!(!is_vue_helper_candidate_source("vue-i18n"));
    assert!(!is_vue_helper_candidate_source("vue-demi"));
    assert!(!is_vue_helper_candidate_source("@tanstack/vue-query"));
}

#[test]
fn vue_helper_candidates_exclude_adjacent_relative_chunks() {
    assert!(!is_vue_helper_candidate_source("./vueuse-core.js"));
    assert!(!is_vue_helper_candidate_source("./chunks/vue-router.js"));
    assert!(!is_vue_helper_candidate_source("./chunks/vuex.js"));
    assert!(!is_vue_helper_candidate_source("./chunks/vue-i18n.js"));
    assert!(!is_vue_helper_candidate_source("./vue-demi.js"));
    assert!(!is_vue_helper_candidate_source("./vendor-vue-query.js"));
}

#[test]
fn vue_helper_candidates_keep_runtime_and_local_vue_chunks() {
    assert!(is_vue_helper_candidate_source("@vue/runtime-core"));
    assert!(is_vue_helper_candidate_source("@vue/runtime-dom"));
    assert!(is_vue_helper_candidate_source("./vendor-vue.js"));
}

#[test]
fn binding_renames_key_on_recorded_top_level_context() {
    // Alias renaming now flows through rename_utils::BindingRenamer, which is
    // keyed on (name, SyntaxContext). `binding_renames` supplies that context
    // from the recorded top-level bindings and skips names that are not
    // top-level bindings (so the alias never touches an inner-scope local).
    let ctxt = SyntaxContext::empty();
    let mut ctx = VueRecoveryContext::default();
    ctx.top_level_binding_ctxts.insert(Atom::from("P"), ctxt);
    let aliases = HashMap::from([
        (Atom::from("P"), Atom::from("Panel_1")),
        (Atom::from("Absent"), Atom::from("Absent_1")),
    ]);

    let renames = binding_renames(&aliases, &ctx);

    assert_eq!(renames.len(), 1);
    assert_eq!(renames[0].old, (Atom::from("P"), ctxt));
    assert_eq!(renames[0].new, Atom::from("Panel_1"));
}

#[test]
fn setup_props_ref_rewriter_expands_shorthand_property_keys() {
    let mut ctx = VueRecoveryContext {
        setup_props_context: Some(Atom::from("p")),
        ..Default::default()
    };
    ctx.setup_props_aliases.insert(Atom::from("propsAlias"));
    let mut prop = Prop::Shorthand(Ident::new(Atom::from("p"), DUMMY_SP, Default::default()));

    prop.visit_mut_with(&mut SetupPropsRefRewriter::new(&ctx, "props"));

    let Prop::KeyValue(key_value) = prop else {
        panic!("shorthand property should be expanded when its value is rewritten");
    };
    assert!(matches!(&key_value.key, PropName::Ident(key) if key.sym.as_ref() == "p"));
    assert!(
        matches!(key_value.value.as_ref(), Expr::Ident(value) if value.sym.as_ref() == "props")
    );
}
