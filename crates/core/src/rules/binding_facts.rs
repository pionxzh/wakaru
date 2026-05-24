use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{Ident, Module, Pat, VarDeclarator};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::decl_utils::{binding_id, BindingId};

pub(crate) struct BindingFacts {
    pub(crate) uninitialized: HashSet<BindingId>,
    pub(crate) references: HashMap<BindingId, usize>,
}

pub(crate) fn collect_binding_facts(module: &Module) -> BindingFacts {
    let mut collector = BindingFactsCollector::default();
    module.visit_with(&mut collector);
    BindingFacts {
        uninitialized: collector.uninitialized,
        references: collector.references,
    }
}

#[derive(Default)]
struct BindingFactsCollector {
    uninitialized: HashSet<BindingId>,
    references: HashMap<BindingId, usize>,
}

impl Visit for BindingFactsCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if declarator.init.is_none() {
            if let Pat::Ident(binding) = &declarator.name {
                self.uninitialized.insert(binding_id(&binding.id));
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        *self.references.entry(binding_id(ident)).or_insert(0) += 1;
    }
}
