use swc_core::ecma::ast::{VarDecl, VarDeclKind};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::un_nullish_coalescing::is_undefined;

/// Remove redundant `= undefined` / `= void 0` from `let` and `var` declarations.
///
/// `let x = undefined` → `let x`
///
/// `const` is excluded because it requires an initializer.
pub struct UnUndefinedInit;

impl VisitMut for UnUndefinedInit {
    fn visit_mut_var_decl(&mut self, decl: &mut VarDecl) {
        decl.visit_mut_children_with(self);

        if decl.kind == VarDeclKind::Const {
            return;
        }

        for declarator in &mut decl.decls {
            if let Some(init) = &declarator.init {
                if is_undefined(init) {
                    declarator.init = None;
                }
            }
        }
    }
}
