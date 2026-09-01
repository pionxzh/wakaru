use swc_core::ecma::ast::{
    Callee, ClassDecl, ClassExpr, Constructor, Expr, FnDecl, FnExpr, Function, Module, ModuleItem,
    Pat, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::transpiler_helper_utils::{
    classify_inline_callable, remove_helpers_without_remaining_refs, BindingKey,
    LocalHelperContext, TranspilerHelperKind,
};
use crate::utils::paren::strip_parens;

/// Removes `_classCallCheck(this, Foo)` calls and equivalent inline IIFEs.
///
/// These are Babel transpiler artifacts for class constructors that guard against
/// calling a class without `new`. Once `UnEs6Class` recovers the class, the
/// `class` syntax carries that guard itself, so the call is redundant. Removing
/// it from a constructor that is *not* recovered as a class drops the no-`new`
/// throw; that is a recorded trade-off (tracked in the rule-correctness audit)
/// pending consumption inside the `UnEs6Class` transaction.
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
    // One pass removes both named-helper calls and inline IIFE forms; both
    // require the same argument frame (`(this, <enclosing binding>)`), so
    // they share the enclosing-frame tracking.
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::ClassCallCheck);
    let mut remover = CallRemover {
        helpers: &helpers,
        enclosing: Vec::new(),
        pending_names: Vec::new(),
        class_names: Vec::new(),
    };
    module.visit_mut_with(&mut remover);

    if !helpers.is_empty() {
        remove_helpers_without_remaining_refs(module, helpers);
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Remove calls to named classCallCheck helpers
// ---------------------------------------------------------------------------

struct CallRemover<'a> {
    helpers: &'a std::collections::HashMap<BindingKey, TranspilerHelperKind>,
    /// Bindings naming the innermost enclosing function, one frame per
    /// `Function` boundary. Babel emits `_classCallCheck(this, Foo)` only at
    /// the top of the lowered constructor `Foo`, so the second argument must
    /// resolve to one of these.
    enclosing: Vec<Vec<BindingKey>>,
    /// Names collected from a `FnDecl`/`FnExpr`/declarator wrapper, consumed
    /// by the `Function` node they wrap.
    pending_names: Vec<BindingKey>,
    /// Bindings naming enclosing classes. A residual
    /// `_classCallCheck(this, Bar)` inside `class Bar`'s recovered
    /// constructor is definitionally satisfied (a class constructor cannot be
    /// called without `new`), so the innermost class name is a valid frame
    /// for the constructor body.
    class_names: Vec<BindingKey>,
}

impl VisitMut for CallRemover<'_> {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !self.is_removable_class_call_check(stmt));
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| {
            let ModuleItem::Stmt(stmt) = item else {
                return true;
            };
            !self.is_removable_class_call_check(stmt)
        });
    }

    fn visit_mut_fn_decl(&mut self, fn_decl: &mut FnDecl) {
        self.pending_names = vec![(fn_decl.ident.sym.clone(), fn_decl.ident.ctxt)];
        fn_decl.visit_mut_children_with(self);
    }

    fn visit_mut_fn_expr(&mut self, fn_expr: &mut FnExpr) {
        // Keep a declarator name collected by visit_mut_var_declarator; the
        // inner function name (if any) shadows it inside the body, but Babel
        // output references either binding consistently, so accept both.
        if let Some(ident) = &fn_expr.ident {
            self.pending_names.push((ident.sym.clone(), ident.ctxt));
        }
        fn_expr.visit_mut_children_with(self);
    }

    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&decl.name, decl.init.as_deref()) {
            match strip_parens(init) {
                Expr::Fn(_) => {
                    self.pending_names = vec![(name.id.sym.clone(), name.id.ctxt)];
                }
                Expr::Class(_) => {
                    self.class_names.push((name.id.sym.clone(), name.id.ctxt));
                    decl.visit_mut_children_with(self);
                    self.class_names.pop();
                    return;
                }
                _ => {}
            }
        }
        decl.visit_mut_children_with(self);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let names = std::mem::take(&mut self.pending_names);
        self.enclosing.push(names);
        function.visit_mut_children_with(self);
        self.enclosing.pop();
    }

    fn visit_mut_class_decl(&mut self, class_decl: &mut ClassDecl) {
        self.class_names
            .push((class_decl.ident.sym.clone(), class_decl.ident.ctxt));
        class_decl.visit_mut_children_with(self);
        self.class_names.pop();
    }

    fn visit_mut_class_expr(&mut self, class_expr: &mut ClassExpr) {
        if let Some(ident) = &class_expr.ident {
            self.class_names.push((ident.sym.clone(), ident.ctxt));
            class_expr.visit_mut_children_with(self);
            self.class_names.pop();
        } else {
            class_expr.visit_mut_children_with(self);
        }
    }

    fn visit_mut_constructor(&mut self, ctor: &mut Constructor) {
        let names = self.class_names.last().cloned().into_iter().collect();
        self.enclosing.push(names);
        ctor.visit_mut_children_with(self);
        self.enclosing.pop();
    }
}

impl CallRemover<'_> {
    fn is_removable_class_call_check(&self, stmt: &Stmt) -> bool {
        let Stmt::Expr(expr_stmt) = stmt else {
            return false;
        };
        let expr = expr_stmt.expr.as_ref();

        // Inline IIFE forms carry an optional `!` prefix (minification
        // artifact); the named-helper form does not.
        let call_expr = match expr {
            Expr::Unary(unary) if unary.op == UnaryOp::Bang => unary.arg.as_ref(),
            _ => expr,
        };
        let Expr::Call(call) = call_expr else {
            return false;
        };
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };

        // Helper identity: a named binding proven by body shape, or an inline
        // paren-wrapped arrow/function whose body matches the helper shape.
        let is_helper = match callee.as_ref() {
            Expr::Ident(id) => self.helpers.contains_key(&(id.sym.clone(), id.ctxt)),
            other => {
                classify_inline_callable(strip_parens(other))
                    == Some(TranspilerHelperKind::ClassCallCheck)
            }
        };
        if !is_helper {
            return false;
        }

        // Helper identity alone does not prove the generated argument frame:
        // a non-canonical call could carry argument side effects that removal
        // would delete. Require Babel's emitted shape — `(this, Foo)` where
        // `Foo` is the enclosing function/class binding.
        if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
            return false;
        }
        if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
            return false;
        }
        let Expr::Ident(ctor) = call.args[1].expr.as_ref() else {
            return false;
        };
        let Some(enclosing) = self.enclosing.last() else {
            return false;
        };
        enclosing
            .iter()
            .any(|(sym, ctxt)| *sym == ctor.sym && *ctxt == ctor.ctxt)
    }
}
