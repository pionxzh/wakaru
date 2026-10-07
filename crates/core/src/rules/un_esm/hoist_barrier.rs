//! Keeps a `require` in place when an earlier top-level statement has an
//! effect that the provider can observe (`import_hoisting_eagerness`).
//!
//! An `import` evaluates its provider before the module body, but a
//! `require` runs it at the call. When an earlier statement writes a global
//! (`window.fetch = spy`), runs another provider through a require that stays
//! a call (`require("dotenv").config()`), or calls into a required module and
//! discards the result (`polyfill.install()`), hoisting a later provider
//! above it changes what that provider sees. After the first such statement,
//! every top-level `require` stays a call: UnEsm then sees no `require`
//! there, so the module may still become ESM with those calls left in it.
//!
//! Other statements before a require are not barriers: declarations,
//! function and class definitions, export plumbing, and calls whose results
//! are kept (`var x = lib.make()`) or that go through local helpers. A
//! numeric-id require of a module outside the input stays a call by itself
//! and is not a barrier either.

use crate::collections::HashSet;

use swc_core::common::{Mark, SyntaxContext};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, CallExpr, Callee, Class, Decl, Expr, Function, Ident,
    ImportSpecifier, Lit, MemberExpr, Module, ModuleDecl, ModuleItem, Pat, Prop, PropOrSpread,
    SimpleAssignTarget, Stmt, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::BindingId;
use crate::utils::paren::strip_parens;

/// Hides every top-level `require` after the first hoisting barrier by giving
/// the callee a fresh context. Returns that context for
/// [`restore_requires`], or `None` when the module has no barrier before a
/// require.
pub(super) fn hide_requires_after_barrier(
    module: &mut Module,
    unresolved_mark: Mark,
) -> Option<SyntaxContext> {
    let mut required = HashSet::default();
    let mut first_barrier = None;
    for (index, item) in module.body.iter().enumerate() {
        let stmt = match item {
            ModuleItem::Stmt(stmt) => stmt,
            // A require an earlier run turned into an import is still a
            // required binding: the next run must find the same barrier.
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                required.extend(import.specifiers.iter().map(|specifier| match specifier {
                    ImportSpecifier::Named(named) => binding_id(&named.local),
                    ImportSpecifier::Default(default) => binding_id(&default.local),
                    ImportSpecifier::Namespace(namespace) => binding_id(&namespace.local),
                }));
                continue;
            }
            ModuleItem::ModuleDecl(_) => continue,
        };
        if is_barrier(stmt, unresolved_mark, &mut required) {
            first_barrier = Some(index);
            break;
        }
    }
    let first_barrier = first_barrier?;
    let mut hide = HideRequire {
        unresolved_mark,
        hidden: SyntaxContext::empty().apply_mark(Mark::new()),
        found: false,
    };
    // The barrier statement itself is left to UnEsm: a require it contains
    // either stays a call or is hoisted out of a shape UnEsm recovers.
    for item in &mut module.body[first_barrier + 1..] {
        if let ModuleItem::Stmt(stmt) = item {
            stmt.visit_mut_with(&mut hide);
        }
    }
    hide.found.then_some(hide.hidden)
}

/// Gives the hidden `require` callees back their unresolved context, so later
/// rules and the next UnEsm run see ordinary requires.
pub(super) fn restore_requires(module: &mut Module, hidden: SyntaxContext, unresolved_mark: Mark) {
    struct Restore {
        hidden: SyntaxContext,
        unresolved: SyntaxContext,
    }

    impl VisitMut for Restore {
        fn visit_mut_ident(&mut self, ident: &mut Ident) {
            if ident.ctxt == self.hidden {
                ident.ctxt = self.unresolved;
            }
        }
    }

    module.visit_mut_with(&mut Restore {
        hidden,
        unresolved: SyntaxContext::empty().apply_mark(unresolved_mark),
    });
}

struct HideRequire {
    unresolved_mark: Mark,
    hidden: SyntaxContext,
    found: bool,
}

impl VisitMut for HideRequire {
    // A `require` inside a function never becomes an import.
    fn visit_mut_function(&mut self, _: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}

    fn visit_mut_class(&mut self, _: &mut Class) {}

    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);
        if let Callee::Expr(callee) = &mut call.callee {
            if let Expr::Ident(id) = &mut **callee {
                if is_unresolved(id, "require", self.unresolved_mark) {
                    id.ctxt = self.hidden;
                    self.found = true;
                }
            }
        }
    }
}

fn is_unresolved(id: &Ident, name: &str, unresolved_mark: Mark) -> bool {
    id.sym == name && id.ctxt.outer() == unresolved_mark
}

/// `Some(true)` for `require("literal")`, `Some(false)` for any other
/// `require(...)` call.
fn require_call_kind(expr: &Expr, unresolved_mark: Mark) -> Option<bool> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = strip_parens(callee) else {
        return None;
    };
    if !is_unresolved(id, "require", unresolved_mark) {
        return None;
    }
    Some(matches!(call.args.as_slice(), [arg]
            if arg.spread.is_none() && matches!(&*arg.expr, Expr::Lit(Lit::Str(_)))))
}

/// `require("x")`, `require("x").a.b`, or `helper(require("x"))`: the shapes
/// UnEsm turns into an import.
fn is_require_like(expr: &Expr, unresolved_mark: Mark) -> bool {
    let mut current = strip_parens(expr);
    while let Expr::Member(member) = current {
        current = strip_parens(&member.obj);
    }
    if require_call_kind(current, unresolved_mark) == Some(true) {
        return true;
    }
    let Expr::Call(call) = current else {
        return false;
    };
    call.args.first().is_some_and(|first| {
        first.spread.is_none() && require_call_kind(&first.expr, unresolved_mark) == Some(true)
    })
}

/// Evaluating the expression runs no code.
fn is_inert(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Lit(_)
        | Expr::Ident(_)
        | Expr::This(_)
        | Expr::Fn(_)
        | Expr::Arrow(_)
        | Expr::Class(_) => true,
        Expr::Member(member) => is_inert(&member.obj),
        Expr::Unary(unary) => unary.op != UnaryOp::Delete && is_inert(&unary.arg),
        Expr::Bin(binary) => is_inert(&binary.left) && is_inert(&binary.right),
        Expr::Cond(cond) => is_inert(&cond.test) && is_inert(&cond.cons) && is_inert(&cond.alt),
        Expr::Seq(sequence) => sequence.exprs.iter().all(|expr| is_inert(expr)),
        Expr::Tpl(tpl) => tpl.exprs.iter().all(|expr| is_inert(expr)),
        Expr::Array(array) => array
            .elems
            .iter()
            .flatten()
            .all(|elem| is_inert(&elem.expr)),
        Expr::Object(object) => object.props.iter().all(|prop| match prop {
            PropOrSpread::Spread(spread) => is_inert(&spread.expr),
            PropOrSpread::Prop(prop) => match prop.as_ref() {
                Prop::KeyValue(key_value) => is_inert(&key_value.value),
                _ => true,
            },
        }),
        _ => false,
    }
}

/// The root of a member or call chain: `a` in `a.b.c()` and `a.b().c`.
fn chain_root(expr: &Expr) -> &Expr {
    let mut current = strip_parens(expr);
    loop {
        match current {
            Expr::Member(member) => current = strip_parens(&member.obj),
            Expr::Call(CallExpr {
                callee: Callee::Expr(callee),
                ..
            }) => current = strip_parens(callee),
            _ => return current,
        }
    }
}

fn mentions_commonjs_object(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    struct Finder {
        unresolved_mark: Mark,
        found: bool,
    }

    impl Visit for Finder {
        fn visit_ident(&mut self, id: &Ident) {
            if id.ctxt.outer() == self.unresolved_mark
                && matches!(id.sym.as_ref(), "exports" | "module")
            {
                self.found = true;
            }
        }
    }

    let mut finder = Finder {
        unresolved_mark,
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

fn is_export_target(target: &AssignTarget, unresolved_mark: Mark) -> bool {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return false;
    };
    match strip_parens(&member.obj) {
        Expr::Ident(id) => {
            is_unresolved(id, "exports", unresolved_mark)
                || is_unresolved(id, "module", unresolved_mark)
        }
        Expr::Member(inner) => matches!(strip_parens(&inner.obj), Expr::Ident(id)
            if is_unresolved(id, "module", unresolved_mark)),
        _ => false,
    }
}

/// Effects a statement runs at module evaluation, outside nested function
/// and class bodies.
#[derive(Default)]
struct Effects {
    /// A `require("literal")` that stays a call. In a declaration or an
    /// expression statement that is one whose result is called
    /// (`require("dotenv").config()`, `require("x")(a)`); UnEsm hoists a
    /// require read as a value (`f(require("x").default)`) into an import.
    /// In any other statement (`if`, loops, `try`) every require stays.
    leftover_require: bool,
    /// A write to a global (`window.fetch = f`, `process.env.X = v`, `g = 1`).
    global_write: bool,
}

struct EffectScan<'a> {
    unresolved_mark: Mark,
    /// Count every string require, not only called ones.
    in_control_flow: bool,
    effects: &'a mut Effects,
}

impl EffectScan<'_> {
    fn is_global_member(&self, member: &MemberExpr) -> bool {
        matches!(chain_root(&member.obj), Expr::Ident(id)
            if id.ctxt.outer() == self.unresolved_mark
                && !matches!(id.sym.as_ref(), "exports" | "module" | "require"))
    }
}

impl Visit for EffectScan<'_> {
    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &Class) {}

    fn visit_call_expr(&mut self, call: &CallExpr) {
        let leftover = if self.in_control_flow {
            is_string_require_call(call, self.unresolved_mark)
        } else {
            matches!(&call.callee, Callee::Expr(callee)
                if matches!(chain_root(callee), Expr::Call(inner)
                    if is_string_require_call(inner, self.unresolved_mark)))
        };
        if leftover {
            self.effects.leftover_require = true;
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let global_target = match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                self.is_global_member(member)
            }
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                binding.id.ctxt.outer() == self.unresolved_mark
            }
            _ => false,
        };
        self.effects.global_write |= global_target;
        assign.visit_children_with(self);
    }
}

fn scan_effects(stmt: &Stmt, unresolved_mark: Mark) -> Effects {
    let mut effects = Effects::default();
    stmt.visit_with(&mut EffectScan {
        unresolved_mark,
        in_control_flow: !matches!(stmt, Stmt::Expr(_) | Stmt::Decl(_)),
        effects: &mut effects,
    });
    effects
}

fn is_string_require_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    matches!(&call.callee, Callee::Expr(callee) if matches!(strip_parens(callee), Expr::Ident(id)
        if is_unresolved(id, "require", unresolved_mark)))
        && matches!(call.args.as_slice(), [arg]
            if arg.spread.is_none() && matches!(&*arg.expr, Expr::Lit(Lit::Str(_))))
}

/// An expression statement whose call result is discarded and whose callee
/// goes through a required binding or a require: `lib.install();`,
/// `a(), lib.setup()`.
fn is_discarded_required_call(
    expr: &Expr,
    unresolved_mark: Mark,
    required: &HashSet<BindingId>,
) -> bool {
    let elements: Vec<&Expr> = match strip_parens(expr) {
        Expr::Seq(sequence) => sequence
            .exprs
            .iter()
            .map(|expr| strip_parens(expr))
            .collect(),
        other => vec![other],
    };
    elements.into_iter().any(|element| {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(callee),
            ..
        }) = element
        else {
            return false;
        };
        is_require_like(callee, unresolved_mark)
            || matches!(chain_root(callee), Expr::Ident(id) if required.contains(&binding_id(id)))
    })
}

/// Whether `stmt` is a hoisting barrier. Records the bindings of require
/// declarations in `required` as it goes.
fn is_barrier(stmt: &Stmt, unresolved_mark: Mark, required: &mut HashSet<BindingId>) -> bool {
    let effects_are_barrier = |effects: Effects| effects.leftover_require || effects.global_write;
    match stmt {
        Stmt::Decl(Decl::Fn(_) | Decl::Class(_)) | Stmt::Empty(_) => false,
        Stmt::Decl(Decl::Var(var)) => {
            let mut all_known = true;
            for decl in &var.decls {
                match decl.init.as_deref() {
                    None => {}
                    Some(init) if is_require_like(init, unresolved_mark) => {
                        if let Pat::Ident(name) = &decl.name {
                            required.insert(binding_id(&name.id));
                        }
                    }
                    Some(init) if is_inert(init) => {}
                    Some(_) => all_known = false,
                }
            }
            !all_known && effects_are_barrier(scan_effects(stmt, unresolved_mark))
        }
        Stmt::Expr(expr_stmt) => {
            let expr = strip_parens(&expr_stmt.expr);
            if matches!(expr, Expr::Lit(_))
                || require_call_kind(expr, unresolved_mark) == Some(true)
            {
                return false;
            }
            if let Expr::Assign(assign) = expr {
                if is_export_target(&assign.left, unresolved_mark) {
                    let mut value = strip_parens(&assign.right);
                    while let Expr::Assign(inner) = value {
                        if !is_export_target(&inner.left, unresolved_mark) {
                            break;
                        }
                        value = strip_parens(&inner.right);
                    }
                    if is_require_like(value, unresolved_mark) || is_inert(value) {
                        return false;
                    }
                    return effects_are_barrier(scan_effects(stmt, unresolved_mark));
                }
            }
            // Export plumbing: `Object.defineProperty(exports, ...)`,
            // `__exportStar(require("x"), exports)`, `require.d(exports, ...)`.
            if matches!(expr, Expr::Call(_)) && mentions_commonjs_object(stmt, unresolved_mark) {
                return false;
            }
            effects_are_barrier(scan_effects(stmt, unresolved_mark))
                || is_discarded_required_call(expr, unresolved_mark, required)
        }
        _ => effects_are_barrier(scan_effects(stmt, unresolved_mark)),
    }
}
