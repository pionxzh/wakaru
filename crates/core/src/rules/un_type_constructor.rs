use swc_core::common::{Mark, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, BinExpr, BinaryOp, CallExpr, Expr, ExprOrSpread, Ident, Lit, Module, Number, Str,
    UnaryExpr, UnaryOp,
};
use swc_core::ecma::utils::ExprFactory;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::RewriteLevel;

pub struct UnTypeConstructor {
    level: RewriteLevel,
    /// Context for the synthesized `String` / `Number` / `Boolean` references:
    /// they name the globals, so they carry the unresolved mark like any other
    /// generated global reference.
    unresolved_ctxt: SyntaxContext,
}

impl UnTypeConstructor {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_ctxt: SyntaxContext::empty().apply_mark(unresolved_mark),
        }
    }
}

impl VisitMut for UnTypeConstructor {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // The rewrites emit references to the globals `Number`, `String`, and
        // `Array`; a `with` statement or a direct eval anywhere in the module
        // can rebind them, so the module is left as is
        // (docs/rewrite-assumptions.md, dynamic-scope skip).
        if super::eval_utils::has_dynamic_scope_construct(module) {
            return;
        }
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if self.level < RewriteLevel::Aggressive {
            return;
        }
        expr.visit_mut_children_with(self);

        let original_span = expr.span();
        match expr {
            // +x → Number(x) — only when x is an Ident
            Expr::Unary(UnaryExpr {
                op: UnaryOp::Plus,
                arg,
                ..
            }) if matches!(**arg, Expr::Ident(_)) => {
                let arg = std::mem::replace(
                    arg,
                    Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: 0.0,
                        raw: None,
                    }))),
                );
                *expr = make_call("Number", arg, original_span, self.unresolved_ctxt);
            }

            // x + "" → String(x)  OR  "str" + "" → "str"
            Expr::Bin(BinExpr {
                op: BinaryOp::Add,
                left,
                right,
                ..
            }) if is_empty_string(right) => {
                if is_string_lit(left) {
                    let left = std::mem::replace(
                        left,
                        Box::new(Expr::Lit(Lit::Num(Number {
                            span: DUMMY_SP,
                            value: 0.0,
                            raw: None,
                        }))),
                    );
                    *expr = *left;
                } else {
                    let left = std::mem::replace(
                        left,
                        Box::new(Expr::Lit(Lit::Num(Number {
                            span: DUMMY_SP,
                            value: 0.0,
                            raw: None,
                        }))),
                    );
                    *expr = make_call("String", left, original_span, self.unresolved_ctxt);
                }
            }

            // [,,,] → Array(n) — all-holes array with n > 0
            Expr::Array(ArrayLit { elems, .. }) if is_all_holes(elems) && !elems.is_empty() => {
                let n = elems.len();
                *expr = make_call(
                    "Array",
                    Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: n as f64,
                        raw: None,
                    }))),
                    original_span,
                    self.unresolved_ctxt,
                );
            }

            _ => {}
        }
    }
}

fn make_call(name: &str, arg: Box<Expr>, span: Span, unresolved_ctxt: SyntaxContext) -> Expr {
    Expr::Call(CallExpr {
        span,
        ctxt: Default::default(),
        callee: Expr::Ident(Ident::new(name.into(), DUMMY_SP, unresolved_ctxt)).as_callee(),
        args: vec![arg.as_arg()],
        type_args: None,
    })
}

fn is_empty_string(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(Str { value, .. })) if value.is_empty())
}

fn is_string_lit(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(_)))
}

fn is_all_holes(elems: &[Option<ExprOrSpread>]) -> bool {
    elems.iter().all(|e| e.is_none())
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, Globals, Mark, SourceMap, GLOBALS};
    use swc_core::ecma::ast::{Callee, Decl, ModuleItem, Stmt};
    use swc_core::ecma::transforms::base::resolver;

    use super::*;

    #[test]
    fn synthesized_number_callee_carries_the_unresolved_mark() {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = crate::unpacker::parse_es_module("const a = +x;", "fixture.js", cm)
                .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, Mark::new(), false));

            module.visit_mut_with(&mut UnTypeConstructor::new(
                unresolved_mark,
                RewriteLevel::Aggressive,
            ));

            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[0] else {
                panic!("expected a var declaration");
            };
            let init = var.decls[0].init.as_deref().expect("init");
            let Expr::Call(call) = init else {
                panic!("expected Number(x), got {init:?}");
            };
            let Callee::Expr(callee) = &call.callee else {
                panic!("expected an expression callee");
            };
            let Expr::Ident(id) = callee.as_ref() else {
                panic!("expected an identifier callee");
            };
            assert_eq!(id.sym.as_ref(), "Number");
            assert_eq!(id.ctxt.outer(), unresolved_mark);
        });
    }
}
