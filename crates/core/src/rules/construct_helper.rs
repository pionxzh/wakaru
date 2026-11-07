//! Body-shape proof for the `_construct` helper that Babel 7 and swc emit for
//! `new C(...xs)` when targeting ES5.
//!
//! Babel 7.29 (producer `@babel/preset-env@7.29`):
//!
//! ```js
//! function _construct(t, e, r) {
//!     if (_isNativeReflectConstruct()) return Reflect.construct.apply(null, arguments);
//!     var o = [null]; o.push.apply(o, e);
//!     var p = new (t.bind.apply(t, o))();
//!     return r && _setPrototypeOf(p, r.prototype), p;
//! }
//! ```
//!
//! swc 1.16 (producer `@swc/core@1.16.2`), and Babel before 7.21, redefine the
//! helper on its first call:
//!
//! ```js
//! function _construct(Parent, args, Class) {
//!     if (_is_native_reflect_construct()) _construct = Reflect.construct;
//!     else _construct = function construct(Parent, args, Class) {
//!         var a = [null]; a.push.apply(a, args);
//!         var Constructor = Function.bind.apply(Parent, a);
//!         var instance = new Constructor();
//!         if (Class) _set_prototype_of(instance, Class.prototype);
//!         return instance;
//!     };
//!     return _construct.apply(null, arguments);
//! }
//! ```
//!
//! Called with two arguments, both forms construct `Parent` with the list
//! read as an array-like (`Reflect.construct`, or the bound-function
//! fallback), and the `Class` step never runs. The three-argument form, which
//! `_wrapNativeSuper` uses, sets another prototype and is not covered. The
//! native-construct check is accepted only when it calls a module-level
//! function that probes `Reflect.construct`, so dropping the call drops no
//! other effect.

use swc_core::common::{Mark, Span};
use swc_core::ecma::ast::{
    ArrowExpr, AssignOp, AssignTarget, CallExpr, Callee, Decl, Expr, ExprOrSpread, FnDecl,
    Function, Ident, Lit, MemberExpr, MemberProp, Module, ModuleItem, Pat, SimpleAssignTarget,
    Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::{BindingId, BindingUseIndex, UseKind};
use crate::collections::{HashMap, HashSet};
use crate::utils::paren::strip_parens;

use super::expr_utils::is_unresolved_ident;
use super::match_context::MatchContext;

/// The proven `_construct` helpers of a module, with the bindings their
/// bodies call (the native-construct check and the prototype setter), which
/// become removable with the helper.
#[derive(Default)]
pub(crate) struct ConstructHelpers {
    pub(crate) helpers: HashSet<BindingId>,
    pub(crate) dependencies: HashSet<BindingId>,
}

impl ConstructHelpers {
    pub(crate) fn collect(module: &Module, unresolved_mark: Mark) -> Self {
        let functions: HashMap<BindingId, &Function> = module
            .body
            .iter()
            .filter_map(|item| match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Fn(FnDecl {
                    ident, function, ..
                }))) => Some((binding_id(ident), function.as_ref())),
                _ => None,
            })
            .collect();
        let mut found = Self::default();
        let mut uses = None;
        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
                continue;
            };
            let matcher = Matcher {
                unresolved_mark,
                functions: &functions,
                helper: &fn_decl.ident,
            };
            let Some(dependencies) = matcher.matches(&fn_decl.function) else {
                continue;
            };
            let uses = uses.get_or_insert_with(|| BindingUseIndex::collect(module));
            let key = binding_id(&fn_decl.ident);
            if !uses.has_single_declaration(&key)
                || !writes_stay_inside(uses, &key, fn_decl.function.span)
            {
                continue;
            }
            found.helpers.insert(key);
            found.dependencies.extend(dependencies);
        }
        found
    }

    /// `helper(C, list)` with exactly two non-spread arguments: `C` and
    /// `list`.
    pub(crate) fn two_argument_call<'a>(
        &self,
        call: &'a CallExpr,
    ) -> Option<(&'a ExprOrSpread, &'a ExprOrSpread)> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        if !self.helpers.contains(&binding_id(callee)) {
            return None;
        }
        match call.args.as_slice() {
            [target, list] if target.spread.is_none() && list.spread.is_none() => {
                Some((target, list))
            }
            _ => None,
        }
    }
}

/// Every write of the helper binding is the self-redefinition inside its own
/// body (none for Babel 7.21 and later).
fn writes_stay_inside(uses: &BindingUseIndex, key: &BindingId, body: Span) -> bool {
    uses.use_sites(key).iter().all(|site| {
        !matches!(site.kind, UseKind::Write | UseKind::ReadWrite)
            || (!site.span.is_dummy() && body.lo <= site.span.lo && site.span.hi <= body.hi)
    })
}

struct Matcher<'a> {
    unresolved_mark: Mark,
    functions: &'a HashMap<BindingId, &'a Function>,
    helper: &'a Ident,
}

const PARAMS: [&str; 3] = ["parent", "args", "class"];

impl Matcher<'_> {
    /// The helper's dependencies when `function` is either form.
    fn matches(&self, function: &Function) -> Option<Vec<BindingId>> {
        if function.is_async || function.is_generator {
            return None;
        }
        let ctx = MatchContext::from_params(function, &PARAMS)?;
        let stmts = &function.body.as_ref()?.stmts;
        let (check, rest) = stmts.split_first()?;
        let Stmt::If(check) = check else {
            return None;
        };
        let native_check = self.native_check(&check.test)?;
        let mut dependencies = vec![native_check];
        match (check.alt.as_deref(), rest) {
            // Babel 7.21+: `if (check()) return Reflect.construct.apply(null, arguments);`
            (None, fallback) => {
                let Some(Stmt::Return(ret)) = single_stmt(&check.cons) else {
                    return None;
                };
                if !ret.arg.as_deref().is_some_and(|arg| {
                    self.is_apply_of_arguments(arg, |callee| self.is_reflect_construct(callee))
                }) {
                    return None;
                }
                dependencies.extend(self.fallback(&ctx, fallback)?);
            }
            // swc / Babel before 7.21: redefine, then call the new definition.
            (Some(alt), [Stmt::Return(ret)]) => {
                let reflect = self.self_assignment(single_stmt(&check.cons)?)?;
                if !self.is_reflect_construct(reflect) {
                    return None;
                }
                let Expr::Fn(inner) = strip_parens(self.self_assignment(single_stmt(alt)?)?) else {
                    return None;
                };
                let inner_ctx = MatchContext::from_params(&inner.function, &PARAMS)?;
                if inner.function.is_async || inner.function.is_generator {
                    return None;
                }
                dependencies
                    .extend(self.fallback(&inner_ctx, &inner.function.body.as_ref()?.stmts)?);
                if !ret.arg.as_deref().is_some_and(|arg| {
                    self.is_apply_of_arguments(arg, |callee| {
                        matches!(callee, Expr::Ident(id) if id.sym == self.helper.sym && id.ctxt == self.helper.ctxt)
                    })
                }) {
                    return None;
                }
            }
            _ => return None,
        }
        Some(dependencies)
    }

    /// `check()` where `check` is a module-level function that probes
    /// `Reflect.construct`.
    fn native_check(&self, test: &Expr) -> Option<BindingId> {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(callee),
            args,
            ..
        }) = strip_parens(test)
        else {
            return None;
        };
        let Expr::Ident(callee) = strip_parens(callee) else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        let key = binding_id(callee);
        let function = self.functions.get(&key)?;
        let mut probe = ReflectConstructProbe {
            unresolved_mark: self.unresolved_mark,
            found: false,
        };
        function.visit_with(&mut probe);
        probe.found.then_some(key)
    }

    /// `<helper> = value`: the value.
    fn self_assignment<'b>(&self, stmt: &'b Stmt) -> Option<&'b Expr> {
        let Stmt::Expr(expr) = stmt else {
            return None;
        };
        let Expr::Assign(assign) = strip_parens(&expr.expr) else {
            return None;
        };
        let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
            return None;
        };
        (assign.op == AssignOp::Assign
            && target.id.sym == self.helper.sym
            && target.id.ctxt == self.helper.ctxt)
            .then_some(&assign.right)
    }

    fn is_reflect_construct(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Member(MemberExpr { obj, prop: MemberProp::Ident(prop), .. })
            if prop.sym == "construct"
                && matches!(strip_parens(obj), Expr::Ident(id) if is_unresolved_ident(id, "Reflect", self.unresolved_mark)))
    }

    /// `callee.apply(null, arguments)`.
    fn is_apply_of_arguments(&self, expr: &Expr, callee: impl Fn(&Expr) -> bool) -> bool {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(apply),
            args,
            ..
        }) = strip_parens(expr)
        else {
            return false;
        };
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) = strip_parens(apply)
        else {
            return false;
        };
        prop.sym == "apply"
            && callee(strip_parens(obj))
            && matches!(args.as_slice(), [this, list]
                if this.spread.is_none()
                    && list.spread.is_none()
                    && matches!(strip_parens(&this.expr), Expr::Lit(Lit::Null(_)))
                    && matches!(strip_parens(&list.expr), Expr::Ident(id) if is_unresolved_ident(id, "arguments", self.unresolved_mark)))
    }

    /// The fallback after the native branch:
    ///
    /// ```js
    /// var a = [null]; a.push.apply(a, args);
    /// var instance = new (BIND(parent, a))();   // or: var K = BIND(parent, a); var instance = new K();
    /// if (class) …;                             // or: class && …;
    /// return instance;                          // or: return class && …, instance;
    /// ```
    ///
    /// `BIND(parent, a)` is `parent.bind.apply(parent, a)` or the global
    /// `Function.bind.apply(parent, a)` / `Function.prototype.bind.apply`.
    /// Returns the bindings the class step calls.
    fn fallback(&self, ctx: &MatchContext, stmts: &[Stmt]) -> Option<Vec<BindingId>> {
        let mut ctx = ctx.clone();
        let mut rest = stmts;

        // var a = [null];
        let (list, tail) = rest.split_first()?;
        let (list_binding, init) = single_var(list)?;
        if !matches!(strip_parens(init), Expr::Array(array)
            if matches!(array.elems.as_slice(), [Some(ExprOrSpread { spread: None, expr })]
                if matches!(strip_parens(expr), Expr::Lit(Lit::Null(_)))))
        {
            return None;
        }
        ctx.declare("list", list_binding.sym.clone(), list_binding.ctxt);
        rest = tail;

        // a.push.apply(a, args);
        let (push, tail) = rest.split_first()?;
        let Stmt::Expr(push) = push else {
            return None;
        };
        if !self.is_push_apply(&push.expr, &ctx) {
            return None;
        }
        rest = tail;

        // var instance = new (BIND(parent, a))();  |  var K = BIND(…); var instance = new K();
        let (first, tail) = rest.split_first()?;
        let (binding, init) = single_var(first)?;
        let instance = if self.is_bind_apply(init, &ctx) {
            ctx.declare("bound", binding.sym.clone(), binding.ctxt);
            let (second, tail) = tail.split_first()?;
            let (instance, init) = single_var(second)?;
            if !is_construct_without_arguments(init, |callee| ctx.is_binding(callee, "bound")) {
                return None;
            }
            rest = tail;
            instance
        } else if is_construct_without_arguments(init, |callee| self.is_bind_apply(callee, &ctx)) {
            rest = tail;
            binding
        } else {
            return None;
        };
        ctx.declare("instance", instance.sym.clone(), instance.ctxt);

        let mut dependencies = Vec::new();
        match rest {
            [class_step, Stmt::Return(ret)] => {
                dependencies.extend(self.class_step_stmt(class_step, &ctx)?);
                if !ret
                    .arg
                    .as_deref()
                    .is_some_and(|arg| ctx.is_binding(strip_parens(arg), "instance"))
                {
                    return None;
                }
            }
            [Stmt::Return(ret)] => {
                let Expr::Seq(seq) = strip_parens(ret.arg.as_deref()?) else {
                    return None;
                };
                let [class_step, result] = seq.exprs.as_slice() else {
                    return None;
                };
                dependencies.extend(self.class_step_expr(class_step, &ctx)?);
                if !ctx.is_binding(strip_parens(result), "instance") {
                    return None;
                }
            }
            _ => return None,
        }
        Some(dependencies)
    }

    /// `a.push.apply(a, args)`.
    fn is_push_apply(&self, expr: &Expr, ctx: &MatchContext) -> bool {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(callee),
            args,
            ..
        }) = strip_parens(expr)
        else {
            return false;
        };
        let Some(push) = static_member(callee, "apply") else {
            return false;
        };
        static_member(push, "push").is_some_and(|list| ctx.is_binding(list, "list"))
            && matches!(args.as_slice(), [this, source]
                if this.spread.is_none()
                    && source.spread.is_none()
                    && ctx.is_binding(strip_parens(&this.expr), "list")
                    && ctx.is_binding(strip_parens(&source.expr), "args"))
    }

    /// `parent.bind.apply(parent, a)`, `Function.bind.apply(parent, a)` or
    /// `Function.prototype.bind.apply(parent, a)`.
    fn is_bind_apply(&self, expr: &Expr, ctx: &MatchContext) -> bool {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(callee),
            args,
            ..
        }) = strip_parens(expr)
        else {
            return false;
        };
        let Some(bind) = static_member(callee, "apply") else {
            return false;
        };
        let Some(owner) = static_member(bind, "bind") else {
            return false;
        };
        let is_function = |expr: &Expr| matches!(expr, Expr::Ident(id) if is_unresolved_ident(id, "Function", self.unresolved_mark));
        let owner_matches = ctx.is_binding(owner, "parent")
            || is_function(owner)
            || static_member(owner, "prototype").is_some_and(is_function);
        owner_matches
            && matches!(args.as_slice(), [target, list]
                if target.spread.is_none()
                    && list.spread.is_none()
                    && ctx.is_binding(strip_parens(&target.expr), "parent")
                    && ctx.is_binding(strip_parens(&list.expr), "list"))
    }

    /// `if (class) …` or `class && …`, as a statement: the guarded part runs
    /// only with a third argument.
    fn class_step_stmt(&self, stmt: &Stmt, ctx: &MatchContext) -> Option<Vec<BindingId>> {
        match stmt {
            Stmt::If(if_stmt) if if_stmt.alt.is_none() => {
                if !ctx.is_binding(strip_parens(&if_stmt.test), "class") {
                    return None;
                }
                let Some(Stmt::Expr(expr)) = single_stmt(&if_stmt.cons) else {
                    return None;
                };
                Some(called_bindings(&expr.expr))
            }
            Stmt::Expr(expr) => self.class_step_expr(&expr.expr, ctx),
            _ => None,
        }
    }

    fn class_step_expr(&self, expr: &Expr, ctx: &MatchContext) -> Option<Vec<BindingId>> {
        let Expr::Bin(bin) = strip_parens(expr) else {
            return None;
        };
        (bin.op == swc_core::ecma::ast::BinaryOp::LogicalAnd
            && ctx.is_binding(strip_parens(&bin.left), "class"))
        .then(|| called_bindings(&bin.right))
    }
}

/// `new callee()` or `new callee`.
fn is_construct_without_arguments(expr: &Expr, callee: impl Fn(&Expr) -> bool) -> bool {
    matches!(strip_parens(expr), Expr::New(new)
        if new.args.as_ref().is_none_or(Vec::is_empty) && callee(strip_parens(&new.callee)))
}

/// `var x = init;` with one declarator.
fn single_var(stmt: &Stmt) -> Option<(&Ident, &Expr)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    let VarDecl { kind, decls, .. } = var.as_ref();
    if *kind != VarDeclKind::Var {
        return None;
    }
    let [decl] = decls.as_slice() else {
        return None;
    };
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    Some((&binding.id, decl.init.as_deref()?))
}

/// A statement, or the one statement of a block.
fn single_stmt(stmt: &Stmt) -> Option<&Stmt> {
    match stmt {
        Stmt::Block(block) => match block.stmts.as_slice() {
            [stmt] => single_stmt(stmt),
            _ => None,
        },
        stmt => Some(stmt),
    }
}

/// The object of `<object>.<name>`.
fn static_member<'a>(expr: &'a Expr, name: &str) -> Option<&'a Expr> {
    match strip_parens(expr) {
        Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) if prop.sym == name => Some(strip_parens(obj)),
        _ => None,
    }
}

/// Identifier callees in `expr`, outside nested functions.
fn called_bindings(expr: &Expr) -> Vec<BindingId> {
    struct Callees(Vec<BindingId>);
    impl Visit for Callees {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(id) = strip_parens(callee) {
                    self.0.push(binding_id(id));
                }
            }
            call.visit_children_with(self);
        }
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    }
    let mut callees = Callees(Vec::new());
    expr.visit_with(&mut callees);
    callees.0
}

/// A `Reflect.construct(…)` call on the global `Reflect`.
struct ReflectConstructProbe {
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for ReflectConstructProbe {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(MemberExpr {
                obj,
                prop: MemberProp::Ident(prop),
                ..
            }) = strip_parens(callee)
            {
                if prop.sym == "construct"
                    && matches!(strip_parens(obj), Expr::Ident(id) if is_unresolved_ident(id, "Reflect", self.unresolved_mark))
                {
                    self.found = true;
                }
            }
        }
        call.visit_children_with(self);
    }
}
