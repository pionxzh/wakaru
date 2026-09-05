use swc_core::atoms::Atom;
use swc_core::common::{Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, CallExpr, Callee, Decl, Expr, ExprStmt, FnExpr, Function,
    FunctionBody, Ident, Module, ModuleItem, Pat, ReturnStmt, Stmt, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::{strip_parens, strip_parens_mut};

pub(super) fn collect_unwrap_candidates(module: &Module) -> Vec<Module> {
    let mut candidates = Vec::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        collect_umd_factory_candidates(expr, &mut candidates);
        collect_amd_define_candidates(expr, &mut candidates);
    }
    collect_plain_iife_module_candidate(module, &mut candidates);

    candidates
}

// Plain IIFEs are much broader than UMD/AMD wrapper shapes, so only accept one
// when it owns the entire parsed module. Otherwise a successful inner detector
// would silently discard sibling top-level statements.
fn collect_plain_iife_module_candidate(module: &Module, candidates: &mut Vec<Module>) {
    let [item] = module.body.as_slice() else {
        return;
    };
    match item {
        ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => {
            collect_plain_iife_candidates(expr, candidates);
        }
        // var app = (() => { ... })();  — esbuild --global-name / rollup
        // iife output assigns the wrapper result to a global.
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            let [declarator] = var_decl.decls.as_slice() else {
                return;
            };
            if !matches!(declarator.name, Pat::Ident(_)) {
                return;
            }
            if let Some(init) = &declarator.init {
                collect_plain_iife_candidates(init, candidates);
            }
        }
        _ => {}
    }
}

// Zero-arg IIFE wrapper around an entire bundle: `(() => { ... })();`,
// `(function () { ... })();`, or `!function () { ... }();`. esbuild's
// --format=iife (the browser default) and rollup's iife output emit this
// shape. The body becomes a detection candidate; a trailing `return X;`
// (named-global form) becomes `X;` so its startup/effects still run even
// though the wrapper result itself is not represented by the module graph.
fn collect_plain_iife_candidates(expr: &Expr, candidates: &mut Vec<Module>) {
    let Some(call) = top_level_call(expr) else {
        return;
    };
    if !call.args.is_empty() {
        return;
    }
    let Some(body) = plain_iife_callee_body(&call.callee) else {
        return;
    };

    let stmts = body.stmts.as_slice();
    if super::stmts_have_function_level_special_bindings(stmts) {
        return;
    }
    let mut inner = stmts.to_vec();
    if let Some(Stmt::Return(return_stmt)) = inner.last_mut() {
        if let Some(arg) = return_stmt.arg.take() {
            *inner.last_mut().expect("terminal return is present") = Stmt::Expr(ExprStmt {
                span: return_stmt.span,
                expr: arg,
            });
        } else {
            inner.pop();
        }
    }
    if inner.is_empty() || super::stmts_have_function_level_return(&inner) {
        return;
    }
    candidates.push(module_from_stmts(inner));
}

fn plain_iife_callee_body(callee: &Callee) -> Option<&FunctionBody> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    match strip_parens(callee_expr) {
        Expr::Fn(FnExpr {
            ident: None,
            function,
        }) if !function.is_async && !function.is_generator && function.params.is_empty() => {
            function.body.as_ref()
        }
        Expr::Arrow(arrow) if !arrow.is_async && arrow.params.is_empty() => {
            let ArrowFunctionBody::FunctionBody(body) = &*arrow.body else {
                return None;
            };
            Some(body)
        }
        _ => None,
    }
}

/// Tries Bun's compiled CommonJS container by moving its body into the
/// detector candidate instead of cloning the entire embedded bundle.
///
/// A rejected candidate is restored exactly so an ordinary function with the
/// same parameter names still follows the plain-source fallback.
pub(super) fn try_detect_bun_compile_candidate<T>(
    module: &mut Module,
    mut detect: impl FnMut(Module) -> Result<T, Module>,
) -> Option<T> {
    for item in &mut module.body {
        let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
            continue;
        };
        let Some(body) = bun_compile_body_mut(expr) else {
            continue;
        };
        let body_span = body.span;
        let mut candidate = module_from_stmts(std::mem::take(&mut body.stmts));
        candidate.span = body_span;
        let candidate = match detect(candidate) {
            Ok(result) => return Some(result),
            Err(candidate) => candidate,
        };
        body.stmts = candidate
            .body
            .into_iter()
            .map(|item| match item {
                ModuleItem::Stmt(stmt) => stmt,
                ModuleItem::ModuleDecl(_) => {
                    unreachable!("Bun wrapper candidates contain statements only")
                }
            })
            .collect();
    }
    None
}

fn bun_compile_body_mut(expr: &mut Box<Expr>) -> Option<&mut FunctionBody> {
    let Expr::Fn(FnExpr { function, .. }) = strip_parens_mut(expr) else {
        return None;
    };
    if function.is_async || function.is_generator {
        return None;
    }
    let params = function
        .params
        .iter()
        .map(|param| pat_ident_sym(&param.pat))
        .collect::<Option<Vec<_>>>()?;
    let expected = ["exports", "require", "module", "__filename", "__dirname"];
    if params.len() != expected.len()
        || params
            .iter()
            .zip(expected)
            .any(|(actual, expected)| actual.as_ref() != expected)
    {
        return None;
    }
    function.body.as_mut()
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

fn wrapper_callee_parts(callee: &Callee) -> Option<(Vec<Atom>, &FunctionBody)> {
    let Callee::Expr(callee_expr) = callee else {
        return None;
    };
    match strip_parens(callee_expr) {
        Expr::Fn(FnExpr { function, .. }) => function_parts(function),
        Expr::Arrow(arrow) => arrow_parts(arrow),
        _ => None,
    }
}

fn function_parts(function: &Function) -> Option<(Vec<Atom>, &FunctionBody)> {
    let params = function
        .params
        .iter()
        .filter_map(|param| pat_ident_sym(&param.pat))
        .collect();
    Some((params, function.body.as_ref()?))
}

fn arrow_parts(arrow: &ArrowExpr) -> Option<(Vec<Atom>, &FunctionBody)> {
    let params = arrow.params.iter().filter_map(pat_ident_sym).collect();
    let ArrowFunctionBody::FunctionBody(body) = &*arrow.body else {
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

pub(super) fn body_looks_like_umd_wrapper(body: &FunctionBody, factory_sym: &Atom) -> bool {
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
        ArrowFunctionBody::FunctionBody(body) => collect_block_candidates(body, candidates),
        ArrowFunctionBody::Expr(expr) => push_expr_candidate(strip_parens(expr), candidates),
    }
}

fn collect_block_candidates(body: &FunctionBody, candidates: &mut Vec<Module>) {
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
    let expr = strip_parens(expr);
    candidates.push(module_from_stmts(vec![Stmt::Expr(ExprStmt {
        span: expr.span(),
        expr: Box::new(expr.clone()),
    })]));
}

/// Candidates keep a real span covering their statements: downstream
/// detection looks up the candidate's source file through `module.span`
/// (e.g. esbuild path-comment hints), and a `DUMMY_SP` module panics there.
fn module_from_stmts(stmts: Vec<Stmt>) -> Module {
    let span = match (stmts.first(), stmts.last()) {
        (Some(first), Some(last)) if first.span().lo.0 != 0 && last.span().hi.0 != 0 => {
            Span::new(first.span().lo, last.span().hi)
        }
        _ => DUMMY_SP,
    };
    Module {
        span,
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};

    use super::try_detect_bun_compile_candidate;

    #[test]
    fn rejected_bun_compile_candidate_restores_moved_body() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = crate::unpacker::parse_es_module(
                r#"
                (function(exports, require, module, __filename, __dirname) {
                    const value = 1;
                    module.exports = value;
                })
                "#,
                "bun-wrapper.js",
                cm,
            )
            .expect("fixture should parse");

            let mut rejected_body_len = 0;
            let rejected = try_detect_bun_compile_candidate(&mut module, |candidate| {
                rejected_body_len = candidate.body.len();
                Err::<(), _>(candidate)
            });
            assert!(rejected.is_none());
            assert_eq!(rejected_body_len, 2);

            let accepted =
                try_detect_bun_compile_candidate(&mut module, |candidate| Ok(candidate.body.len()));
            assert_eq!(accepted, Some(2), "rejected body should be restored");
        });
    }
}
