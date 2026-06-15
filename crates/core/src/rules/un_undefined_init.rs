use std::collections::{HashMap, HashSet};
use swc_core::common::Mark;

use swc_core::ecma::ast::{Module, Pat, VarDecl, VarDeclKind};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::binding_facts::collect_binding_facts;
use super::dead_decls::remove_consumed_uninitialized_decls;
use super::decl_utils::{binding_id, BindingId};
use super::expr_utils::is_unresolved_undefined;

/// Remove redundant `= undefined` / `= void 0` from `let` and `var` declarations.
///
/// `let x = undefined` → `let x`
///
/// `const` is excluded because it requires an initializer.
pub struct UnUndefinedInit {
    unresolved_mark: Mark,
    binding_references: HashMap<BindingId, usize>,
    converted_unused_bindings: HashSet<BindingId>,
}

impl UnUndefinedInit {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            binding_references: HashMap::new(),
            converted_unused_bindings: HashSet::new(),
        }
    }
}

impl VisitMut for UnUndefinedInit {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let facts = collect_binding_facts(module);
        self.binding_references = facts.references;
        self.converted_unused_bindings.clear();

        module.visit_mut_children_with(self);
        remove_consumed_uninitialized_decls(module, &self.converted_unused_bindings);
    }

    fn visit_mut_var_decl(&mut self, decl: &mut VarDecl) {
        decl.visit_mut_children_with(self);

        if decl.kind == VarDeclKind::Const {
            return;
        }

        for declarator in &mut decl.decls {
            let Pat::Ident(binding) = &declarator.name else {
                continue;
            };
            if let Some(init) = &declarator.init {
                if is_unresolved_undefined(init, self.unresolved_mark) {
                    let binding = binding_id(&binding.id);
                    declarator.init = None;
                    if self.binding_references.get(&binding).copied().unwrap_or(0) <= 1 {
                        self.converted_unused_bindings.insert(binding);
                    }
                }
            }
        }
    }
}
