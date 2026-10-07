use swc_core::atoms::Atom;
use swc_core::common::util::take::Take;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, ClassDecl, ClassMember, Expr, Ident, Lit,
    MemberExpr, MemberProp, Module, ObjectLit, Pat, Prop, PropName, PropOrSpread, SeqExpr,
    SimpleAssignTarget, VarDeclarator, WithStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::eval_utils::function_observes_receiver;
use super::RewriteLevel;

use crate::analysis::{binding_id, BindingId};
use crate::collections::HashSet;
use crate::utils::paren::strip_parens;

pub struct UnIndirectCall {
    unresolved_mark: Mark,
    level: RewriteLevel,
    with_depth: usize,
    /// `(binding, member)` pairs whose member is a same-module function that
    /// reads `this`; `(0, o.m)()` calls it with an undefined receiver.
    receiver_sensitive_members: HashSet<(BindingId, Atom)>,
}

impl UnIndirectCall {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
            with_depth: 0,
            receiver_sensitive_members: HashSet::default(),
        }
    }

    /// `call_receiver_independence` covers callables the module cannot see.
    /// A local object whose method visibly reads `this` keeps the wrapper.
    fn keeps_receiverless_call(&self, inner: &Expr) -> bool {
        let Expr::Member(MemberExpr { obj, prop, .. }) = inner else {
            return false;
        };
        let Expr::Ident(object) = strip_parens(obj) else {
            return false;
        };
        let Some(name) = static_member_name(prop) else {
            return false;
        };
        object.ctxt.outer() != self.unresolved_mark
            && self
                .receiver_sensitive_members
                .contains(&(binding_id(object), name))
    }
}

impl VisitMut for UnIndirectCall {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if self.level >= RewriteLevel::Standard {
            self.receiver_sensitive_members = collect_receiver_sensitive_members(module);
        }
        module.visit_mut_children_with(self);
    }

    fn visit_mut_with_stmt(&mut self, stmt: &mut WithStmt) {
        self.with_depth += 1;
        stmt.visit_mut_children_with(self);
        self.with_depth -= 1;
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(CallExpr {
            span,
            ctxt,
            callee,
            args,
            type_args,
        }) = expr
        else {
            return;
        };

        let Callee::Expr(callee_expr) = callee else {
            return;
        };

        // Pattern 1: (0, fn)(args) → fn(args) is safe for direct identifiers,
        // except `eval` and `with`-scoped identifiers. Member calls stay
        // standard+ because they change the receiver `this` binding.
        if let Some(seq) = as_seq_expr(callee_expr) {
            let exprs = &seq.exprs;
            if exprs.len() == 2
                && matches!(&*exprs[0], Expr::Lit(Lit::Num(num)) if num.value == 0.0)
            {
                let inner = if self.level >= RewriteLevel::Standard {
                    as_member_or_safe_ident(&exprs[1], self.with_depth)
                        .filter(|inner| !self.keeps_receiverless_call(inner))
                } else {
                    as_safe_ident(&exprs[1], self.with_depth)
                };
                if let Some(inner) = inner {
                    *expr = Expr::Call(CallExpr {
                        span: *span,
                        ctxt: *ctxt,
                        callee: Callee::Expr(inner),
                        args: args.take(),
                        type_args: type_args.take(),
                    });
                }
            }
            return;
        }

        // Pattern 2: Object(fn.method)(args) → fn.method(args)
        // Object() called on a function just returns it — used as indirect call
        if self.level >= RewriteLevel::Standard {
            if let Some(inner) =
                as_object_wrap_call(callee_expr, self.unresolved_mark, self.with_depth)
                    .filter(|inner| !self.keeps_receiverless_call(inner))
            {
                *expr = Expr::Call(CallExpr {
                    span: *span,
                    ctxt: *ctxt,
                    callee: Callee::Expr(inner),
                    args: args.take(),
                    type_args: type_args.take(),
                });
            }
        }
    }
}

/// If `expr` is `Object(inner)` where inner is a member or ident expr, return `inner`.
fn as_object_wrap_call(expr: &Expr, unresolved_mark: Mark, with_depth: usize) -> Option<Box<Expr>> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    if !call.args.is_empty() && call.args.len() != 1 {
        return None;
    }
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    // Must be exactly the global `Object`
    let Expr::Ident(Ident { sym, ctxt, .. }) = strip_parens(callee_expr) else {
        return None;
    };
    if sym.as_str() != "Object" || ctxt.outer() != unresolved_mark {
        return None;
    }
    let arg = call.args.first()?;
    if arg.spread.is_some() {
        return None;
    }
    as_member_or_safe_ident(&arg.expr, with_depth)
}

fn as_seq_expr(expr: &Expr) -> Option<&SeqExpr> {
    match strip_parens(expr) {
        Expr::Seq(seq) => Some(seq),
        _ => None,
    }
}

fn as_member_or_safe_ident(expr: &Expr, with_depth: usize) -> Option<Box<Expr>> {
    match strip_parens(expr) {
        Expr::Member(_) => Some(Box::new(strip_parens(expr).clone())),
        Expr::Ident(_) => as_safe_ident(expr, with_depth),
        _ => None,
    }
}

fn as_safe_ident(expr: &Expr, with_depth: usize) -> Option<Box<Expr>> {
    let Expr::Ident(id) = strip_parens(expr) else {
        return None;
    };
    if with_depth > 0 || id.sym.as_str() == "eval" {
        return None;
    }
    Some(Box::new(Expr::Ident(id.clone())))
}

fn static_member_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(name) => Some(name.sym.clone()),
        MemberProp::Computed(computed) => match strip_parens(&computed.expr) {
            Expr::Lit(Lit::Str(name)) => name.value.as_atom().cloned(),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn static_prop_name(key: &PropName) -> Option<Atom> {
    match key {
        PropName::Ident(name) => Some(name.sym.clone()),
        PropName::Str(name) => name.value.as_atom().cloned(),
        _ => None,
    }
}

/// Members of same-module bindings that are ordinary functions reading
/// `this`: object literal methods and function values (`var o = { m() {} }`,
/// `o = { m: function () {} }`), member assignments (`o.m = function () {}`),
/// and static class methods. Arrows keep the outer `this` and are skipped.
fn collect_receiver_sensitive_members(module: &Module) -> HashSet<(BindingId, Atom)> {
    #[derive(Default)]
    struct Collector {
        members: HashSet<(BindingId, Atom)>,
    }

    impl Collector {
        fn add_object(&mut self, binding: BindingId, object: &ObjectLit) {
            for prop in &object.props {
                let PropOrSpread::Prop(prop) = prop else {
                    continue;
                };
                let (key, sensitive) = match prop.as_ref() {
                    Prop::Method(method) => {
                        (&method.key, function_observes_receiver(&method.function))
                    }
                    Prop::KeyValue(kv) => (
                        &kv.key,
                        matches!(strip_parens(&kv.value), Expr::Fn(function)
                            if function_observes_receiver(&function.function)),
                    ),
                    _ => continue,
                };
                if let Some(name) = static_prop_name(key).filter(|_| sensitive) {
                    self.members.insert((binding.clone(), name));
                }
            }
        }
    }

    impl Visit for Collector {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            if let (Pat::Ident(name), Some(Expr::Object(object))) =
                (&decl.name, decl.init.as_deref().map(strip_parens))
            {
                self.add_object(binding_id(&name.id), object);
            }
            decl.visit_children_with(self);
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign {
                match &assign.left {
                    AssignTarget::Simple(SimpleAssignTarget::Ident(name)) => {
                        if let Expr::Object(object) = strip_parens(&assign.right) {
                            self.add_object(binding_id(&name.id), object);
                        }
                    }
                    AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                        if let (Expr::Ident(object), Some(name), Expr::Fn(function)) = (
                            strip_parens(&member.obj),
                            static_member_name(&member.prop),
                            strip_parens(&assign.right),
                        ) {
                            if function_observes_receiver(&function.function) {
                                self.members.insert((binding_id(object), name));
                            }
                        }
                    }
                    _ => {}
                }
            }
            assign.visit_children_with(self);
        }

        fn visit_class_decl(&mut self, decl: &ClassDecl) {
            for member in &decl.class.body {
                let ClassMember::Method(method) = member else {
                    continue;
                };
                if !method.is_static || !function_observes_receiver(&method.function) {
                    continue;
                }
                if let Some(name) = static_prop_name(&method.key) {
                    self.members.insert((binding_id(&decl.ident), name));
                }
            }
            decl.visit_children_with(self);
        }
    }

    let mut collector = Collector::default();
    module.visit_with(&mut collector);
    collector.members
}
