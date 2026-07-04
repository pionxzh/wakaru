use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{Callee, Decl, Expr, Pat, Stmt};

use crate::js_names::is_valid_identifier_name;

use super::context::{is_ref_object_alias, is_ref_object_expr};
use super::locals::{
    collect_local_pat_bindings, setup_script_binding_refs, setup_value_dependency_refs,
    VueSetupLocalBinding,
};
use super::{VueRecoveryContext, VueTemplateUsage};

pub(super) fn setup_local_declarations<'a>(
    ctx: &'a VueRecoveryContext,
    template_usage: &VueTemplateUsage,
) -> Vec<&'a VueSetupLocalBinding> {
    if ctx.script_local_bindings.is_empty() && ctx.setup_local_bindings.is_empty() {
        return Vec::new();
    }

    let candidates = ctx
        .script_local_bindings
        .iter()
        .chain(ctx.setup_local_bindings.iter())
        .collect::<Vec<_>>();
    let selection_context = VueSetupSelectionContext::new(ctx, template_usage, &candidates);
    let selected = VueSelectionPlan::new(selection_context).select(&candidates);

    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, declaration)| selected.contains(&index).then_some(declaration))
        .collect()
}

pub(super) struct VueSetupSelectionContext {
    pub(super) setup_scope_bindings: HashSet<Atom>,
    pub(super) initial_setup_refs: HashSet<Atom>,
}

impl VueSetupSelectionContext {
    pub(super) fn new(
        ctx: &VueRecoveryContext,
        template_usage: &VueTemplateUsage,
        candidates: &[&VueSetupLocalBinding],
    ) -> Self {
        let setup_scope_bindings = setup_scope_bindings(ctx, candidates);
        let mut initial_setup_refs = template_usage.event_refs.clone();
        initial_setup_refs.extend(template_composable_refs(ctx, &template_usage.expr_refs));
        initial_setup_refs.extend(template_safe_local_refs(
            ctx,
            candidates,
            &template_usage.expr_refs,
            &template_usage.read_refs,
        ));
        initial_setup_refs.extend(setup_value_dependency_refs(ctx, template_usage));
        initial_setup_refs.extend(setup_script_binding_refs(ctx));

        Self {
            setup_scope_bindings,
            initial_setup_refs,
        }
    }
}

pub(super) struct VueSelectionPlan {
    setup_scope_bindings: HashSet<Atom>,
    setup_wanted_refs: HashSet<Atom>,
    module_wanted_refs: HashSet<Atom>,
}

impl VueSelectionPlan {
    pub(super) fn new(context: VueSetupSelectionContext) -> Self {
        Self {
            setup_scope_bindings: context.setup_scope_bindings,
            setup_wanted_refs: context.initial_setup_refs,
            module_wanted_refs: HashSet::new(),
        }
    }

    pub(super) fn select(mut self, candidates: &[&VueSetupLocalBinding]) -> HashSet<usize> {
        let mut selected = HashSet::new();
        loop {
            let mut changed = false;
            self.route_setup_refs_to_module_scope();
            for (index, declaration) in candidates.iter().enumerate() {
                if selected.contains(&index) || !self.wants_declaration(declaration) {
                    continue;
                }

                selected.insert(index);
                self.add_dependencies(declaration);
                changed = true;
            }

            if !changed {
                break;
            }
        }
        selected
    }

    fn route_setup_refs_to_module_scope(&mut self) {
        self.module_wanted_refs.extend(
            self.setup_wanted_refs
                .iter()
                .filter(|binding| !self.setup_scope_bindings.contains(*binding))
                .cloned(),
        );
    }

    fn wants_declaration(&self, declaration: &VueSetupLocalBinding) -> bool {
        let wanted_refs = if declaration.module_scope {
            &self.module_wanted_refs
        } else {
            &self.setup_wanted_refs
        };
        declaration
            .bindings
            .iter()
            .chain(declaration.emitted_bindings.iter())
            .any(|binding| {
                is_valid_identifier_name(binding.as_ref()) && wanted_refs.contains(binding)
            })
    }

    fn add_dependencies(&mut self, declaration: &VueSetupLocalBinding) {
        if declaration.module_scope {
            self.module_wanted_refs
                .extend(declaration.refs.iter().cloned());
        } else {
            self.setup_wanted_refs
                .extend(declaration.refs.iter().cloned());
        }
    }
}

fn template_composable_refs(ctx: &VueRecoveryContext, expr_refs: &HashSet<Atom>) -> HashSet<Atom> {
    expr_refs
        .iter()
        .filter(|binding| ctx.bindings.composable_refs.contains(*binding))
        .cloned()
        .collect()
}

fn template_safe_local_refs(
    ctx: &VueRecoveryContext,
    candidates: &[&VueSetupLocalBinding],
    expr_refs: &HashSet<Atom>,
    expr_read_refs: &HashSet<Atom>,
) -> HashSet<Atom> {
    candidates
        .iter()
        .filter(|declaration| {
            selects_safe_template_expr_local(ctx, declaration, expr_refs, expr_read_refs)
        })
        .flat_map(|declaration| {
            declaration
                .bindings
                .iter()
                .chain(declaration.emitted_bindings.iter())
                .filter(|binding| expr_refs.contains(*binding))
                .cloned()
        })
        .collect()
}

fn setup_scope_bindings(
    ctx: &VueRecoveryContext,
    candidates: &[&VueSetupLocalBinding],
) -> HashSet<Atom> {
    let mut bindings = candidates
        .iter()
        .filter(|declaration| !declaration.module_scope)
        .flat_map(|declaration| {
            declaration
                .bindings
                .iter()
                .chain(declaration.emitted_bindings.iter())
                .cloned()
        })
        .collect::<HashSet<_>>();
    if let Some(binding) = &ctx.setup_props_context {
        bindings.insert(binding.clone());
    }
    bindings.extend(ctx.setup_props_aliases.iter().cloned());
    if let Some(binding) = &ctx.setup_context {
        bindings.insert(binding.clone());
    }
    if let Some(binding) = &ctx.setup_emit_context {
        bindings.insert(binding.clone());
    }
    bindings.extend(ctx.setup_emit_aliases.iter().cloned());
    bindings.extend(ctx.slot_bindings.iter().cloned());
    bindings.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    bindings.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    bindings
}

fn selects_safe_template_expr_local(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
    expr_refs: &HashSet<Atom>,
    expr_read_refs: &HashSet<Atom>,
) -> bool {
    if !declaration.template_selectable
        && !direct_computed_value_template_ref(ctx, declaration, expr_read_refs)
    {
        return false;
    }
    if !any_binding_ref(declaration, expr_refs) {
        return false;
    }
    if declaration.module_scope {
        return true;
    }
    match &declaration.stmt {
        Stmt::Decl(Decl::Fn(_)) | Stmt::Decl(Decl::Class(_)) => true,
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().any(|decl| {
            let mut decl_bindings = HashSet::new();
            collect_local_pat_bindings(&decl.name, &mut decl_bindings);
            if !decl_bindings
                .iter()
                .any(|binding| expr_refs.contains(binding))
                && !declaration
                    .emitted_bindings
                    .iter()
                    .any(|binding| expr_refs.contains(binding))
            {
                return false;
            }
            if matches!(decl.name, Pat::Ident(_))
                && decl
                    .init
                    .as_deref()
                    .is_some_and(|init| is_opaque_vue_helper_candidate_call(init, ctx))
            {
                return false;
            }
            matches!(decl.name, Pat::Ident(_) | Pat::Array(_))
                || (matches!(decl.name, Pat::Object(_))
                    && (decl_bindings
                        .iter()
                        .any(|binding| expr_read_refs.contains(binding))
                        || declaration
                            .emitted_bindings
                            .iter()
                            .any(|binding| expr_read_refs.contains(binding)))
                    && decl.init.as_deref().is_some_and(|init| {
                        is_ref_object_expr(init, ctx)
                            || is_ref_object_alias(init, ctx)
                            || declaration_refs_setup_props(ctx, declaration)
                    }))
        }),
        _ => false,
    }
}

fn any_binding_ref(declaration: &VueSetupLocalBinding, refs: &HashSet<Atom>) -> bool {
    declaration
        .bindings
        .iter()
        .chain(declaration.emitted_bindings.iter())
        .any(|binding| refs.contains(binding))
}

fn direct_computed_value_template_ref(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
    expr_refs: &HashSet<Atom>,
) -> bool {
    declaration
        .bindings
        .iter()
        .chain(declaration.emitted_bindings.iter())
        .any(|binding| expr_refs.contains(binding) && ctx.bindings.values.contains_key(binding))
}

fn declaration_refs_setup_props(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
) -> bool {
    ctx.setup_props_context
        .as_ref()
        .is_some_and(|binding| declaration.refs.contains(binding))
        || ctx
            .setup_props_aliases
            .iter()
            .any(|binding| declaration.refs.contains(binding))
}

fn is_opaque_vue_helper_candidate_call(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match expr {
        Expr::Paren(paren) => is_opaque_vue_helper_candidate_call(paren.expr.as_ref(), ctx),
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            let Expr::Ident(ident) = callee.as_ref() else {
                return false;
            };
            ctx.vue_helper_candidates.contains(&ident.sym)
                && !ctx.vue_helpers.contains_key(&ident.sym)
        }
        _ => false,
    }
}
