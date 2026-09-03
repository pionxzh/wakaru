use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::Module;

use crate::analysis::binding_uses::BindingUseIndex;

use super::decl_utils::BindingId;

pub(crate) struct BindingFacts {
    /// Every declarator without an initializer, any kind. Dead-declaration
    /// removal uses this.
    pub(crate) uninitialized: HashSet<BindingId>,
    /// Uninitialized declarators a pattern may assign without observable
    /// difference: hoisted `var _a;` (the compiler-temp shape) and `let _a;`
    /// that straight-line control flow definitely initializes before every
    /// use (what `VarDeclToLetConst` makes of the former before the cleanup
    /// passes) — see `BindingUseIndex::assignable_uninitialized_bindings`. A
    /// `let n;` declared after the pattern, or in a `switch` case another case
    /// can skip, is in its TDZ when the pattern assigns it; deleting the
    /// assignment would drop that ReferenceError.
    pub(crate) assignable_uninitialized: HashSet<BindingId>,
    pub(crate) references: HashMap<BindingId, usize>,
}

pub(crate) fn collect_binding_facts(module: &Module) -> BindingFacts {
    let index = BindingUseIndex::collect(module);
    BindingFacts {
        uninitialized: index.uninitialized_bindings(),
        assignable_uninitialized: index.assignable_uninitialized_bindings(),
        references: index.legacy_reference_counts(),
    }
}
