use swc_core::ecma::ast::{Callee, Expr, Module, ModuleItem, Stmt, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::transpiler_helper_utils::{
    classify_inline_callable, remove_helpers_without_remaining_refs, BindingKey,
    LocalHelperContext, TranspilerHelperKind,
};
use crate::utils::paren::strip_parens;

/// Removes `_classCallCheck(this, Foo)` calls and equivalent inline IIFEs.
///
/// These are Babel transpiler artifacts for class constructors that guard against
/// calling a class without `new`. Since we're decompiling (not running), these
/// guards are pure noise.
///
/// Handles two forms:
/// 1. Named function: `_classCallCheck(this, Foo)` where the function is declared
///    at module level with the classCallCheck body shape.
/// 2. Inline IIFE: `!((e, t) => { if (!(e instanceof t)) throw TypeError(...) })(this, Foo)`
pub struct UnClassCallCheck;

impl UnClassCallCheck {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_class_call_check(module, local_helpers);
    }
}

impl VisitMut for UnClassCallCheck {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_class_call_check(module, &local_helpers);
    }
}

fn run_un_class_call_check(module: &mut Module, local_helpers: &LocalHelperContext) {
    // Phase 1: detect and remove module-level classCallCheck helpers
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::ClassCallCheck);
    if !helpers.is_empty() {
        let mut remover = CallRemover { helpers: &helpers };
        module.visit_mut_with(&mut remover);

        remove_helpers_without_remaining_refs(module, helpers);
    }

    // Phase 2: remove inline IIFE classCallCheck patterns
    module.visit_mut_with(&mut InlineIifeRemover);
}

// ---------------------------------------------------------------------------
// Phase 1: Remove calls to named classCallCheck helpers
// ---------------------------------------------------------------------------

struct CallRemover<'a> {
    helpers: &'a std::collections::HashMap<BindingKey, TranspilerHelperKind>,
}

impl VisitMut for CallRemover<'_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !self.is_class_call_check_stmt(stmt));
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| {
            let ModuleItem::Stmt(stmt) = item else {
                return true;
            };
            !self.is_class_call_check_stmt(stmt)
        });
    }
}

impl CallRemover<'_> {
    fn is_class_call_check_stmt(&self, stmt: &Stmt) -> bool {
        let Stmt::Expr(expr_stmt) = stmt else {
            return false;
        };
        let Expr::Call(call) = expr_stmt.expr.as_ref() else {
            return false;
        };
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return false;
        };

        let key = (id.sym.clone(), id.ctxt);
        self.helpers.contains_key(&key)
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Remove inline IIFE classCallCheck patterns
// ---------------------------------------------------------------------------

struct InlineIifeRemover;

impl VisitMut for InlineIifeRemover {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !is_inline_class_call_check(stmt));
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| {
            let ModuleItem::Stmt(stmt) = item else {
                return true;
            };
            !is_inline_class_call_check(stmt)
        });
    }
}

/// Check if a statement is an inline classCallCheck IIFE.
///
/// Matches:
/// - `!((e, t) => { if (!(e instanceof t)) throw TypeError(...) })(this, Foo)`
/// - `((e, t) => { if (!(e instanceof t)) throw TypeError(...) })(this, Foo)`
/// - Same with `function` expression instead of arrow
///
/// The body-shape recognition is delegated to the shared helper-detection
/// module ([`classify_inline_callable`]); this function only validates the
/// call-site framing (optional `!`, two args, first is `this`).
fn is_inline_class_call_check(stmt: &Stmt) -> bool {
    let Stmt::Expr(expr_stmt) = stmt else {
        return false;
    };
    let expr = expr_stmt.expr.as_ref();

    // Unwrap optional `!` prefix (minification artifact)
    let call_expr = match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => unary.arg.as_ref(),
        _ => expr,
    };

    let Expr::Call(call) = call_expr else {
        return false;
    };

    // Must be called with 2 args, first is `this`
    if call.args.len() != 2 {
        return false;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
        return false;
    }

    // Callee must be a paren-wrapped arrow or function expression whose body
    // matches the classCallCheck shape.
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };

    classify_inline_callable(strip_parens(callee)) == Some(TranspilerHelperKind::ClassCallCheck)
}
