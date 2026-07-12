use super::super::VueRecoveryContext;
use super::{
    binding_renames, is_vue_helper_candidate_source, record_compiled_setup_alias,
    setup_alias_renames, setup_props_renames,
};
use std::collections::HashMap;
use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{Expr, Ident};

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
fn setup_alias_renames_key_on_recorded_alias_context() {
    // Setup-scope alias rewriting now flows through rename_utils::BindingRenamer.
    // `setup_alias_renames` supplies each alias source's recorded SyntaxContext so
    // only the aliased binding's references are renamed, and skips aliases whose
    // context was never recorded (and are not top-level bindings).
    let ctxt = SyntaxContext::empty();
    let mut ctx = VueRecoveryContext::default();
    ctx.bindings
        .aliases
        .insert(Atom::from("p"), Atom::from("props"));
    ctx.bindings.alias_ctxts.insert(Atom::from("p"), ctxt);
    ctx.bindings
        .aliases
        .insert(Atom::from("q"), Atom::from("other"));

    let renames = setup_alias_renames(&ctx);

    assert_eq!(renames.len(), 1);
    assert_eq!(renames[0].old, (Atom::from("p"), ctxt));
    assert_eq!(renames[0].new, Atom::from("props"));
}

#[test]
fn setup_props_renames_key_on_recorded_props_source_contexts() {
    // Props-ref rewriting flows through rename_utils::BindingRenamer.
    // `setup_props_renames` maps the setup props parameter and every props alias
    // onto the emitted props binding, keyed on each source's recorded context so
    // an inner-scope local of the same name is never rewritten.
    let param_ctxt = SyntaxContext::empty();
    let mut ctx = VueRecoveryContext {
        setup_props_context: Some(Atom::from("p")),
        setup_props_context_ctxt: Some(param_ctxt),
        ..Default::default()
    };
    ctx.setup_props_aliases.insert(Atom::from("propsAlias"));
    ctx.setup_props_alias_ctxts
        .insert(Atom::from("propsAlias"), param_ctxt);

    let renames = setup_props_renames(&ctx, "props");

    assert_eq!(renames.len(), 2);
    assert!(renames.iter().all(|rename| rename.new == "props"));
    assert!(renames
        .iter()
        .any(|rename| rename.old == (Atom::from("p"), param_ctxt)));
    assert!(renames
        .iter()
        .any(|rename| rename.old == (Atom::from("propsAlias"), param_ctxt)));
}

#[test]
fn returned_object_keys_register_props_emit_and_slots_aliases() {
    let mut ctx = VueRecoveryContext {
        setup_props_context: Some(Atom::from("__props")),
        setup_emit_context: Some(Atom::from("__emit")),
        ..Default::default()
    };
    ctx.slot_bindings.insert(Atom::from("__slots"));

    for (binding, source) in [
        ("myProps", "__props"),
        ("fire", "__emit"),
        ("mySlots", "__slots"),
    ] {
        let expr = Expr::Ident(Ident::new(
            Atom::from(source),
            DUMMY_SP,
            SyntaxContext::empty(),
        ));
        assert!(record_compiled_setup_alias(
            &Atom::from(binding),
            None,
            &expr,
            &mut ctx,
        ));
    }

    assert!(ctx.setup_props_aliases.contains(&Atom::from("myProps")));
    assert!(ctx.setup_emit_aliases.contains(&Atom::from("fire")));
    assert!(ctx.slot_bindings.contains(&Atom::from("mySlots")));
}
