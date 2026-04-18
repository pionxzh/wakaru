use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, BindingIdent, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, CatchClause, ClassDecl,
    Constructor, Decl, Expr, ExprOrSpread, FnDecl, Function, GetterProp, Ident, Lit, MemberProp,
    MethodProp, ObjectPatProp, Param, ParamOrTsParamProp, Pat, SetterProp, Stmt, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct UnIife;

impl VisitMut for UnIife {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Call(call_expr) = expr {
            // Try to simplify `(() => expr)()` (zero-param, expression-body arrow IIFE with no args)
            if let Some(inner) = try_simplify_arrow_expr_iife(call_expr) {
                *expr = *inner;
                return;
            }
            process_iife(call_expr);
        }
    }
}

/// Simplifies `(() => expr)()` to `expr` (zero-param arrow with expression body, called with no args).
/// This handles the output of `require.n(r)` to `() => r` after inlining.
fn try_simplify_arrow_expr_iife(call: &CallExpr) -> Option<Box<Expr>> {
    // Must have no arguments
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let arrow = match callee_expr.as_ref() {
        Expr::Arrow(a) => a,
        Expr::Paren(p) => match p.expr.as_ref() {
            Expr::Arrow(a) => a,
            _ => return None,
        },
        _ => return None,
    };
    // Must have no params
    if !arrow.params.is_empty() {
        return None;
    }
    // Body must be an expression (not a block)
    match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(e) => Some(e.clone()),
        _ => None,
    }
}

fn process_iife(call: &mut CallExpr) {
    match &mut call.callee {
        Callee::Expr(callee_expr) => match callee_expr.as_mut() {
            Expr::Fn(fn_expr) => {
                process_fn_iife(&mut fn_expr.function, &mut call.args);
            }
            Expr::Arrow(arrow_expr) => {
                process_arrow_iife(arrow_expr, &mut call.args);
            }
            Expr::Paren(paren) => match paren.expr.as_mut() {
                Expr::Fn(fn_expr) => {
                    process_fn_iife(&mut fn_expr.function, &mut call.args);
                }
                Expr::Arrow(arrow_expr) => {
                    process_arrow_iife(arrow_expr, &mut call.args);
                }
                _ => {}
            },
            _ => {}
        },
        _ => {}
    }
}

fn process_fn_iife(function: &mut Function, args: &mut Vec<ExprOrSpread>) {
    let Some(body) = &mut function.body else {
        return;
    };
    if should_preserve_iife_shape(body, function.params.len(), args.len()) {
        return;
    }
    let preserve_arg_list = body_uses_own_arguments(body);
    process_params_and_args(&mut function.params, args, body, preserve_arg_list);
}

fn process_arrow_iife(arrow: &mut ArrowExpr, args: &mut Vec<ExprOrSpread>) {
    let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_mut() else {
        return;
    };
    if should_preserve_iife_shape(body, arrow.params.len(), args.len()) {
        return;
    }
    // Arrow functions do not have their own `arguments` binding, so removing
    // arrow params cannot change what an `arguments` reference observes.
    process_arrow_params_and_args(&mut arrow.params, args, body, false);
}

fn should_preserve_iife_shape(body: &BlockStmt, param_count: usize, arg_count: usize) -> bool {
    // UnEs6Class detects Babel's inline `_inherits` helper from its original
    // two-param/two-arg IIFE shape. If UnIife rewrites the superclass param
    // first, that later class rewrite can lose its inheritance evidence.
    param_count == 2 && arg_count == 2 && body_contains_object_create(body)
}

fn process_params_and_args(
    params: &mut Vec<Param>,
    args: &mut Vec<ExprOrSpread>,
    body: &mut BlockStmt,
    preserve_arg_list: bool,
) {
    let plan = plan_param_rewrites(params.len(), args, body, preserve_arg_list, |i| {
        pat_ident(&params[i].pat).map(|id| (id.sym.clone(), id.ctxt))
    });

    // Apply renames in place: param keeps its own ctxt, only the sym changes.
    for (i, _, new_sym, _) in &plan.renames {
        if let Pat::Ident(bi) = &mut params[*i].pat {
            bi.id.sym = new_sym.clone();
        }
    }

    apply_body_rewrites(body, &plan);

    // Process const_inserts: collect indices to remove, then drop params/args
    // in reverse-index order.
    let mut to_remove: Vec<usize> = plan.const_inserts.iter().map(|(i, ..)| *i).collect();
    to_remove.sort();
    to_remove.dedup();
    to_remove.reverse();
    for i in to_remove {
        params.remove(i);
        args.remove(i);
    }

    prepend_const_decls(body, &plan.const_inserts);
}

fn process_arrow_params_and_args(
    params: &mut Vec<Pat>,
    args: &mut Vec<ExprOrSpread>,
    body: &mut BlockStmt,
    preserve_arg_list: bool,
) {
    let plan = plan_param_rewrites(params.len(), args, body, preserve_arg_list, |i| {
        pat_ident(&params[i]).map(|id| (id.sym.clone(), id.ctxt))
    });

    for (i, _, new_sym, _) in &plan.renames {
        if let Pat::Ident(bi) = &mut params[*i] {
            bi.id.sym = new_sym.clone();
        }
    }

    apply_body_rewrites(body, &plan);

    let mut to_remove: Vec<usize> = plan.const_inserts.iter().map(|(i, ..)| *i).collect();
    to_remove.sort();
    to_remove.dedup();
    to_remove.reverse();
    for i in to_remove {
        params.remove(i);
        args.remove(i);
    }

    prepend_const_decls(body, &plan.const_inserts);
}

fn pat_ident(pat: &Pat) -> Option<&Ident> {
    if let Pat::Ident(BindingIdent { id, .. }) = pat {
        Some(id)
    } else {
        None
    }
}

/// Per-param rewrite decisions for an IIFE call. Shared between the regular
/// `Function` and arrow paths since the logic is identical once we abstract
/// param introspection.
#[derive(Default)]
struct RewritePlan {
    /// (idx, old_sym, new_sym, param_ctxt): keep the param, change its sym.
    renames: Vec<(usize, Atom, Atom, SyntaxContext)>,
    /// (idx, sym, ctxt, lit): drop the param + arg and prepend `const sym = lit`.
    const_inserts: Vec<(usize, Atom, SyntaxContext, Lit)>,
}

fn plan_param_rewrites<F>(
    param_count: usize,
    args: &[ExprOrSpread],
    body: &BlockStmt,
    preserve_arg_list: bool,
    param_at: F,
) -> RewritePlan
where
    F: Fn(usize) -> Option<(Atom, SyntaxContext)>,
{
    let mut plan = RewritePlan::default();
    let arg_count = args.len();

    // Printed JavaScript has no `SyntaxContext`, so every new name we introduce
    // must avoid bindings anywhere in the IIFE body. This is conservative for
    // suffix renames, but avoids producing a param name that is shadowed at a
    // nested use site after codegen.
    let mut taken_for_suffix: HashSet<Atom> = collect_all_binding_names(body);
    for i in 0..param_count {
        if let Some((sym, _)) = param_at(i) {
            taken_for_suffix.insert(sym);
        }
    }

    for i in 0..param_count {
        let Some((param_sym, param_ctxt)) = param_at(i) else {
            continue;
        };
        if param_sym.len() != 1 {
            continue;
        }
        if i >= arg_count {
            continue;
        }

        match args[i].expr.as_ref() {
            Expr::Ident(ident) => {
                if ident.sym.len() <= 1 || ident.sym == param_sym {
                    continue;
                }
                if ident.sym.as_ref() == "undefined" {
                    continue;
                }
                // Keep identifier args as parameters. Dropping the param would
                // turn a call-time snapshot into a live read of the outer
                // binding, which is unsafe for closures and body-side effects.
                let mut taken = taken_for_suffix.clone();
                taken.insert(ident.sym.clone());
                let new_sym = pick_non_conflicting_name(&ident.sym, &taken);
                taken_for_suffix.remove(&param_sym);
                taken_for_suffix.insert(new_sym.clone());
                plan.renames.push((i, param_sym, new_sym, param_ctxt));
            }
            Expr::Lit(lit) => {
                if !preserve_arg_list {
                    plan.const_inserts
                        .push((i, param_sym, param_ctxt, lit.clone()));
                }
            }
            _ => {}
        }
    }

    plan
}

fn apply_body_rewrites(body: &mut BlockStmt, plan: &RewritePlan) {
    // Rename refs (sym only; keep ctxt so the inner binding stays distinct).
    for (_, old_sym, new_sym, param_ctxt) in &plan.renames {
        let mut renamer = RenameIdent {
            old_sym: old_sym.clone(),
            new_sym: new_sym.clone(),
            target_ctxt: *param_ctxt,
        };
        body.visit_mut_with(&mut renamer);
    }
}

fn prepend_const_decls(body: &mut BlockStmt, const_inserts: &[(usize, Atom, SyntaxContext, Lit)]) {
    if const_inserts.is_empty() {
        return;
    }
    // Sort by ascending original index to preserve declaration order.
    let mut sorted: Vec<&(usize, Atom, SyntaxContext, Lit)> = const_inserts.iter().collect();
    sorted.sort_by_key(|t| t.0);
    let const_stmts: Vec<Stmt> = sorted
        .into_iter()
        .map(|(_, sym, ctxt, lit)| make_const_decl(sym.clone(), *ctxt, lit.clone()))
        .collect();
    let old_body = std::mem::take(&mut body.stmts);
    body.stmts = const_stmts;
    body.stmts.extend(old_body);
}

/// Pick a name that doesn't collide with anything in `taken`. Returns
/// `preferred` when free; otherwise appends `_1`, `_2`, ... until unique.
fn pick_non_conflicting_name(preferred: &Atom, taken: &HashSet<Atom>) -> Atom {
    if !taken.contains(preferred) {
        return preferred.clone();
    }
    for suffix in 1usize.. {
        let candidate: Atom = format!("{}_{}", preferred, suffix).into();
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Collect every binding name introduced anywhere inside the body, including
/// nested function/arrow params and bodies, catch params, etc. Used to decide
/// whether substituting an outer ident into the body could be shadowed by an
/// inner binding with the same name.
fn collect_all_binding_names(body: &BlockStmt) -> HashSet<Atom> {
    struct Collector {
        names: HashSet<Atom>,
    }

    fn collect_pat(pat: &Pat, names: &mut HashSet<Atom>) {
        match pat {
            Pat::Ident(b) => {
                names.insert(b.id.sym.clone());
            }
            Pat::Array(a) => {
                for elem in a.elems.iter().flatten() {
                    collect_pat(elem, names);
                }
            }
            Pat::Object(o) => {
                for prop in &o.props {
                    match prop {
                        ObjectPatProp::Assign(a) => {
                            names.insert(a.key.sym.clone());
                        }
                        ObjectPatProp::KeyValue(kv) => collect_pat(&kv.value, names),
                        ObjectPatProp::Rest(r) => collect_pat(&r.arg, names),
                    }
                }
            }
            Pat::Rest(r) => collect_pat(&r.arg, names),
            Pat::Assign(a) => collect_pat(&a.left, names),
            _ => {}
        }
    }

    impl Visit for Collector {
        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            collect_pat(&decl.name, &mut self.names);
            decl.visit_children_with(self);
        }
        fn visit_fn_decl(&mut self, decl: &FnDecl) {
            self.names.insert(decl.ident.sym.clone());
            decl.visit_children_with(self);
        }
        fn visit_class_decl(&mut self, decl: &ClassDecl) {
            self.names.insert(decl.ident.sym.clone());
            decl.visit_children_with(self);
        }
        fn visit_function(&mut self, f: &Function) {
            for p in &f.params {
                collect_pat(&p.pat, &mut self.names);
            }
            f.visit_children_with(self);
        }
        fn visit_arrow_expr(&mut self, a: &ArrowExpr) {
            for p in &a.params {
                collect_pat(p, &mut self.names);
            }
            a.visit_children_with(self);
        }
        fn visit_constructor(&mut self, c: &Constructor) {
            for p in &c.params {
                if let ParamOrTsParamProp::Param(p) = p {
                    collect_pat(&p.pat, &mut self.names);
                }
            }
            c.visit_children_with(self);
        }
        fn visit_catch_clause(&mut self, c: &CatchClause) {
            if let Some(p) = &c.param {
                collect_pat(p, &mut self.names);
            }
            c.visit_children_with(self);
        }
    }

    let mut c = Collector {
        names: HashSet::new(),
    };
    body.visit_with(&mut c);
    c.names
}

fn make_const_decl(name: Atom, binding_ctxt: SyntaxContext, lit: Lit) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: Ident {
                    span: DUMMY_SP,
                    ctxt: binding_ctxt,
                    sym: name,
                    optional: false,
                },
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Lit(lit))),
            definite: false,
        }],
    })))
}

struct RenameIdent {
    old_sym: Atom,
    new_sym: Atom,
    /// Match by (sym, ctxt) so a same-named binding in a nested scope is not
    /// rewritten when we rename the IIFE param to an arg-derived alias.
    target_ctxt: SyntaxContext,
}

impl VisitMut for RenameIdent {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        if ident.sym == self.old_sym && ident.ctxt == self.target_ctxt {
            ident.sym = self.new_sym.clone();
        }
    }
}

fn body_uses_own_arguments(body: &BlockStmt) -> bool {
    struct Checker {
        found: bool,
    }

    impl Visit for Checker {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym.as_ref() == "arguments" {
                self.found = true;
            }
        }

        // Nested non-arrow functions have their own `arguments` binding. Arrow
        // functions intentionally keep the default traversal because they
        // capture the enclosing function's `arguments`.
        fn visit_function(&mut self, _: &Function) {}
        fn visit_constructor(&mut self, _: &Constructor) {}
        fn visit_method_prop(&mut self, _: &MethodProp) {}
        fn visit_getter_prop(&mut self, _: &GetterProp) {}
        fn visit_setter_prop(&mut self, _: &SetterProp) {}
    }

    let mut checker = Checker { found: false };
    body.visit_with(&mut checker);
    checker.found
}

fn body_contains_object_create(body: &BlockStmt) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if is_object_create_call(call) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder { found: false };
    body.visit_with(&mut finder);
    finder.found
}

fn is_object_create_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };

    obj.sym.as_ref() == "Object"
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "create")
}
