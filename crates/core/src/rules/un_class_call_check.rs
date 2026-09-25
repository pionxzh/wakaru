use swc_core::ecma::ast::{
    Callee, Class, ClassDecl, ClassExpr, ClassMember, Constructor, Expr, Function, Ident, Module,
    ModuleItem, Pat, Stmt, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::binding_key;
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
/// class constructor. A constructor that is still a plain function does not:
/// `Foo.call(obj)` must keep throwing `Cannot call a class as a function`
/// instead of writing properties onto `obj`.
///
/// The early pipeline pass removes guards that already sit in a class
/// constructor, so `UnClassFields` sees field initializers as the leading
/// constructor statements. `UnEs6Class` and `UnPrototypeClass` copy
/// constructor bodies into class syntax, so a second pass after both of them
/// removes the guards they carried over and drops helpers with no remaining
/// references. A recovery that skips, leaving a function, keeps the guard.
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
    // require the same argument frame (`(this, <class binding>)`).
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::ClassCallCheck);
    let mut remover = CallRemover {
        helpers: &helpers,
        enclosing: Vec::new(),
        class_names: Vec::new(),
        ctor_stripped: Vec::new(),
    };
    module.visit_mut_with(&mut remover);

    if !helpers.is_empty() {
        remove_helpers_without_remaining_refs(module, helpers);
    }
}

/// The constructor a canonical guard statement names: `helper(this, Foo)` or
/// an inline helper IIFE, optionally `!`-prefixed (minification artifact).
///
/// Helper identity alone does not prove the generated argument frame: a
/// non-canonical call could carry argument side effects that removal would
/// delete. Require Babel's emitted shape, `(this, Foo)`.
pub(crate) fn class_call_check_target(
    stmt: &Stmt,
    is_helper: impl Fn(&Ident) -> bool,
) -> Option<&Ident> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let expr = expr_stmt.expr.as_ref();
    let call_expr = match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => unary.arg.as_ref(),
        _ => expr,
    };
    let Expr::Call(call) = call_expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    // A named binding proven by body shape, or an inline paren-wrapped
    // arrow/function whose body matches the helper shape.
    let helper = match callee.as_ref() {
        Expr::Ident(id) => is_helper(id),
        other => {
            classify_inline_callable(strip_parens(other))
                == Some(TranspilerHelperKind::ClassCallCheck)
        }
    };
    if !helper || call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::This(..)) {
        return None;
    }
    let Expr::Ident(ctor) = call.args[1].expr.as_ref() else {
        return None;
    };
    Some(ctor)
}

struct CallRemover<'a> {
    helpers: &'a crate::collections::HashMap<BindingKey, TranspilerHelperKind>,
    /// Innermost frame that may satisfy the second argument. A function
    /// pushes `None`: a plain function still needs its guard, and a nested
    /// function must not inherit an outer class binding. A class constructor
    /// pushes the class binding, because class syntax already rejects a call
    /// without `new`.
    enclosing: Vec<Option<BindingKey>>,
    /// Bindings naming enclosing classes.
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

    fn visit_mut_var_declarator(&mut self, decl: &mut VarDeclarator) {
        if let (Pat::Ident(name), Some(Expr::Class(_))) =
            (&decl.name, decl.init.as_deref().map(strip_parens))
        {
            self.class_names.push(binding_key(&name.id));
            decl.visit_mut_children_with(self);
            self.class_names.pop();
            return;
        }
        decl.visit_mut_children_with(self);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        self.enclosing.push(None);
        function.visit_mut_children_with(self);
        self.enclosing.pop();
    }

    fn visit_mut_class_decl(&mut self, class_decl: &mut ClassDecl) {
        self.class_names.push(binding_key(&class_decl.ident));
        class_decl.visit_mut_children_with(self);
        self.class_names.pop();
    }

    fn visit_mut_class_expr(&mut self, class_expr: &mut ClassExpr) {
        if let Some(ident) = &class_expr.ident {
            self.class_names.push(binding_key(ident));
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
        self.enclosing.push(self.class_names.last().cloned());
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
        let Some(ctor) =
            class_call_check_target(stmt, |id| self.helpers.contains_key(&binding_key(id)))
        else {
            return false;
        };
        self.enclosing
            .last()
            .and_then(Option::as_ref)
            .is_some_and(|class| *class == binding_key(ctor))
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
