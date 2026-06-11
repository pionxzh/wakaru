use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, Expr, ExprStmt, FnExpr, Function,
    Ident, Module, ModuleItem, Pat, ReturnStmt, Stmt, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

pub(super) fn collect_unwrap_candidates(module: &Module) -> Vec<Module> {
    let mut candidates = Vec::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        collect_umd_factory_candidates(expr, &mut candidates);
        collect_amd_define_candidates(expr, &mut candidates);
    }

    candidates
}

fn collect_umd_factory_candidates(expr: &Expr, candidates: &mut Vec<Module>) {
    let Some(call) = top_level_call(expr) else {
        return;
    };
    let Some((wrapper_params, wrapper_body)) = wrapper_callee_parts(&call.callee) else {
        return;
    };
    let Some(factory_sym) = wrapper_params.get(1) else {
        return;
    };
    if !body_looks_like_umd_wrapper(wrapper_body, factory_sym) {
        return;
    }

    let Some(factory_arg) = call.args.get(1) else {
        return;
    };
    if factory_arg.spread.is_some() {
        return;
    }
    collect_factory_expr_candidates(strip_parens(&factory_arg.expr), candidates);
}

fn collect_amd_define_candidates(expr: &Expr, candidates: &mut Vec<Module>) {
    let Some(call) = top_level_call(expr) else {
        return;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return;
    };
    let Expr::Ident(callee_ident) = strip_parens(callee_expr) else {
        return;
    };
    if callee_ident.sym.as_ref() != "define" {
        return;
    }

    let Some(factory_arg) = call.args.iter().rev().find(|arg| {
        arg.spread.is_none()
            && matches!(
                strip_parens(&arg.expr),
                Expr::Fn(_) | Expr::Arrow(_) | Expr::Call(_) | Expr::Unary(_)
            )
    }) else {
        return;
    };
    collect_factory_expr_candidates(strip_parens(&factory_arg.expr), candidates);
}

fn top_level_call(expr: &Expr) -> Option<&CallExpr> {
    match strip_parens(expr) {
        Expr::Call(call) => Some(call),
        Expr::Unary(unary) if matches!(unary.op, UnaryOp::Bang) => match strip_parens(&unary.arg) {
            Expr::Call(call) => Some(call),
            _ => None,
        },
        _ => None,
    }
}

fn wrapper_callee_parts(callee: &Callee) -> Option<(Vec<Atom>, &BlockStmt)> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    match strip_parens(callee_expr) {
        Expr::Fn(FnExpr { function, .. }) => function_parts(function),
        Expr::Arrow(arrow) => arrow_parts(arrow),
        _ => None,
    }
}

fn function_parts(function: &Function) -> Option<(Vec<Atom>, &BlockStmt)> {
    let params = function
        .params
        .iter()
        .filter_map(|param| pat_ident_sym(&param.pat))
        .collect();
    Some((params, function.body.as_ref()?))
}

fn arrow_parts(arrow: &ArrowExpr) -> Option<(Vec<Atom>, &BlockStmt)> {
    let params = arrow.params.iter().filter_map(pat_ident_sym).collect();
    let BlockStmtOrExpr::BlockStmt(body) = &*arrow.body else {
        return None;
    };
    Some((params, body))
}

fn pat_ident_sym(pat: &Pat) -> Option<Atom> {
    match pat {
        Pat::Ident(binding) => Some(binding.sym.clone()),
        _ => None,
    }
}

pub(super) fn body_looks_like_umd_wrapper(body: &BlockStmt, factory_sym: &Atom) -> bool {
    let mut visitor = UmdWrapperUseVisitor {
        factory_sym,
        seen_factory_call: false,
        seen_define: false,
        seen_exports: false,
        seen_module: false,
    };
    body.visit_with(&mut visitor);
    visitor.seen_factory_call
        && (visitor.seen_define || visitor.seen_exports || visitor.seen_module)
}

struct UmdWrapperUseVisitor<'a> {
    factory_sym: &'a Atom,
    seen_factory_call: bool,
    seen_define: bool,
    seen_exports: bool,
    seen_module: bool,
}

impl Visit for UmdWrapperUseVisitor<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee_expr) = &call.callee {
            if let Expr::Ident(ident) = strip_parens(callee_expr) {
                if &ident.sym == self.factory_sym {
                    self.seen_factory_call = true;
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        match ident.sym.as_ref() {
            "define" => self.seen_define = true,
            "exports" => self.seen_exports = true,
            "module" => self.seen_module = true,
            _ => {}
        }
    }
}

fn collect_factory_expr_candidates(expr: &Expr, candidates: &mut Vec<Module>) {
    match strip_parens(expr) {
        Expr::Fn(FnExpr { function, .. }) => collect_function_candidates(function, candidates),
        Expr::Arrow(arrow) => collect_arrow_candidates(arrow, candidates),
        expr => push_expr_candidate(expr, candidates),
    }
}

fn collect_function_candidates(function: &Function, candidates: &mut Vec<Module>) {
    let Some(body) = &function.body else {
        return;
    };
    collect_block_candidates(body, candidates);
}

fn collect_arrow_candidates(arrow: &ArrowExpr, candidates: &mut Vec<Module>) {
    match &*arrow.body {
        BlockStmtOrExpr::BlockStmt(body) => collect_block_candidates(body, candidates),
        BlockStmtOrExpr::Expr(expr) => push_expr_candidate(strip_parens(expr), candidates),
    }
}

fn collect_block_candidates(body: &BlockStmt, candidates: &mut Vec<Module>) {
    if let [Stmt::Return(ReturnStmt {
        arg: Some(expr), ..
    })] = body.stmts.as_slice()
    {
        push_expr_candidate(strip_parens(expr), candidates);
        return;
    }

    if body
        .stmts
        .iter()
        .all(|stmt| !matches!(stmt, Stmt::Return(_)))
    {
        candidates.push(module_from_stmts(body.stmts.clone()));
    }
}

fn push_expr_candidate(expr: &Expr, candidates: &mut Vec<Module>) {
    candidates.push(module_from_stmts(vec![Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(strip_parens(expr).clone()),
    })]));
}

fn module_from_stmts(stmts: Vec<Stmt>) -> Module {
    Module {
        span: DUMMY_SP,
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    }
}
