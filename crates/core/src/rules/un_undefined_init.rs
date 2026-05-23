use swc_core::common::Mark;
use swc_core::ecma::ast::{Pat, VarDecl, VarDeclKind};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::expr_utils::is_unresolved_undefined;

/// Remove redundant `= undefined` / `= void 0` from `let` and `var` declarations.
///
/// `let x = undefined` → `let x`
///
/// `const` is excluded because it requires an initializer.
pub struct UnUndefinedInit {
    unresolved_mark: Mark,
}

impl UnUndefinedInit {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnUndefinedInit {
    fn visit_mut_var_decl(&mut self, decl: &mut VarDecl) {
        decl.visit_mut_children_with(self);

        if decl.kind == VarDeclKind::Const {
            return;
        }

        for declarator in &mut decl.decls {
            if !matches!(declarator.name, Pat::Ident(_)) {
                continue;
            }
            if let Some(init) = &declarator.init {
                if is_unresolved_undefined(init, self.unresolved_mark) {
                    declarator.init = None;
                }
            }
        }
    }
}
