use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::Module;

use crate::analysis::binding_uses::BindingUseIndex;

use super::decl_utils::BindingId;

pub(crate) struct BindingFacts {
    pub(crate) uninitialized: HashSet<BindingId>,
    pub(crate) references: HashMap<BindingId, usize>,
}

pub(crate) fn collect_binding_facts(module: &Module) -> BindingFacts {
    let index = BindingUseIndex::collect(module);
    BindingFacts {
        uninitialized: index.uninitialized_bindings(),
        references: index.legacy_reference_counts(),
    }
}
