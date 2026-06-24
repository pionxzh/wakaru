use std::collections::HashSet;

use swc_core::atoms::Atom;

use crate::js_names::is_valid_identifier_name;

use super::{
    setup_scope_bindings, setup_script_binding_refs, setup_value_dependency_refs,
    template_composable_refs, template_safe_local_refs, VueRecoveryContext, VueSetupLocalBinding,
    VueTemplateUsage,
};

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
