//! Reads of `module.exports` in a module whose value is one assigned
//! expression.
//!
//! `module.exports = v` becomes `export default v`, and an ES module has no
//! `module`: a read of `module.exports` left in the module throws, and a
//! later `UnEsm` run takes the residual `module.exports.name` for export
//! storage, which declares an empty `name`. The pass binds the value once
//! and reads the binding instead:
//!
//! ```text
//! module.exports = { alpha(n) {…}, beta(n) { return module.exports.alpha(n); } };
//! // becomes
//! var _default = { alpha(n) {…}, beta(n) { return _default.alpha(n); } };
//! module.exports = _default;
//! ```
//!
//! and the ordinary classification turns the assignment into
//! `export default _default`. That binding holds `v` from the assignment on,
//! as `module.exports` did, so only a read that cannot run earlier is
//! rewritten: one in a function or accessor that is a property value of the
//! object literal `v` (nothing can call it while the literal is built), or
//! one in a top-level statement after the assignment and outside a function
//! declaration, which hoisting may call first. Before the assignment,
//! CommonJS reads the original `exports` object and ESM would read an
//! uninitialized binding, so one such read leaves the module as it is.
//!
//! The pass also leaves the module alone when anything may change what
//! `module.exports` holds or which properties it has: a second assignment,
//! a write, update, or `delete` through `module.exports`, `module.exports`
//! passed as a call argument (`Object.defineProperty(module.exports, …)`),
//! a use of `module` other than a static member, or a direct `eval`. A call
//! of `module.exports` itself keeps it too, since `module` is its receiver.
//! The statement-level rewrites own the written shapes.

use swc_core::ecma::ast::{FnDecl, NewExpr};

use super::*;

/// Rewrites the reads of `module.exports` as described in the module docs.
pub(super) fn bind_default_self_reads(module: &mut Module, unresolved_mark: Mark) {
    let Some(index) = default_assignment_index(module, unresolved_mark) else {
        return;
    };
    let mut direct_eval = DirectEvalPresence::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.found {
        return;
    }
    let mut reads = SelfReads {
        unresolved_mark,
        assignment: index,
        item: 0,
        in_value_function: false,
        function_decls: 0,
        reads: 0,
        rejected: false,
    };
    reads.visit_module(module);
    if reads.rejected || reads.reads == 0 {
        return;
    }

    let mut used_names = collect_all_identifier_names(module);
    let binding = fresh_binding_ident(
        fresh_prefixed_name(&Atom::from("default"), &mut used_names),
        DUMMY_SP,
    );
    let mut rewriter = SelfReadRewriter {
        unresolved_mark,
        binding: &binding,
    };
    for (item_index, item) in module.body.iter_mut().enumerate() {
        if item_index == index {
            let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
                unreachable!("the assignment index names an expression statement");
            };
            let Expr::Assign(assign) = statement.expr.as_mut() else {
                unreachable!("the assignment index names an assignment");
            };
            assign.right.visit_mut_with(&mut rewriter);
        } else {
            item.visit_mut_with(&mut rewriter);
        }
    }

    let ModuleItem::Stmt(Stmt::Expr(statement)) = module.body.remove(index) else {
        unreachable!("the assignment index names an expression statement");
    };
    let Expr::Assign(mut assign) = *statement.expr else {
        unreachable!("the assignment index names an assignment");
    };
    let value = std::mem::replace(&mut assign.right, Box::new(Expr::Ident(binding.clone())));
    let capture = ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: statement.span,
        ctxt: Default::default(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: statement.span,
            name: Pat::Ident(BindingIdent {
                id: binding,
                type_ann: None,
            }),
            init: Some(value),
            definite: false,
        }],
    }))));
    let export = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: statement.span,
        expr: Box::new(Expr::Assign(assign)),
    }));
    module.body.splice(index..index, [capture, export]);
}

/// The index of the module's one top-level `module.exports = v` statement,
/// when `v` is no identifier (the stable-default recovery reads those) and
/// is not rooted at a `require` call (a re-export or an import of it).
fn default_assignment_index(module: &Module, unresolved_mark: Mark) -> Option<usize> {
    let mut found = None;
    for (index, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            continue;
        };
        let Expr::Assign(assign) = statement.expr.as_ref() else {
            continue;
        };
        if assign.op != AssignOp::Assign
            || !matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Member(target))
                if is_module_exports_member(target, unresolved_mark))
        {
            continue;
        }
        if found.replace(index).is_some() {
            return None;
        }
    }
    let index = found?;
    let ModuleItem::Stmt(Stmt::Expr(statement)) = &module.body[index] else {
        return None;
    };
    let Expr::Assign(assign) = statement.expr.as_ref() else {
        return None;
    };
    let value = strip_parens(&assign.right);
    if matches!(value, Expr::Ident(_))
        || crate::interface_plan::is_rooted_at_require(value, unresolved_mark)
        || is_rooted_at_commonjs_exports(value, unresolved_mark)
    {
        return None;
    }
    Some(index)
}

/// `exports`, `module.exports`, or a member chain on either: a mirror of
/// another export, not a value of its own.
fn is_rooted_at_commonjs_exports(expr: &Expr, unresolved_mark: Mark) -> bool {
    if is_cjs_export_object_expr(expr, unresolved_mark) {
        return true;
    }
    match strip_parens(expr) {
        Expr::Member(member) => is_rooted_at_commonjs_exports(&member.obj, unresolved_mark),
        _ => false,
    }
}

/// `module.exports`, or a member chain on it.
fn is_rooted_at_module_exports(expr: &Expr, unresolved_mark: Mark) -> bool {
    match strip_parens(expr) {
        Expr::Member(member) => {
            is_module_exports_member(member, unresolved_mark)
                || is_rooted_at_module_exports(&member.obj, unresolved_mark)
        }
        _ => false,
    }
}

/// Counts the reads of `module.exports` and rejects the module when one may
/// run before the assignment or when anything else uses `module`.
struct SelfReads {
    unresolved_mark: Mark,
    /// The index of the `module.exports = v` statement.
    assignment: usize,
    /// The index of the top-level item being visited.
    item: usize,
    /// Inside a function that is a property value of `v`.
    in_value_function: bool,
    /// Function declarations around the visited node.
    function_decls: usize,
    reads: usize,
    rejected: bool,
}

impl SelfReads {
    fn visit_value(&mut self, value: &Expr) {
        let Expr::Object(object) = strip_parens(value) else {
            value.visit_with(self);
            return;
        };
        for prop in &object.props {
            let PropOrSpread::Prop(prop) = prop else {
                prop.visit_with(self);
                continue;
            };
            let Some((key, function)) = value_function(prop) else {
                prop.visit_with(self);
                continue;
            };
            key.visit_with(self);
            let outer = std::mem::replace(&mut self.in_value_function, true);
            match function {
                ValueFunction::Function(function) => function.visit_with(self),
                ValueFunction::Arrow(arrow) => arrow.visit_with(self),
            }
            self.in_value_function = outer;
        }
    }

    fn read(&mut self) {
        let runs_after =
            self.in_value_function || (self.item > self.assignment && self.function_decls == 0);
        if runs_after {
            self.reads += 1;
        } else {
            self.rejected = true;
        }
    }

    /// A callee may write the object it is passed
    /// (`Object.defineProperty(module.exports, …)`).
    fn reject_passed_value(&mut self, args: &[ExprOrSpread]) {
        if args
            .iter()
            .any(|arg| is_module_exports_expr(strip_parens(&arg.expr), self.unresolved_mark))
        {
            self.rejected = true;
        }
    }

    /// A call target: `module.exports(…)` supplies `module` as its
    /// receiver.
    fn visit_call_target(&mut self, target: &Expr) {
        if is_module_exports_expr(strip_parens(target), self.unresolved_mark) {
            self.rejected = true;
            return;
        }
        target.visit_with(self);
    }
}

enum ValueFunction<'a> {
    Function(&'a Function),
    Arrow(&'a ArrowExpr),
}

/// The key and the function of a property whose value is a function: a
/// method, an accessor, or a function or arrow literal.
fn value_function(prop: &Prop) -> Option<(&PropName, ValueFunction<'_>)> {
    match prop {
        Prop::Method(method) => Some((&method.key, ValueFunction::Function(&method.function))),
        Prop::Getter(getter) => Some((&getter.key, ValueFunction::Function(&getter.function))),
        Prop::Setter(setter) => Some((&setter.key, ValueFunction::Function(&setter.function))),
        Prop::KeyValue(prop) => match strip_parens(&prop.value) {
            Expr::Fn(function) => Some((&prop.key, ValueFunction::Function(&function.function))),
            Expr::Arrow(arrow) => Some((&prop.key, ValueFunction::Arrow(arrow))),
            _ => None,
        },
        _ => None,
    }
}

impl Visit for SelfReads {
    fn visit_module(&mut self, module: &Module) {
        for (index, item) in module.body.iter().enumerate() {
            self.item = index;
            if index == self.assignment {
                let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
                    unreachable!("the assignment index names an expression statement");
                };
                let Expr::Assign(assign) = statement.expr.as_ref() else {
                    unreachable!("the assignment index names an assignment");
                };
                self.visit_value(&assign.right);
            } else {
                item.visit_with(self);
            }
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.function_decls += 1;
        function.visit_children_with(self);
        self.function_decls -= 1;
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if is_module_exports_member(member, self.unresolved_mark) {
            self.read();
            return;
        }
        if let Expr::Ident(object) = member.obj.as_ref() {
            if is_unresolved_ident(object, "module", self.unresolved_mark) {
                // `module.id`, `module.hot`: no other way to the value.
                if !matches!(member.prop, MemberProp::Ident(_)) {
                    self.rejected = true;
                }
                return;
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        // A `module` that is no member object may reach `module.exports`.
        if is_unresolved_ident(ident, "module", self.unresolved_mark) {
            self.rejected = true;
        }
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::TypeOf
            && matches!(strip_parens(&unary.arg), Expr::Ident(ident)
                if is_unresolved_ident(ident, "module", self.unresolved_mark))
        {
            return;
        }
        if unary.op == UnaryOp::Delete
            && is_rooted_at_module_exports(&unary.arg, self.unresolved_mark)
        {
            self.rejected = true;
        }
        unary.visit_children_with(self);
    }

    fn visit_simple_assign_target(&mut self, target: &SimpleAssignTarget) {
        if let SimpleAssignTarget::Member(member) = target {
            if is_module_exports_member(member, self.unresolved_mark)
                || is_rooted_at_module_exports(&member.obj, self.unresolved_mark)
            {
                self.rejected = true;
            }
        }
        target.visit_children_with(self);
    }

    fn visit_pat(&mut self, pattern: &Pat) {
        if let Pat::Expr(expr) = pattern {
            if is_rooted_at_module_exports(expr, self.unresolved_mark) {
                self.rejected = true;
            }
        }
        pattern.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if is_rooted_at_module_exports(&update.arg, self.unresolved_mark) {
            self.rejected = true;
        }
        update.visit_children_with(self);
    }

    fn visit_callee(&mut self, callee: &Callee) {
        match callee {
            Callee::Expr(expr) => self.visit_call_target(expr),
            _ => callee.visit_children_with(self),
        }
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        self.visit_call_target(&call.callee);
        call.args.visit_with(self);
    }

    fn visit_tagged_tpl(&mut self, tagged: &TaggedTpl) {
        self.visit_call_target(&tagged.tag);
        tagged.tpl.visit_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        self.reject_passed_value(&call.args);
        call.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        if let Some(args) = &new.args {
            self.reject_passed_value(args);
        }
        new.visit_children_with(self);
    }
}

struct SelfReadRewriter<'a> {
    unresolved_mark: Mark,
    binding: &'a Ident,
}

impl VisitMut for SelfReadRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if is_module_exports_member(member, self.unresolved_mark) {
                let mut binding = self.binding.clone();
                binding.span = member.span;
                *expr = Expr::Ident(binding);
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
}
