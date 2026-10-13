//! Restores the loads Terser inlined into their use in a webpack harmony
//! module, so `UnEsm` can turn them back into imports.
//!
//! webpack emits each import of an ES module as `var o = r(id)` at the top of
//! the factory. When a binding has one use, Terser inlines the load into it:
//! `r(100).A.make([…])`, `{ ...r(200) }`, `(0, n(300).Z)(…)`. The
//! call then sits in a shape `UnEsm` does not convert. Terser keeps the
//! order of calls, so the load still runs where the import's load ran, and
//! a `const` binding of it placed before its statement runs it there too.

use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, CallExpr, Callee, Class, Expr, Function, Ident, Lit, Module, ModuleItem, Stmt,
    VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::kept_require::{is_kept_require_call, kept_require_calls_in, RequirePosition};
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

use super::{collect_all_declared_names, collect_unresolved_reference_names, make_var_item};
use crate::rules::decl_utils::{fresh_binding_ident, fresh_specifier_name};

/// Whether the module calls webpack's `require.r(exports)` or
/// `require.d(exports, …)` outside a function: webpack emits them only for a
/// module whose source was ESM.
pub(super) fn is_webpack_harmony_module(module: &Module, unresolved_mark: Mark) -> bool {
    struct Finder {
        unresolved_mark: Mark,
        found: bool,
    }

    impl Visit for Finder {
        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

        fn visit_class(&mut self, _: &Class) {}

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if is_harmony_runtime_call(call, self.unresolved_mark) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
}

fn is_harmony_runtime_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return false;
    };
    if !matches!(strip_parens(&member.obj), Expr::Ident(object)
        if is_unresolved_ident(object, "require", unresolved_mark))
    {
        return false;
    }
    if !member
        .prop
        .as_ident()
        .is_some_and(|prop| matches!(prop.sym.as_ref(), "r" | "d"))
    {
        return false;
    }
    call.args.first().is_some_and(|first| {
        first.spread.is_none()
            && matches!(strip_parens(&first.expr), Expr::Ident(exports)
                if is_unresolved_ident(exports, "exports", unresolved_mark))
    })
}

/// Moves every string `require` that runs on each evaluation of the module
/// body into a `const` binding placed before its top-level statement, unless
/// the statement already has a shape `UnEsm` converts: `var x =
/// require("x")` (a member chain included) or `require("x");`. A require of
/// one specifier gets one binding, at its first use.
pub(super) fn hoist_inlined_requires(module: &mut Module, unresolved_mark: Mark) {
    let mut used_names = collect_all_declared_names(module);
    used_names.extend(collect_unresolved_reference_names(module, unresolved_mark));
    let mut bindings: HashMap<String, Ident> = HashMap::default();
    let mut body = Vec::with_capacity(module.body.len());
    for mut item in std::mem::take(&mut module.body) {
        if let ModuleItem::Stmt(stmt) = &mut item {
            let positions = kept_require_calls_in(&*stmt, unresolved_mark);
            if positions
                .iter()
                .any(|call| call.position == RequirePosition::TopLevel)
            {
                let mut rewriter = InlinedRequireRewriter {
                    unresolved_mark,
                    positions: positions.iter().map(|call| call.position).collect(),
                    next: 0,
                    skip_next: matches!(stmt, Stmt::Expr(expr)
                        if is_simple_require(&expr.expr, unresolved_mark)),
                    bindings: &mut bindings,
                    used_names: &mut used_names,
                    inserted: Vec::new(),
                };
                stmt.visit_mut_with(&mut rewriter);
                body.extend(
                    rewriter
                        .inserted
                        .into_iter()
                        .map(|(local, init)| make_var_item(local, init)),
                );
            }
        }
        body.push(item);
    }
    module.body = body;
}

/// `require("x")` or a member chain on it, `require("x").a.b`.
fn is_simple_require(expr: &Expr, unresolved_mark: Mark) -> bool {
    let mut current = strip_parens(expr);
    while let Expr::Member(member) = current {
        current = strip_parens(&member.obj);
    }
    matches!(current, Expr::Call(call) if is_string_require(call, unresolved_mark))
}

fn is_string_require(call: &CallExpr, unresolved_mark: Mark) -> bool {
    is_kept_require_call(call, unresolved_mark)
        && matches!(call.args.as_slice(), [arg]
            if arg.spread.is_none() && matches!(strip_parens(&arg.expr), Expr::Lit(Lit::Str(_))))
}

struct InlinedRequireRewriter<'a> {
    unresolved_mark: Mark,
    /// Positions of the statement's kept requires, in visit order.
    positions: Vec<RequirePosition>,
    next: usize,
    /// The next require is the whole of a simple shape `UnEsm` converts.
    skip_next: bool,
    bindings: &'a mut HashMap<String, Ident>,
    used_names: &'a mut HashSet<Atom>,
    inserted: Vec<(Ident, Box<Expr>)>,
}

impl InlinedRequireRewriter<'_> {
    fn binding_for(&mut self, source: &str, call: &Expr) -> Ident {
        if let Some(local) = self.bindings.get(source) {
            return local.clone();
        }
        let local = fresh_binding_ident(fresh_specifier_name(source, self.used_names), DUMMY_SP);
        self.bindings.insert(source.to_owned(), local.clone());
        self.inserted.push((local.clone(), Box::new(call.clone())));
        local
    }
}

impl VisitMut for InlinedRequireRewriter<'_> {
    fn visit_mut_var_declarator(&mut self, declarator: &mut VarDeclarator) {
        if declarator
            .init
            .as_deref()
            .is_some_and(|init| is_simple_require(init, self.unresolved_mark))
        {
            self.skip_next = true;
        }
        declarator.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        let Expr::Call(call) = &*expr else {
            expr.visit_mut_children_with(self);
            return;
        };
        if !is_kept_require_call(call, self.unresolved_mark) {
            expr.visit_mut_children_with(self);
            return;
        }
        let position = self.positions.get(self.next).copied();
        self.next += 1;
        if std::mem::take(&mut self.skip_next)
            || position != Some(RequirePosition::TopLevel)
            || !is_string_require(call, self.unresolved_mark)
        {
            expr.visit_mut_children_with(self);
            return;
        }
        let source = match call.args[0].expr.as_ref() {
            Expr::Lit(Lit::Str(source)) => source.value.as_str().unwrap_or("").to_owned(),
            _ => unreachable!("checked by is_string_require"),
        };
        let local = self.binding_for(&source, expr);
        *expr = Expr::Ident(local);
    }
}
