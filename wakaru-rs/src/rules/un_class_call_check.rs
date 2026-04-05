use swc_core::ecma::ast::{
    ArrowExpr, BinaryOp, BlockStmtOrExpr, Callee, Expr, FnExpr, Lit, Module, ModuleItem, Pat,
    Stmt, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_class_call_check_helpers, helpers_with_remaining_refs, remove_helper_declarations,
    BindingKey,
};

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

impl VisitMut for UnClassCallCheck {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Phase 1: detect and remove module-level classCallCheck helpers
        let helpers = collect_class_call_check_helpers(module);
        if !helpers.is_empty() {
            let mut remover = CallRemover { helpers: &helpers };
            module.visit_mut_with(&mut remover);

            let remaining = helpers_with_remaining_refs(module, &helpers);
            let safe: std::collections::HashMap<BindingKey, super::babel_helper_utils::BabelHelperKind> = helpers
                .into_iter()
                .filter(|(key, _)| !remaining.contains(key))
                .collect();
            if !safe.is_empty() {
                remove_helper_declarations(&mut module.body, &safe);
            }
        }

        // Phase 2: remove inline IIFE classCallCheck patterns
        module.visit_mut_with(&mut InlineIifeRemover);
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Remove calls to named classCallCheck helpers
// ---------------------------------------------------------------------------

struct CallRemover<'a> {
    helpers: &'a std::collections::HashMap<BindingKey, super::babel_helper_utils::BabelHelperKind>,
}

impl VisitMut for CallRemover<'_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !self.is_class_call_check_stmt(stmt));
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| {
            let ModuleItem::Stmt(stmt) = item else { return true };
            !self.is_class_call_check_stmt(stmt)
        });
    }
}

impl CallRemover<'_> {
    fn is_class_call_check_stmt(&self, stmt: &Stmt) -> bool {
        let Stmt::Expr(expr_stmt) = stmt else { return false };
        let Expr::Call(call) = expr_stmt.expr.as_ref() else { return false };
        let Callee::Expr(callee) = &call.callee else { return false };
        let Expr::Ident(id) = callee.as_ref() else { return false };

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
            let ModuleItem::Stmt(stmt) = item else { return true };
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
fn is_inline_class_call_check(stmt: &Stmt) -> bool {
    let Stmt::Expr(expr_stmt) = stmt else { return false };
    let expr = expr_stmt.expr.as_ref();

    // Unwrap optional `!` prefix (minification artifact)
    let call_expr = match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => unary.arg.as_ref(),
        _ => expr,
    };

    let Expr::Call(call) = call_expr else { return false };

    // Must be called with 2 args, first is `this`
    if call.args.len() != 2 {
        return false;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
        return false;
    }

    // Callee must be a paren-wrapped arrow or function expression
    let callee_expr = match &call.callee {
        Callee::Expr(e) => e.as_ref(),
        _ => return false,
    };

    // Strip optional Paren wrapper
    let callee_inner = match callee_expr {
        Expr::Paren(p) => p.expr.as_ref(),
        other => other,
    };

    match callee_inner {
        Expr::Arrow(arrow) => is_class_call_check_arrow_body(arrow),
        Expr::Fn(fn_expr) => is_class_call_check_fn_body(fn_expr),
        _ => false,
    }
}

/// Check if an arrow function body matches the classCallCheck pattern:
/// `(e, t) => { if (!(e instanceof t)) { throw new TypeError("...") } }`
fn is_class_call_check_arrow_body(arrow: &ArrowExpr) -> bool {
    if arrow.params.len() != 2 {
        return false;
    }
    match &*arrow.body {
        BlockStmtOrExpr::BlockStmt(block) => {
            is_class_call_check_stmts(&block.stmts)
        }
        _ => false,
    }
}

/// Check if a function expression body matches the classCallCheck pattern.
fn is_class_call_check_fn_body(fn_expr: &FnExpr) -> bool {
    if fn_expr.function.params.len() != 2 {
        return false;
    }
    let Some(body) = &fn_expr.function.body else { return false };
    is_class_call_check_stmts(&body.stmts)
}

/// Check if statements match: `if (!(e instanceof t)) { throw new TypeError("...") }`
fn is_class_call_check_stmts(stmts: &[Stmt]) -> bool {
    if stmts.len() != 1 {
        return false;
    }
    let Stmt::If(if_stmt) = &stmts[0] else { return false };

    // Test: !(e instanceof t)
    let Expr::Unary(unary) = if_stmt.test.as_ref() else { return false };
    if unary.op != UnaryOp::Bang {
        return false;
    }
    // May be wrapped in parens
    let inner = match unary.arg.as_ref() {
        Expr::Paren(p) => p.expr.as_ref(),
        other => other,
    };
    if !matches!(inner, Expr::Bin(bin) if bin.op == BinaryOp::InstanceOf) {
        return false;
    }

    // Consequent must contain a throw with TypeError
    has_throw_type_error(&if_stmt.cons)
}

/// Check if a statement (or block) contains `throw new TypeError(...)`.
fn has_throw_type_error(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Throw(throw) => is_new_type_error(&throw.arg),
        Stmt::Block(block) => {
            block.stmts.len() == 1 && matches!(&block.stmts[0], Stmt::Throw(t) if is_new_type_error(&t.arg))
        }
        _ => false,
    }
}

fn is_new_type_error(expr: &Expr) -> bool {
    let Expr::New(new_expr) = expr else { return false };
    let Expr::Ident(id) = new_expr.callee.as_ref() else { return false };
    id.sym.as_ref() == "TypeError"
}
