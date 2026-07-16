use std::collections::HashSet;

use swc_core::common::util::take::Take;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{Callee, Expr, FnDecl, Module, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{
    binding_key, remaining_refs_outside_declarations, remove_fn_decls_by_binding,
    remove_import_specifiers_by_binding, remove_var_declarators_by_binding, BindingKey,
};
use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};

/// Detects and simplifies Babel's `_typeof` polyfill.
///
/// Pattern:
/// ```js
/// var _typeof = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
///     ? function(e) { return typeof e; }
///     : function(e) { /* Symbol polyfill */ return typeof e; };
/// ```
/// Also recognizes Babel/SWC's cached declaration form, where the helper
/// assigns that conditional back to its own binding and calls itself once.
///
/// All calls `_typeof(expr)` are replaced with `typeof expr`, and the
/// polyfill declaration is removed.
pub struct UnTypeofPolyfill;

impl UnTypeofPolyfill {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_typeof_polyfill(module, local_helpers);
    }
}

impl VisitMut for UnTypeofPolyfill {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_typeof_polyfill(module, &local_helpers);
    }
}

fn run_un_typeof_polyfill(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers = local_helpers
        .helpers_of_kind(TranspilerHelperKind::Typeof)
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    if helpers.is_empty() {
        return;
    }

    let mut replacer = TypeofReplacer { helpers: &helpers };
    module.visit_mut_with(&mut replacer);

    // Remove declarations if no remaining references
    let remaining = remaining_refs_outside_declarations(module, &helpers, &helpers);
    let safe_to_remove: HashSet<BindingKey> = helpers.difference(&remaining).cloned().collect();
    if !safe_to_remove.is_empty() {
        remove_fn_decls_by_binding(module, &safe_to_remove);
        remove_var_declarators_by_binding(&mut module.body, &safe_to_remove);
        remove_import_specifiers_by_binding(&mut module.body, &safe_to_remove);
    }
}

// ---------------------------------------------------------------------------
// Replacement: _typeof(expr) → typeof expr
// ---------------------------------------------------------------------------

struct TypeofReplacer<'a> {
    helpers: &'a HashSet<BindingKey>,
}

impl VisitMut for TypeofReplacer<'_> {
    fn visit_mut_fn_decl(&mut self, function: &mut FnDecl) {
        // A retained self-caching helper must keep its internal recursive call.
        // Only rewrite call sites outside declarations that were classified as
        // helpers; declaration cleanup below removes the body when it is dead.
        if self.helpers.contains(&binding_key(&function.ident)) {
            return;
        }
        function.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return;
        };

        let key = binding_key(id);
        if !self.helpers.contains(&key) {
            return;
        }

        // Must be a single-arg call: _typeof(expr)
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return;
        }

        *expr = Expr::Unary(UnaryExpr {
            span: DUMMY_SP,
            op: UnaryOp::TypeOf,
            arg: call.args[0].expr.take(),
        });
    }
}
