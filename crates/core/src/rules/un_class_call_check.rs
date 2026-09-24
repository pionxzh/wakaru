use swc_core::ecma::ast::{
    Callee, Class, ClassDecl, ClassExpr, ClassMember, Constructor, Expr, FnDecl, FnExpr, Function,
    Module, ModuleItem, Pat, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::transpiler_helper_utils::{
    classify_inline_callable, remove_helpers_without_remaining_refs, BindingKey,
    LocalHelperContext, TranspilerHelperKind,
};
use crate::utils::paren::strip_parens;

/// Removes `_classCallCheck(this, Foo)` calls and equivalent inline IIFEs
/// only after the constructor is class syntax.
///
/// These are Babel transpiler artifacts that guard against calling a class
/// without `new`. Class syntax carries that guard itself (`Class constructor
/// Foo cannot be invoked without 'new'`), so the call is redundant inside a
/// recovered constructor. A constructor that is still a plain function does
/// not: `Foo.call(obj)` must keep throwing `Cannot call a class as a function`
/// instead of writing properties onto `obj`.
///
/// The early pipeline pass therefore deletes the call only when it already
/// sits in a `class` constructor. `UnEs6Class` and `UnPrototypeClass` clone
/// constructor bodies, so a later pass (after both of them) deletes guards
/// that those recoveries copied into class syntax and drops helpers that no
/// longer have references. A recovery that skips, leaving a function, keeps
/// the guard.
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
        ctor_stripped: Vec::new(),
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
    helpers: &'a crate::collections::HashMap<BindingKey, TranspilerHelperKind>,
    /// Innermost frame that may satisfy the second argument. Function
    /// boundaries push an empty frame so a nested function does not inherit a
    /// class constructor binding. Only a class constructor frame carries a
    /// name: class syntax already rejects a call without `new`.
    enclosing: Vec<Vec<BindingKey>>,
    /// Names collected from a declarator or function wrapper. Function nodes
    /// discard them. Class constructor nodes do not use this vector.
    pending_names: Vec<BindingKey>,
    /// Bindings naming enclosing classes. A residual
    /// `_classCallCheck(this, Bar)` inside `class Bar`'s recovered
    /// constructor is definitionally satisfied (a class constructor cannot be
    /// called without `new`), so the innermost class name is a valid frame
    /// for the constructor body.
    class_names: Vec<BindingKey>,
    /// One flag per class currently being visited. Set when this pass deleted
    /// a classCallCheck statement from that class's constructor.
    ctor_stripped: Vec<bool>,
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
        // A plain function still needs the guard. Discard any name collected
        // for this function and push an empty frame so a nested function
        // cannot match an outer class constructor binding.
        self.pending_names.clear();
        self.enclosing.push(Vec::new());
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

    fn visit_mut_class(&mut self, class: &mut Class) {
        self.ctor_stripped.push(false);
        class.visit_mut_children_with(self);
        let stripped = self.ctor_stripped.pop().unwrap_or(false);
        if !stripped {
            return;
        }
        let derived = class.super_class.is_some();
        class.body.retain(|member| match member {
            ClassMember::Constructor(ctor) => {
                !constructor_omittable_after_guard_removal(ctor, derived)
            }
            _ => true,
        });
    }

    fn visit_mut_constructor(&mut self, ctor: &mut Constructor) {
        let names = self.class_names.last().cloned().into_iter().collect();
        self.enclosing.push(names);
        let before = ctor.body.as_ref().map(|body| body.stmts.len()).unwrap_or(0);
        ctor.visit_mut_children_with(self);
        self.enclosing.pop();
        let after = ctor.body.as_ref().map(|body| body.stmts.len()).unwrap_or(0);
        if after < before {
            if let Some(flag) = self.ctor_stripped.last_mut() {
                *flag = true;
            }
        }
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
        // `Foo` is the enclosing class binding. A function name is not enough:
        // removing the guard there drops the no-`new` throw.
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

/// A constructor may disappear only when deleting the guard left the implicit
/// form. `super()` does not forward arguments. A derived empty body still
/// throws because `super` was not called. Parameter lists stay, including an
/// empty body. A missing body is a TypeScript signature and is never ours.
pub(crate) fn constructor_omittable_after_guard_removal(ctor: &Constructor, derived: bool) -> bool {
    if !ctor.params.is_empty() {
        return false;
    }
    let Some(body) = &ctor.body else {
        return false;
    };
    if !derived && body.stmts.is_empty() {
        return true;
    }
    if !derived || body.stmts.len() != 1 {
        return false;
    }
    let Stmt::Expr(expr_stmt) = &body.stmts[0] else {
        return false;
    };
    let Expr::Call(call) = expr_stmt.expr.as_ref() else {
        return false;
    };
    if !matches!(&call.callee, Callee::Super(_)) || call.args.len() != 1 {
        return false;
    }
    let arg = &call.args[0];
    arg.spread.is_some()
        && matches!(arg.expr.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "arguments")
}
